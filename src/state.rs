//! The mixer's replicated state: the live state of model.schema.json over
//! the configured shape, touch ownership, the last seq per session and
//! control, and scenes. Every replica applies the same commands in log
//! order through [`Product`], so all of it is decided here and nowhere
//! else.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use samplez_replica::entry::{Origin, SessionKey};
use samplez_replica::state_machine::{Changed, Effect, Product};
use samplez_service::OwnerHandle;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::contract::{check, contract};
use crate::shape::Shape;

/// A client's command as the replicated log carries it. `id` is the
/// client's command id, under which a refusal or a stored scene is kept
/// for the answer ([`Mixer::answer`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Command {
    /// A WebSocket `set`, `touch` or `release` of one control.
    Control(Control),
    /// `PATCH /state`: every change or none.
    Patch {
        id: String,
        changes: Vec<Write>,
    },
    /// `POST /scenes` without `scene`, `PUT /scenes/{id}` with it.
    StoreScene {
        id: String,
        scene: Option<String>,
        name: Option<String>,
        /// When the receiving replica took the request, so every replica
        /// stores the same time.
        at: String,
    },
    DeleteScene {
        id: String,
        scene: String,
    },
    Recall {
        id: String,
        scene: String,
        at: String,
    },
    /// `POST /afv/tally` (one source) or `PUT /afv/tally` (`replace`: the
    /// sources not listed go `off`).
    Tally {
        id: String,
        sources: BTreeMap<String, String>,
        replace: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Control {
    pub id: String,
    pub action: Action,
    /// The `kind` of the connection's hello.
    pub kind: String,
    pub seq: u64,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Set,
    Touch,
    Release,
}

/// One control and the value written to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub path: String,
    pub value: Value,
}

/// What a command that changed nothing gets beside its resolution, or
/// the scene a scene command stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "camelCase")]
pub enum Answer {
    /// 409: the controls are owned by another session; each entry is a
    /// model `Conflict`.
    Conflict { conflicts: Vec<Value> },
    /// 404.
    NotFound { message: String },
    /// 400.
    Invalid { message: String },
    /// The scene a `StoreScene` stored.
    Scene { scene: String },
}

/// What one command or expiry changed, for [`Product::delta`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Edits {
    values: Vec<(String, Value)>,
    owners: Vec<(String, Value)>,
    /// The delta's `origins` entry: client commands only.
    origin: Option<Value>,
}

/// The holder of a touched control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Holder {
    principal: String,
    /// Kept in the replicated state, never published: the owner record
    /// carries its handle.
    session: String,
    kind: String,
    connection: String,
}

impl Holder {
    fn key(&self) -> SessionKey {
        SessionKey::from_parts(&self.principal, &self.session)
    }

    /// The model's `Owner`.
    fn public(&self) -> Value {
        json!({
            "principal": self.principal,
            "handle": OwnerHandle::of(&self.session).as_str(),
            "kind": self.kind,
            "connection": self.connection,
        })
    }
}

/// Answers kept per session, the newest; as many as the dedup table keeps
/// command ids of one session, so a duplicate finds its answer.
const ANSWERS_KEPT: usize = 1024;

#[derive(Default)]
pub struct Mixer {
    /// `None` only in a [`Default`] state before a snapshot is loaded.
    shape: Option<Arc<Shape>>,
    state: Value,
    owners: BTreeMap<String, Holder>,
    seqs: BTreeMap<SessionKey, BTreeMap<String, u64>>,
    answers: BTreeMap<SessionKey, VecDeque<(String, Answer)>>,
    /// Scene id -> the stored part of the state.
    scenes: BTreeMap<String, Value>,
    scenes_created: u64,
    /// Leaves of a loaded state that the configured shape has no place
    /// for, or whose value it does not take.
    dropped: Vec<String>,
}

/// The state as snapshots carry it.
#[derive(Serialize, Deserialize)]
struct Saved {
    shape: Shape,
    state: Value,
    owners: Vec<(String, Holder)>,
    seqs: Vec<(SessionKey, Vec<(String, u64)>)>,
    answers: Vec<(SessionKey, Vec<(String, Answer)>)>,
    scenes: Vec<(String, Value)>,
    scenes_created: u64,
}

impl Mixer {
    pub fn new(shape: Shape) -> Mixer {
        Mixer {
            state: shape.initial_state(),
            shape: Some(Arc::new(shape)),
            ..Mixer::default()
        }
    }

    pub fn shape(&self) -> &Shape {
        self.shape
            .as_deref()
            .expect("a shape is configured or loaded")
    }

    /// The live state, model `State`.
    pub fn state(&self) -> &Value {
        &self.state
    }

    /// Owned controls, model `Owners`.
    pub fn owners(&self) -> Value {
        Value::Object(
            self.owners
                .iter()
                .map(|(path, h)| (path.clone(), h.public()))
                .collect(),
        )
    }

    /// What the command `id` of `session` got beside its resolution.
    pub fn answer(&self, session: &SessionKey, id: &str) -> Option<&Answer> {
        self.answers
            .get(session)?
            .iter()
            .rev()
            .find(|(i, _)| i == id)
            .map(|(_, a)| a)
    }

    /// The leaves the last [`Product::load`] dropped.
    pub fn dropped(&self) -> &[String] {
        &self.dropped
    }

    pub fn has_scene(&self, scene: &str) -> bool {
        self.scenes.contains_key(scene)
    }

    /// The model `Scene` record of `scene`.
    pub fn scene(&self, scene: &str) -> Option<&Value> {
        self.state["scenes"]
            .as_array()?
            .iter()
            .find(|s| s["id"] == scene)
    }

    /// Whether `path` names a control of this shape: an existing leaf,
    /// or the recall-safe groups of an element.
    pub fn exists(&self, path: &str) -> bool {
        let tokens = tokens(path);
        if tokens.first().map(String::as_str) == Some("recallSafe") {
            return tokens.len() == 2 && self.shape().recallable(&tokens[1]);
        }
        self.state.pointer(path).is_some_and(|v| !v.is_object())
    }

    /// The checks of a write that do not depend on the order of commands:
    /// the path names a control (404), and the value is one its leaf
    /// takes (400): the contract's `Change`, then what it names exists.
    pub fn check_write(&self, path: &str, value: &Value) -> Result<(), Answer> {
        if !self.exists(path) {
            return Err(Answer::NotFound {
                message: format!("unknown control {path}"),
            });
        }
        let invalid = |message: String| Err(Answer::Invalid { message });
        if let Err(e) = check(&contract().change, &json!({ "path": path, "value": value })) {
            return invalid(format!("invalid value for {path}: {e}"));
        }
        self.check_references(path, value).or_else(invalid)
    }

    /// Ids a value names exist in the shape.
    fn check_references(&self, path: &str, value: &Value) -> Result<(), String> {
        let shape = self.shape();
        let ids = || {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        };
        if path.ends_with("/exclude") || path.ends_with("/members") {
            if let Some(bad) = ids().find(|&id| !shape.is_channel(id)) {
                return Err(format!("unknown channel {bad}"));
            }
        } else if path == "/tone/targets" {
            if let Some(bad) = ids().find(|&id| !shape.is_output_bus(id)) {
                return Err(format!("unknown output {bad}"));
            }
        } else if path == "/monitor/source" {
            if !value.as_str().is_some_and(|s| shape.is_output_bus(s)) {
                return Err(format!("unknown monitor source {value}"));
            }
        } else if let [group, channel, leaf] = tokens(path).as_slice()
            && (group.as_str(), leaf.as_str()) == ("channels", "input")
            && value == "b"
            && !shape
                .channels
                .iter()
                .any(|c| &c.id == channel && c.alt_input)
        {
            return Err(format!("channel {channel} has no B input"));
        }
        Ok(())
    }

    fn record(&mut self, session: &SessionKey, id: &str, answer: Answer) {
        let kept = self.answers.entry(session.clone()).or_default();
        if kept.len() == ANSWERS_KEPT {
            kept.pop_front();
        }
        kept.push_back((id.to_owned(), answer));
    }

    fn refuse(&mut self, session: &SessionKey, id: &str, answer: Answer) -> Effect<Edits> {
        self.record(session, id, answer);
        Effect::Unchanged
    }

    fn conflict(&self, path: &str, holder: &Holder) -> Value {
        json!({
            "path": path,
            "current": self.state.pointer(path).cloned().unwrap_or(Value::Null),
            "owner": holder.public(),
        })
    }

    /// Writes `written` and what follows from it, and returns every leaf
    /// that changed: loudness target and tolerance from a preset, preset
    /// `custom` from a written target or tolerance, AFV stages from
    /// bindings and tally.
    fn write(&mut self, written: Vec<(String, Value)>) -> Vec<(String, Value)> {
        let mut all = written.clone();
        let preset_written = written
            .iter()
            .any(|(p, _)| p == "/metering/loudness/preset");
        for (path, value) in &written {
            match (path.as_str(), value.as_str()) {
                ("/metering/loudness/preset", Some("ebu")) => {
                    all.push(("/metering/loudness/target".into(), json!(-23)));
                    all.push(("/metering/loudness/tolerance".into(), json!(1)));
                }
                ("/metering/loudness/preset", Some("atsc")) => {
                    all.push(("/metering/loudness/target".into(), json!(-24)));
                    all.push(("/metering/loudness/tolerance".into(), json!(2)));
                }
                ("/metering/loudness/target" | "/metering/loudness/tolerance", _)
                    if !preset_written =>
                {
                    all.push(("/metering/loudness/preset".into(), json!("custom")));
                }
                _ => {}
            }
        }
        let afv = all
            .iter()
            .any(|(p, _)| p.starts_with("/afv/") || p.starts_with("/tally/"));
        let opens = afv.then(|| self.afv_open());
        let mut changed: Vec<(String, Value)> = Vec::new();
        for (path, value) in all {
            if self.state.pointer(&path) != Some(&value) {
                set_at(&mut self.state, &path, value.clone());
                changed.retain(|(p, _)| *p != path);
                changed.push((path, value));
            }
        }
        if let Some(before) = opens {
            for (channel, open) in self.afv_open() {
                if before.get(&channel) != Some(&open) {
                    let path = format!("/afv/{channel}/open");
                    set_at(&mut self.state, &path, json!(open));
                    changed.push((path, json!(open)));
                }
            }
        }
        changed
    }

    /// Each channel's AFV stage as bindings and tally set it: open when
    /// disabled, else open on `program` of its source, else as it was.
    fn afv_open(&self) -> BTreeMap<String, bool> {
        let afv = self.state["afv"].as_object().into_iter().flatten();
        afv.map(|(channel, b)| {
            let source = b["source"]
                .as_str()
                .and_then(|s| self.state["tally"][s].as_str());
            let open = if b["enabled"] != true {
                true
            } else {
                source.map_or(b["open"] == true, |t| t == "program")
            };
            (channel.clone(), open)
        })
        .collect()
    }

    /// Momentary controls among `paths` (IFB interrupt) go back to off.
    fn momentary_off(&self, paths: &[String]) -> Vec<(String, Value)> {
        paths
            .iter()
            .filter(|p| p.starts_with("/mixMinus/") && p.ends_with("/interrupt/active"))
            .filter(|p| self.state.pointer(p) == Some(&json!(true)))
            .map(|p| (p.clone(), json!(false)))
            .collect()
    }

    fn control(&mut self, origin: &Origin, c: &Control) -> Effect<Edits> {
        let (id, action, kind, seq, path) = (
            c.id.as_str(),
            c.action,
            c.kind.as_str(),
            c.seq,
            c.path.as_str(),
        );
        let value = c.value.as_ref();
        let session = origin.key();
        let holder = self.owners.get(path).cloned();
        if action != Action::Release
            && let Some(h) = holder.as_ref().filter(|h| h.key() != session)
        {
            let conflicts = vec![self.conflict(path, h)];
            return self.refuse(&session, id, Answer::Conflict { conflicts });
        }
        if !self.exists(path) {
            let message = format!("unknown control {path}");
            return self.refuse(&session, id, Answer::NotFound { message });
        }
        let last = self.seqs.get(&session).and_then(|s| s.get(path)).copied();
        if last.is_some_and(|last| seq <= last) {
            return Effect::Stale;
        }
        let me = Holder {
            principal: origin.principal.clone(),
            session: origin.session.clone(),
            kind: kind.to_owned(),
            connection: origin.connection.clone(),
        };
        let origin = Some(json!({ "owner": me.public(), "commandId": id, "seq": seq }));
        let edits = match action {
            Action::Touch => {
                self.seen(&session, path, seq);
                if holder.is_some() {
                    return Effect::Unchanged;
                }
                let owners = vec![(path.to_owned(), me.public())];
                self.owners.insert(path.to_owned(), me);
                Edits {
                    owners,
                    origin,
                    ..Edits::default()
                }
            }
            Action::Release => {
                self.seen(&session, path, seq);
                if holder.is_none_or(|h| h.key() != session) {
                    return Effect::Unchanged;
                }
                self.owners.remove(path);
                let off = self.momentary_off(&[path.to_owned()]);
                Edits {
                    values: self.write(off),
                    owners: vec![(path.to_owned(), Value::Null)],
                    origin,
                }
            }
            Action::Set => {
                let value = value.cloned().unwrap_or(Value::Null);
                if let Err(answer) = self.check_write(path, &value) {
                    return self.refuse(&session, id, answer);
                }
                self.seen(&session, path, seq);
                let values = self.write(vec![(path.to_owned(), value)]);
                if values.is_empty() {
                    return Effect::Unchanged;
                }
                Edits {
                    values,
                    origin,
                    ..Edits::default()
                }
            }
        };
        Effect::Changed(edits)
    }

    fn seen(&mut self, session: &SessionKey, path: &str, seq: u64) {
        self.seqs
            .entry(session.clone())
            .or_default()
            .insert(path.to_owned(), seq);
    }

    fn patch(&mut self, session: &SessionKey, id: &str, changes: &[Write]) -> Effect<Edits> {
        let conflicts: Vec<Value> = changes
            .iter()
            .filter_map(|c| Some(self.conflict(&c.path, self.owners.get(&c.path)?)))
            .collect();
        if !conflicts.is_empty() {
            return self.refuse(session, id, Answer::Conflict { conflicts });
        }
        for c in changes {
            if let Err(answer) = self.check_write(&c.path, &c.value) {
                return self.refuse(session, id, answer);
            }
        }
        let written = changes.iter().map(|c| (c.path.clone(), c.value.clone()));
        changed(self.write(written.collect()))
    }

    fn scene_list(&self) -> Vec<Value> {
        self.state["scenes"].as_array().cloned().unwrap_or_default()
    }

    /// The part of the state a scene stores: everything an operator sets
    /// on channels, buses, auxes, mix-minus outputs and VCAs, and the AFV
    /// bindings without the stage tally drives.
    fn scene_data(&self) -> Value {
        let mut data = Map::new();
        for group in ["channels", "buses", "auxes", "mixMinus", "vcas", "afv"] {
            data.insert(group.into(), self.state[group].clone());
        }
        if let Some(afv) = data.get_mut("afv").and_then(Value::as_object_mut) {
            for binding in afv.values_mut() {
                binding.as_object_mut().map(|b| b.remove("open"));
            }
        }
        Value::Object(data)
    }

    fn store_scene(
        &mut self,
        session: &SessionKey,
        id: &str,
        scene: Option<&str>,
        name: Option<&str>,
        at: &str,
    ) -> Effect<Edits> {
        let mut list = self.scene_list();
        let (scene, name) = match scene {
            Some(scene) => match self.scene(scene) {
                Some(record) => (
                    scene.to_owned(),
                    name.unwrap_or_else(|| record["name"].as_str().unwrap_or(scene))
                        .to_owned(),
                ),
                None => {
                    let message = format!("unknown scene {scene}");
                    return self.refuse(session, id, Answer::NotFound { message });
                }
            },
            None => {
                let scene = loop {
                    self.scenes_created += 1;
                    let scene = format!("scene-{}", self.scenes_created);
                    if !self.scenes.contains_key(&scene) {
                        break scene;
                    }
                };
                (scene, name.unwrap_or_default().to_owned())
            }
        };
        let record = json!({ "id": scene, "name": name, "storedAt": at });
        match list.iter_mut().find(|s| s["id"] == scene.as_str()) {
            Some(s) => *s = record,
            None => list.push(record),
        }
        self.scenes.insert(scene.clone(), self.scene_data());
        self.record(session, id, Answer::Scene { scene });
        changed(self.write(vec![("/scenes".into(), Value::Array(list))]))
    }

    fn delete_scene(&mut self, session: &SessionKey, id: &str, scene: &str) -> Effect<Edits> {
        if self.scenes.remove(scene).is_none() {
            let message = format!("unknown scene {scene}");
            return self.refuse(session, id, Answer::NotFound { message });
        }
        let mut list = self.scene_list();
        list.retain(|s| s["id"] != scene);
        let mut written = vec![("/scenes".to_owned(), Value::Array(list))];
        if self.state["sceneRecalled"]["id"] == scene {
            written.push(("/sceneRecalled".into(), Value::Null));
        }
        changed(self.write(written))
    }

    /// Recalls `scene`: every stored leaf whose recall-safe group its
    /// element does not keep, owned controls included, and
    /// `sceneRecalled`.
    fn recall(&mut self, session: &SessionKey, id: &str, scene: &str, at: &str) -> Effect<Edits> {
        let Some(data) = self.scenes.get(scene) else {
            let message = format!("unknown scene {scene}");
            return self.refuse(session, id, Answer::NotFound { message });
        };
        let mut written = Vec::new();
        for (group, elements) in data.as_object().into_iter().flatten() {
            for (element, value) in elements.as_object().into_iter().flatten() {
                let safe = &self.state["recallSafe"][element];
                for (path, v) in leaves(value, &format!("/{group}/{element}")) {
                    let Some(g) = recall_group(&path) else {
                        continue;
                    };
                    if safe.as_array().is_some_and(|s| s.iter().any(|x| x == g)) {
                        continue;
                    }
                    if self
                        .state
                        .pointer(&path)
                        .is_some_and(|current| *current != v)
                    {
                        written.push((path, v));
                    }
                }
            }
        }
        written.push(("/sceneRecalled".into(), json!({ "id": scene, "at": at })));
        changed(self.write(written))
    }

    fn tally(&mut self, sources: &BTreeMap<String, String>, replace: bool) -> Effect<Edits> {
        let mut all = sources.clone();
        if replace {
            for name in self.state["tally"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, _)| k)
            {
                all.entry(name.clone()).or_insert_with(|| "off".into());
            }
        }
        let written = all
            .into_iter()
            .map(|(name, state)| (format!("/tally/{}", escape(&name)), Value::String(state)))
            .collect();
        changed(self.write(written))
    }

    /// Loads `saved` into this state's configured shape: leaves the shape
    /// has no place for, or whose value it does not take, are dropped;
    /// tally, scenes and the scene recalled are kept as they are.
    fn reconcile(&mut self, saved: Saved) {
        let shape = self.shape.clone().expect("configured");
        let mut state = shape.initial_state();
        let mut dropped = Vec::new();
        for (path, value) in leaves(&saved.state, "") {
            let group = tokens(&path).into_iter().next().unwrap_or_default();
            if matches!(
                group.as_str(),
                "tally" | "scenes" | "sceneRecalled" | "consoles"
            ) {
                continue;
            }
            let fits = match state.pointer(&path) {
                _ if group == "recallSafe" => {
                    tokens(&path).get(1).is_some_and(|id| shape.recallable(id))
                }
                Some(old) => {
                    std::mem::discriminant(old) == std::mem::discriminant(&value)
                        && check(
                            &contract().state_change,
                            &json!({ "path": path, "value": value }),
                        )
                        .is_ok()
                        && self.check_references(&path, &value).is_ok()
                }
                None => false,
            };
            if fits {
                set_at(&mut state, &path, value);
            } else {
                dropped.push(path);
            }
        }
        for group in ["tally", "scenes", "sceneRecalled"] {
            state[group] = saved.state[group].clone();
        }
        self.state = state;
        self.owners = saved
            .owners
            .into_iter()
            .filter(|(p, _)| self.exists(p))
            .collect();
        self.dropped = dropped;
    }
}

impl Product for Mixer {
    type Change = Edits;

    fn apply(&mut self, origin: &Origin, command: &[u8]) -> Effect<Edits> {
        let Ok(command) = serde_json::from_slice::<Command>(command) else {
            return Effect::Unchanged;
        };
        let session = origin.key();
        match &command {
            Command::Control(c) => self.control(origin, c),
            Command::Patch { id, changes } => self.patch(&session, id, changes),
            Command::StoreScene {
                id,
                scene,
                name,
                at,
            } => self.store_scene(&session, id, scene.as_deref(), name.as_deref(), at),
            Command::DeleteScene { id, scene } => self.delete_scene(&session, id, scene),
            Command::Recall { id, scene, at } => self.recall(&session, id, scene, at),
            Command::Tally {
                sources, replace, ..
            } => self.tally(sources, *replace),
        }
    }

    /// Releases every control `session` holds.
    fn expire(&mut self, session: &SessionKey) -> Effect<Edits> {
        let held: Vec<String> = self
            .owners
            .iter()
            .filter(|(_, h)| h.key() == *session)
            .map(|(p, _)| p.clone())
            .collect();
        if held.is_empty() {
            return Effect::Unchanged;
        }
        for path in &held {
            self.owners.remove(path);
        }
        let off = self.momentary_off(&held);
        Effect::Changed(Edits {
            values: self.write(off),
            owners: held.into_iter().map(|p| (p, Value::Null)).collect(),
            origin: None,
        })
    }

    fn forget(&mut self, session: &SessionKey) {
        self.seqs.remove(session);
        self.answers.remove(session);
    }

    /// The delta without its `type`, `replica` and `revision`, which the
    /// sending replica adds ([`crate::ws::delta_message`]): every changed
    /// leaf and owner once, the later write winning, the entry's stamp as
    /// `atIndex` when a value changed, and the client commands among
    /// `origins`.
    fn delta(&self, stamp: u64, changes: &[Changed<'_, Edits>]) -> String {
        let mut values: Vec<&(String, Value)> = Vec::new();
        let mut owners: Vec<&(String, Value)> = Vec::new();
        let mut origins = Vec::new();
        for c in changes {
            for v in &c.change.values {
                values.retain(|(p, _)| *p != v.0);
                values.push(v);
            }
            for o in &c.change.owners {
                owners.retain(|(p, _)| *p != o.0);
                owners.push(o);
            }
            if c.command.is_some()
                && let Some(origin) = &c.change.origin
            {
                origins.push(origin.clone());
            }
        }
        let mut delta = Map::new();
        if !values.is_empty() {
            delta.insert("atIndex".into(), json!(stamp));
            let values = values.iter().map(|(p, v)| json!({ "path": p, "value": v }));
            delta.insert("changes".into(), values.collect());
        }
        if !owners.is_empty() {
            let owners = owners.iter().map(|(p, o)| json!({ "path": p, "owner": o }));
            delta.insert("owners".into(), owners.collect());
        }
        if !origins.is_empty() {
            delta.insert("origins".into(), Value::Array(origins));
        }
        Value::Object(delta).to_string()
    }

    /// `shape`, `state` and `owners`, which the snapshot message carries.
    fn text(&self) -> String {
        json!({ "shape": self.shape(), "state": self.state, "owners": self.owners() }).to_string()
    }

    fn save(&self) -> Vec<u8> {
        let saved = Saved {
            shape: self.shape().clone(),
            state: self.state.clone(),
            owners: self
                .owners
                .iter()
                .map(|(p, h)| (p.clone(), h.clone()))
                .collect(),
            seqs: self
                .seqs
                .iter()
                .map(|(k, s)| (k.clone(), s.iter().map(|(p, n)| (p.clone(), *n)).collect()))
                .collect(),
            answers: self
                .answers
                .iter()
                .map(|(k, a)| (k.clone(), a.iter().cloned().collect()))
                .collect(),
            scenes: self
                .scenes
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            scenes_created: self.scenes_created,
        };
        serde_json::to_vec(&saved).expect("the state serializes")
    }

    /// A state with a configured shape keeps it ([`Mixer::reconcile`]); a
    /// [`Default`] one, as a snapshot install builds, takes the saved one.
    fn load(&mut self, saved: &[u8]) -> Result<(), String> {
        let saved: Saved = serde_json::from_slice(saved).map_err(|e| e.to_string())?;
        self.seqs = saved
            .seqs
            .iter()
            .map(|(k, s)| (k.clone(), s.iter().cloned().collect()))
            .collect();
        self.answers = saved
            .answers
            .iter()
            .map(|(k, a)| (k.clone(), a.iter().cloned().collect()))
            .collect();
        self.scenes = saved.scenes.iter().cloned().collect();
        self.scenes_created = saved.scenes_created;
        if self.shape.is_some() {
            self.reconcile(saved);
        } else {
            self.shape = Some(Arc::new(saved.shape));
            self.state = saved.state;
            self.owners = saved.owners.into_iter().collect();
            self.dropped = Vec::new();
        }
        Ok(())
    }
}

/// A command's effect from the leaves it changed, none of them owners.
fn changed(values: Vec<(String, Value)>) -> Effect<Edits> {
    if values.is_empty() {
        Effect::Unchanged
    } else {
        Effect::Changed(Edits {
            values,
            ..Edits::default()
        })
    }
}

/// The reference tokens of a JSON pointer, unescaped.
pub fn tokens(path: &str) -> Vec<String> {
    path.split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect()
}

fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// Sets the leaf at `path`, creating its last key in an object.
fn set_at(root: &mut Value, path: &str, value: Value) {
    let tokens = tokens(path);
    let Some((last, parents)) = tokens.split_last() else {
        *root = value;
        return;
    };
    let mut at = root;
    for t in parents {
        at = match at {
            Value::Object(m) => m.entry(t.clone()).or_insert_with(|| json!({})),
            Value::Array(a) => match t.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
                Some(v) => v,
                None => return,
            },
            _ => return,
        };
    }
    match at {
        Value::Object(m) => {
            m.insert(last.clone(), value);
        }
        Value::Array(a) => {
            if let Some(slot) = last.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
                *slot = value;
            }
        }
        _ => {}
    }
}

/// The leaves under `value`: an array of strings (an id set or recall
/// groups) is one leaf, other arrays split into one leaf per leg.
fn leaves(value: &Value, base: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    collect(value, base, &mut out);
    out
}

fn collect(value: &Value, base: &str, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Array(a) if a.iter().all(Value::is_string) => {
            out.push((base.to_owned(), value.clone()))
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                collect(v, &format!("{base}/{i}"), out);
            }
        }
        Value::Object(m) => {
            for (k, v) in m {
                collect(v, &format!("{base}/{}", escape(k)), out);
            }
        }
        _ => out.push((base.to_owned(), value.clone())),
    }
}

/// The recall-safe group a leaf of a scene belongs to; `None` for leaves
/// a recall never touches.
fn recall_group(path: &str) -> Option<&'static str> {
    let tokens = tokens(path);
    let group = tokens.first()?.as_str();
    let key = tokens.get(2).map(String::as_str);
    match group {
        "afv" => (key != Some("open")).then_some("afv"),
        "mixMinus" => match key? {
            "exclude" => Some("routing"),
            "interrupt" => None,
            "fader" => Some("fader"),
            "mute" => Some("mute"),
            _ => None,
        },
        "vcas" => match key? {
            "offset" => Some("fader"),
            "members" => Some("routing"),
            "solo" => None,
            _ => Some("mute"),
        },
        _ => match key? {
            "input" | "trim" | "legTrim" | "polarity" | "hpf" | "delay" => Some("input"),
            "eq" => Some("eq"),
            "gate" | "comp" | "dynamicsBeforeEq" | "limiter" => Some("dynamics"),
            "pan" | "image" => Some("pan"),
            "sends" => Some("sends"),
            "routes" | "directOut" => Some("routing"),
            "fader" => Some("fader"),
            "mute" => Some("mute"),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::tests::small;
    use samplez_replica::entry::Command as Entry;
    use samplez_service::Role;

    fn mixer() -> Mixer {
        Mixer::new(Shape::parse(&small()).unwrap())
    }

    fn origin(session: &str) -> Origin {
        Origin {
            principal: "anonymous".into(),
            session: session.into(),
            role: Role::Operator,
            connection: format!("r1:{session}:1"),
        }
    }

    fn control(action: Action, id: &str, seq: u64, path: &str, value: Option<Value>) -> Vec<u8> {
        serde_json::to_vec(&Command::Control(Control {
            id: id.into(),
            action,
            kind: "ui".into(),
            seq,
            path: path.into(),
            value,
        }))
        .unwrap()
    }

    fn set(m: &mut Mixer, session: &str, seq: u64, path: &str, value: Value) -> Effect<Edits> {
        let id = format!("{session}-{seq}");
        m.apply(
            &origin(session),
            &control(Action::Set, &id, seq, path, Some(value)),
        )
    }

    fn touch(m: &mut Mixer, session: &str, seq: u64, path: &str) -> Effect<Edits> {
        let id = format!("{session}-t{seq}");
        m.apply(
            &origin(session),
            &control(Action::Touch, &id, seq, path, None),
        )
    }

    fn release(m: &mut Mixer, session: &str, seq: u64, path: &str) -> Effect<Edits> {
        let id = format!("{session}-r{seq}");
        m.apply(
            &origin(session),
            &control(Action::Release, &id, seq, path, None),
        )
    }

    fn rest(m: &mut Mixer, command: Command) -> Effect<Edits> {
        m.apply(&origin("rest"), &serde_json::to_vec(&command).unwrap())
    }

    fn patch(m: &mut Mixer, id: &str, changes: &[(&str, Value)]) -> Effect<Edits> {
        let changes = changes
            .iter()
            .map(|(p, v)| Write {
                path: (*p).into(),
                value: v.clone(),
            })
            .collect();
        rest(
            m,
            Command::Patch {
                id: id.into(),
                changes,
            },
        )
    }

    fn values(effect: &Effect<Edits>) -> Vec<(String, Value)> {
        match effect {
            Effect::Changed(e) => e.values.clone(),
            other => panic!("not changed: {other:?}"),
        }
    }

    fn at<'a>(m: &'a Mixer, path: &str) -> &'a Value {
        m.state().pointer(path).unwrap()
    }

    fn rest_key() -> SessionKey {
        origin("rest").key()
    }

    #[test]
    fn a_set_changes_its_leaf_and_names_its_origin() {
        let mut m = mixer();
        let effect = set(&mut m, "a", 1, "/channels/ch1/fader", json!(-3));
        let Effect::Changed(edits) = &effect else {
            panic!("{effect:?}")
        };
        assert_eq!(
            edits.values,
            vec![("/channels/ch1/fader".into(), json!(-3))]
        );
        let origin = edits.origin.as_ref().unwrap();
        assert_eq!(origin["commandId"], "a-1");
        assert_eq!(origin["seq"], 1);
        assert_eq!(origin["owner"]["handle"], OwnerHandle::of("a").as_str());
        assert_eq!(*at(&m, "/channels/ch1/fader"), json!(-3));
        assert_eq!(
            set(&mut m, "a", 2, "/channels/ch1/fader", json!(-3)),
            Effect::Unchanged
        );
    }

    /// A copy, or a command resent after a later one on the same control
    /// applied, cannot undo the later one; other controls count apart.
    #[test]
    fn a_seq_not_above_the_last_of_its_session_and_control_is_stale() {
        let mut m = mixer();
        set(&mut m, "a", 5, "/channels/ch1/fader", json!(-3));
        assert_eq!(
            set(&mut m, "a", 4, "/channels/ch1/fader", json!(-9)),
            Effect::Stale
        );
        assert_eq!(
            set(&mut m, "a", 5, "/channels/ch1/fader", json!(-9)),
            Effect::Stale
        );
        assert_eq!(touch(&mut m, "a", 5, "/channels/ch1/fader"), Effect::Stale);
        assert!(matches!(
            set(&mut m, "a", 1, "/channels/ch2/fader", json!(-9)),
            Effect::Changed(_)
        ));
        assert!(matches!(
            set(&mut m, "b", 1, "/channels/ch1/fader", json!(-9)),
            Effect::Changed(_)
        ));
        assert_eq!(*at(&m, "/channels/ch1/fader"), json!(-9));
    }

    /// Ownership: another session's write and touch are refused with the
    /// current value and the owner, without moving its seq; a release by
    /// another session changes nothing; the owner releases.
    #[test]
    fn a_touched_control_refuses_other_sessions_until_released() {
        let mut m = mixer();
        let path = "/channels/ch1/fader";
        let Effect::Changed(t) = touch(&mut m, "a", 1, path) else {
            panic!()
        };
        assert_eq!(t.owners[0].1["handle"], OwnerHandle::of("a").as_str());
        assert_eq!(set(&mut m, "b", 1, path, json!(0)), Effect::Unchanged);
        let Some(Answer::Conflict { conflicts }) = m.answer(&origin("b").key(), "b-1") else {
            panic!("{:?}", m.answer(&origin("b").key(), "b-1"))
        };
        assert_eq!(conflicts[0]["current"], json!(-90));
        assert_eq!(
            conflicts[0]["owner"]["handle"],
            OwnerHandle::of("a").as_str()
        );
        // The refused set left b's seq alone: a release at seq 1 is not
        // stale, and changes nothing as b holds nothing.
        assert_eq!(release(&mut m, "b", 1, path), Effect::Unchanged);
        assert_eq!(touch(&mut m, "b", 2, path), Effect::Unchanged);
        assert!(matches!(
            m.answer(&origin("b").key(), "b-t2"),
            Some(Answer::Conflict { .. })
        ));
        assert!(m.owners().get(path).is_some());
        assert!(matches!(
            set(&mut m, "a", 2, path, json!(-1)),
            Effect::Changed(_)
        ));
        let Effect::Changed(r) = release(&mut m, "a", 3, path) else {
            panic!()
        };
        assert_eq!(r.owners, vec![(path.to_owned(), Value::Null)]);
        assert!(matches!(
            set(&mut m, "b", 3, path, json!(0)),
            Effect::Changed(_)
        ));
    }

    #[test]
    fn a_lease_expiry_releases_the_sessions_controls_and_ends_a_talk() {
        let mut m = mixer();
        let talk = "/mixMinus/mm1/interrupt/active";
        touch(&mut m, "a", 1, talk);
        set(&mut m, "a", 2, talk, json!(true));
        touch(&mut m, "a", 1, "/channels/ch1/fader");
        touch(&mut m, "b", 1, "/channels/ch2/fader");
        let Effect::Changed(e) = m.expire(&origin("a").key()) else {
            panic!()
        };
        assert_eq!(e.values, vec![(talk.to_owned(), json!(false))]);
        assert_eq!(e.owners.len(), 2);
        assert!(e.origin.is_none());
        assert_eq!(
            m.owners().as_object().unwrap().keys().collect::<Vec<_>>(),
            ["/channels/ch2/fader"]
        );
        assert_eq!(m.expire(&origin("a").key()), Effect::Unchanged);
    }

    /// Seqs live until the session is forgotten; a copy arriving after
    /// that is a new command.
    #[test]
    fn forget_drops_the_seqs_of_a_session() {
        let mut m = mixer();
        set(&mut m, "a", 5, "/channels/ch1/fader", json!(-3));
        m.forget(&origin("a").key());
        assert!(matches!(
            set(&mut m, "a", 4, "/channels/ch1/fader", json!(-9)),
            Effect::Changed(_)
        ));
    }

    #[test]
    fn a_patch_touching_an_owned_control_is_refused_whole() {
        let mut m = mixer();
        touch(&mut m, "a", 1, "/channels/ch1/fader");
        let effect = patch(
            &mut m,
            "p1",
            &[
                ("/channels/ch2/fader", json!(0)),
                ("/channels/ch1/fader", json!(0)),
            ],
        );
        assert_eq!(effect, Effect::Unchanged);
        let Some(Answer::Conflict { conflicts }) = m.answer(&rest_key(), "p1") else {
            panic!()
        };
        assert_eq!(conflicts.len(), 1);
        assert_eq!(
            *at(&m, "/channels/ch2/fader"),
            json!(-90),
            "nothing applied"
        );
    }

    /// The order-independent checks refuse before anything changes.
    #[test]
    fn writes_the_shape_cannot_take_are_refused() {
        let m = mixer();
        let refused = |path: &str, value: Value| m.check_write(path, &value).unwrap_err();
        assert!(matches!(
            refused("/channels/ch9/fader", json!(0)),
            Answer::NotFound { .. }
        ));
        assert!(matches!(
            refused("/channels/ch1/legTrim/1", json!(0)),
            Answer::NotFound { .. }
        ));
        assert!(matches!(
            refused("/recallSafe/mon", json!([])),
            Answer::NotFound { .. }
        ));
        assert!(matches!(
            refused("/channels/ch1/fader", json!(11)),
            Answer::Invalid { .. }
        ));
        assert!(matches!(
            refused("/channels/ch2/input", json!("b")),
            Answer::Invalid { .. }
        ));
        assert!(matches!(
            refused("/vcas/vca1/members", json!(["ch1", "pgm"])),
            Answer::Invalid { .. }
        ));
        assert!(matches!(
            refused("/tone/targets", json!(["ch1"])),
            Answer::Invalid { .. }
        ));
        assert!(matches!(
            refused("/monitor/source", json!("ch1")),
            Answer::Invalid { .. }
        ));
        m.check_write("/channels/ch1/input", &json!("b")).unwrap();
        m.check_write("/recallSafe/vca1", &json!(["fader"]))
            .unwrap();
        m.check_write("/monitor/source", &json!("aux1")).unwrap();
    }

    #[test]
    fn a_loudness_preset_sets_target_and_tolerance_and_a_target_makes_it_custom() {
        let mut m = mixer();
        let v = values(&patch(
            &mut m,
            "p1",
            &[("/metering/loudness/preset", json!("atsc"))],
        ));
        assert!(v.contains(&("/metering/loudness/target".into(), json!(-24))));
        assert!(v.contains(&("/metering/loudness/tolerance".into(), json!(2))));
        let v = values(&patch(
            &mut m,
            "p2",
            &[("/metering/loudness/target", json!(-20))],
        ));
        assert!(v.contains(&("/metering/loudness/preset".into(), json!("custom"))));
    }

    #[test]
    fn the_afv_stage_follows_the_tally_of_its_source() {
        let mut m = mixer();
        patch(
            &mut m,
            "p1",
            &[
                ("/afv/ch1/enabled", json!(true)),
                ("/afv/ch1/source", json!("CAM1")),
            ],
        );
        let tally = |m: &mut Mixer, state: &str, replace| {
            let sources = BTreeMap::from([("CAM1".to_owned(), state.to_owned())]);
            rest(
                m,
                Command::Tally {
                    id: "t".into(),
                    sources,
                    replace,
                },
            )
        };
        let v = values(&tally(&mut m, "preview", false));
        assert!(v.contains(&("/afv/ch1/open".into(), json!(false))), "{v:?}");
        let v = values(&tally(&mut m, "program", false));
        assert!(v.contains(&("/afv/ch1/open".into(), json!(true))));
        patch(&mut m, "p2", &[("/afv/ch2/source", json!("CAM2"))]);
        let sources = BTreeMap::from([("CAM2".to_owned(), "program".to_owned())]);
        let v = values(&rest(
            &mut m,
            Command::Tally {
                id: "t".into(),
                sources,
                replace: true,
            },
        ));
        assert!(
            v.contains(&("/tally/CAM1".into(), json!("off"))),
            "unlisted sources go off: {v:?}"
        );
        assert_eq!(*at(&m, "/afv/ch1/open"), json!(false));
    }

    fn store(m: &mut Mixer, id: &str, scene: Option<&str>, name: &str) -> String {
        rest(
            m,
            Command::StoreScene {
                id: id.into(),
                scene: scene.map(Into::into),
                name: Some(name.into()),
                at: "2026-11-15T12:00:00.000Z".into(),
            },
        );
        match m.answer(&rest_key(), id) {
            Some(Answer::Scene { scene }) => scene.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// A recall applies every stored leaf but those of recall-safe groups
    /// and the AFV stage, owned controls included, and names the scene.
    #[test]
    fn a_recall_restores_the_scene_but_its_recall_safe_groups() {
        let mut m = mixer();
        let scene = store(&mut m, "s1", None, "Opening");
        assert_eq!(scene, "scene-1");
        patch(
            &mut m,
            "p1",
            &[
                ("/channels/ch1/fader", json!(-6)),
                ("/channels/ch1/mute", json!(true)),
                ("/channels/ch2/fader", json!(-6)),
                ("/recallSafe/ch2", json!(["fader"])),
            ],
        );
        touch(&mut m, "a", 1, "/channels/ch1/fader");
        let at_ = "2026-11-15T12:01:00.000Z";
        let v = values(&rest(
            &mut m,
            Command::Recall {
                id: "r1".into(),
                scene: scene.clone(),
                at: at_.into(),
            },
        ));
        assert!(v.contains(&("/channels/ch1/fader".into(), json!(-90))));
        assert!(v.contains(&("/channels/ch1/mute".into(), json!(false))));
        assert!(
            !v.iter().any(|(p, _)| p == "/channels/ch2/fader"),
            "recall-safe"
        );
        assert_eq!(*at(&m, "/sceneRecalled"), json!({ "id": scene, "at": at_ }));
        assert_eq!(
            rest(
                &mut m,
                Command::Recall {
                    id: "r2".into(),
                    scene: "nope".into(),
                    at: at_.into()
                }
            ),
            Effect::Unchanged
        );
        assert!(matches!(
            m.answer(&rest_key(), "r2"),
            Some(Answer::NotFound { .. })
        ));
    }

    #[test]
    fn scenes_are_stored_renamed_and_deleted() {
        let mut m = mixer();
        let s1 = store(&mut m, "c1", None, "Opening");
        let s2 = store(&mut m, "c2", None, "Interview");
        assert_ne!(s1, s2);
        store(&mut m, "c3", Some(&s1), "Open");
        assert_eq!(m.scene(&s1).unwrap()["name"], "Open");
        rest(
            &mut m,
            Command::Recall {
                id: "r".into(),
                scene: s1.clone(),
                at: "2026-11-15T12:00:00.000Z".into(),
            },
        );
        let v = values(&rest(
            &mut m,
            Command::DeleteScene {
                id: "d".into(),
                scene: s1.clone(),
            },
        ));
        assert!(v.contains(&("/sceneRecalled".into(), Value::Null)));
        assert_eq!(at(&m, "/scenes").as_array().unwrap().len(), 1);
        assert_eq!(
            rest(
                &mut m,
                Command::DeleteScene {
                    id: "d2".into(),
                    scene: s1
                }
            ),
            Effect::Unchanged
        );
    }

    /// One delta per entry: a leaf written twice once, the later value;
    /// every client command among the origins; `atIndex` only when a
    /// value changed.
    #[test]
    fn a_delta_merges_its_entrys_changes() {
        let mut m = mixer();
        let commands: Vec<Entry> = ["a", "b"]
            .iter()
            .map(|s| Entry {
                origin: origin(s),
                id: format!("{s}-1"),
                at_index: None,
                body: Vec::new(),
            })
            .collect();
        let e1 = values(&set(&mut m, "a", 1, "/channels/ch1/fader", json!(-3)));
        let e2 = values(&set(&mut m, "b", 1, "/channels/ch1/fader", json!(-4)));
        let o = |c: &str| Some(json!({ "commandId": c }));
        let a = Edits {
            values: e1,
            origin: o("a-1"),
            ..Edits::default()
        };
        let b = Edits {
            values: e2,
            origin: o("b-1"),
            ..Edits::default()
        };
        let changes = [
            Changed {
                command: Some(&commands[0]),
                change: &a,
            },
            Changed {
                command: Some(&commands[1]),
                change: &b,
            },
        ];
        let delta: Value = serde_json::from_str(&m.delta(4800, &changes)).unwrap();
        assert_eq!(
            delta["changes"],
            json!([{ "path": "/channels/ch1/fader", "value": -4 }])
        );
        assert_eq!(delta["atIndex"], 4800);
        assert_eq!(delta["origins"].as_array().unwrap().len(), 2);
        let owned = Edits {
            owners: vec![("/channels/ch1/fader".into(), Value::Null)],
            ..Edits::default()
        };
        let delta: Value = serde_json::from_str(&m.delta(
            9600,
            &[Changed {
                command: None,
                change: &owned,
            }],
        ))
        .unwrap();
        assert!(
            delta.get("atIndex").is_none() && delta.get("origins").is_none(),
            "{delta}"
        );
    }

    #[test]
    fn a_saved_state_loads_back_whole() {
        let mut m = mixer();
        set(&mut m, "a", 3, "/channels/ch1/fader", json!(-3));
        touch(&mut m, "a", 4, "/channels/ch2/fader");
        store(&mut m, "c1", None, "Opening");
        let saved = m.save();
        let mut back = mixer();
        back.load(&saved).unwrap();
        assert_eq!(back.text(), m.text());
        assert!(back.dropped().is_empty());
        assert_eq!(
            set(&mut back, "a", 3, "/channels/ch1/fader", json!(0)),
            Effect::Stale,
            "seqs kept"
        );
        assert!(back.has_scene("scene-1"));
        let mut installed = Mixer::default();
        installed.load(&saved).unwrap();
        assert_eq!(
            installed.text(),
            m.text(),
            "a snapshot install takes the saved shape"
        );
    }

    /// The configured shape wins: leaves of a channel it lost, a leg it
    /// lost and ids it no longer has are dropped and reported.
    #[test]
    fn a_saved_state_loads_into_a_smaller_shape() {
        let mut m = mixer();
        patch(
            &mut m,
            "p",
            &[
                ("/channels/ch2/fader", json!(-3)),
                ("/channels/st1/legTrim/1", json!(2)),
                ("/channels/ch1/fader", json!(-1)),
                ("/vcas/vca1/members", json!(["ch1", "ch2"])),
            ],
        );
        let mut smaller = small();
        smaller["channels"].as_array_mut().unwrap().remove(1);
        smaller["channels"][1]["width"] = json!(1);
        smaller["channels"][1]["layout"] = json!("mono");
        let mut back = Mixer::new(Shape::parse(&smaller).unwrap());
        back.load(&m.save()).unwrap();
        assert_eq!(*at(&back, "/channels/ch1/fader"), json!(-1));
        assert!(back.state().pointer("/channels/ch2").is_none());
        let dropped = back.dropped();
        assert!(
            dropped.contains(&"/channels/ch2/fader".to_owned()),
            "{dropped:?}"
        );
        assert!(
            dropped.contains(&"/channels/st1/legTrim/1".to_owned()),
            "{dropped:?}"
        );
        assert!(
            dropped.contains(&"/vcas/vca1/members".to_owned()),
            "{dropped:?}"
        );
        check(&contract().state, back.state()).unwrap();
    }
}
