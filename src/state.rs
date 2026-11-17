//! The mixer's replicated state: the live state of model.schema.json over
//! the configured shape, touch ownership, the last seq per session and
//! control, scenes and consoles. Every replica applies the same commands
//! in log order through [`Product`], so all of it is decided here and
//! nowhere else.

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
    /// A WebSocket `console`: registers or updates `console` as the
    /// origin session's.
    Register {
        id: String,
        console: Console,
        bank_offset: u64,
    },
    /// `PUT /consoles/{id}`: creates or updates the descriptor.
    PutConsole {
        id: String,
        console: ConsolePut,
    },
    /// `DELETE /consoles/{id}`.
    RemoveConsole {
        id: String,
        console: String,
    },
}

/// A console's descriptor: model `ConsoleStatus` without what the server
/// keeps (`handle`, `connected`, `bankOffset`). Also an entry of the
/// configuration's `consoles`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Console {
    pub id: String,
    pub name: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default = "default_strips")]
    pub strips: u64,
}

pub(crate) fn default_protocol() -> String {
    "mcu".into()
}

pub(crate) fn default_transport() -> String {
    "bridge".into()
}

pub(crate) fn default_strips() -> u64 {
    8
}

impl Console {
    /// Model `ConsoleStatus` of this descriptor.
    pub fn status(&self, session: Option<&SessionKey>, connected: bool, bank_offset: u64) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "handle": session.map(|s| OwnerHandle::of(&s.session).as_str().to_owned()),
            "protocol": self.protocol,
            "transport": self.transport,
            "connected": connected,
            "bankOffset": bank_offset,
            "strips": self.strips,
        })
    }
}

/// What `PUT /consoles/{id}` sets: a field left out keeps its value, or
/// takes its default on a new console.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsolePut {
    pub id: String,
    pub name: String,
    pub protocol: Option<String>,
    pub transport: Option<String>,
    pub strips: Option<u64>,
}

/// A console the state holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registered {
    console: Console,
    bank_offset: u64,
    connected: bool,
    /// From the configuration: never removed, named by it.
    declared: bool,
    /// The session that registered it last. Kept in the replicated state,
    /// never published: the status carries its handle.
    session: Option<SessionKey>,
}

impl Registered {
    fn public(&self) -> Value {
        self.console
            .status(self.session.as_ref(), self.connected, self.bank_offset)
    }
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
    /// 409 with a message alone: a console connected to another session,
    /// or a declared one to remove.
    Refused { message: String },
    /// The console a `PutConsole` created.
    Created,
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

/// Answers kept per session, the newest: as many as samplez-replica's
/// dedup table keeps command ids of one session. Only some commands get
/// one, so every command id the table still answers as a duplicate
/// finds its answer; the table also retires ids by age and by its total,
/// which only shortens what it holds.
fn answers_kept() -> usize {
    samplez_replica::dedup::Limits::defaults(crate::replica::RATE).max_session_commands
}

/// Consoles registered at runtime the state holds at most, besides the
/// declared ones.
pub const MAX_CONSOLES: usize = 64;

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
    /// Declared consoles first, in configuration order, then the others in
    /// the order they were created; `/consoles` publishes them so.
    consoles: Vec<Registered>,
    /// The configuration's consoles, which a loaded state is merged with.
    declared: Vec<Console>,
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
    /// Absent in snapshots taken before consoles were kept.
    #[serde(default)]
    consoles: Vec<Registered>,
}

impl Mixer {
    pub fn new(shape: Shape) -> Mixer {
        Mixer::with_consoles(shape, Vec::new())
    }

    /// A state with the `declared` consoles of the configuration, not
    /// connected yet.
    pub fn with_consoles(shape: Shape, declared: Vec<Console>) -> Mixer {
        let mut m = Mixer {
            state: shape.initial_state(),
            shape: Some(Arc::new(shape)),
            consoles: declared.iter().map(Mixer::declared_entry).collect(),
            declared,
            ..Mixer::default()
        };
        m.state["consoles"] = m.console_list();
        m
    }

    fn declared_entry(console: &Console) -> Registered {
        Registered {
            console: console.clone(),
            bank_offset: 0,
            connected: false,
            declared: true,
            session: None,
        }
    }

    /// Model `ConsoleStatus` of every console, in published order.
    fn console_list(&self) -> Value {
        self.consoles.iter().map(Registered::public).collect()
    }

    /// Model `ConsoleStatus` of console `id`.
    pub fn console(&self, id: &str) -> Option<Value> {
        self.consoles
            .iter()
            .find(|r| r.console.id == id)
            .map(Registered::public)
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

    /// The session that owns `path`, when one does.
    pub fn owner_of(&self, path: &str) -> Option<SessionKey> {
        self.owners.get(path).map(Holder::key)
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
        if kept.len() >= answers_kept() {
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
        let mut changed: Vec<(String, Value)> = Vec::new();
        for (path, value) in all {
            if self.state.pointer(&path) != Some(&value) {
                set_at(&mut self.state, &path, value.clone());
                changed.retain(|(p, _)| *p != path);
                changed.push((path, value));
            }
        }
        if afv {
            for (channel, open) in self.afv_open() {
                if self.state["afv"][&channel]["open"] != open {
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
                    // A scene keeps the ids of the shape it was stored in.
                    if self
                        .state
                        .pointer(&path)
                        .is_some_and(|current| *current != v)
                        && self.check_references(&path, &v).is_ok()
                    {
                        written.push((path, v));
                    }
                }
            }
        }
        written.push(("/sceneRecalled".into(), json!({ "id": scene, "at": at })));
        changed(self.write(written))
    }

    fn tally(
        &mut self,
        session: &SessionKey,
        id: &str,
        sources: &BTreeMap<String, String>,
        replace: bool,
    ) -> Effect<Edits> {
        let known = self.state["tally"].as_object().map_or(0, Map::len);
        let new = sources
            .keys()
            .filter(|s| self.state["tally"].get(s.as_str()).is_none());
        if known + new.count() > MAX_TALLY_SOURCES {
            let message = format!("at most {MAX_TALLY_SOURCES} tally sources");
            return self.refuse(session, id, Answer::Invalid { message });
        }
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

    /// `/consoles` written from the consoles held.
    fn publish_consoles(&mut self) -> Effect<Edits> {
        let list = self.console_list();
        changed(self.write(vec![("/consoles".into(), list)]))
    }

    fn position(&self, console: &str) -> Option<usize> {
        self.consoles.iter().position(|r| r.console.id == console)
    }

    /// Refuses a console beyond [`MAX_CONSOLES`] not declared.
    fn room_for_console(&self) -> Result<(), Answer> {
        if self.consoles.iter().filter(|r| !r.declared).count() >= MAX_CONSOLES {
            return Err(Answer::Invalid {
                message: format!("at most {MAX_CONSOLES} consoles besides the declared ones"),
            });
        }
        Ok(())
    }

    /// Registers `console` as `origin`'s session's, connected, at
    /// `bank_offset`. Refused while it is connected to another session. A
    /// declared console keeps the name of the configuration.
    fn register(
        &mut self,
        origin: &Origin,
        id: &str,
        console: &Console,
        bank_offset: u64,
    ) -> Effect<Edits> {
        let session = origin.key();
        match self.position(&console.id) {
            Some(i) => {
                let r = &self.consoles[i];
                if r.connected && r.session.as_ref().is_some_and(|s| *s != session) {
                    let message = format!("console {} is connected to another session", console.id);
                    return self.refuse(&session, id, Answer::Refused { message });
                }
                let r = &mut self.consoles[i];
                let name = if r.declared {
                    r.console.name.clone()
                } else {
                    console.name.clone()
                };
                r.console = Console {
                    name,
                    ..console.clone()
                };
                r.bank_offset = bank_offset;
                r.connected = true;
                r.session = Some(session);
            }
            None => {
                if let Err(answer) = self.room_for_console() {
                    return self.refuse(&session, id, answer);
                }
                self.consoles.push(Registered {
                    console: console.clone(),
                    bank_offset,
                    connected: true,
                    declared: false,
                    session: Some(session),
                });
            }
        }
        self.publish_consoles()
    }

    /// Creates or updates the descriptor of a console, keeping who it is
    /// connected to and its bank. A declared console keeps the name of the
    /// configuration.
    fn put_console(&mut self, session: &SessionKey, id: &str, put: &ConsolePut) -> Effect<Edits> {
        let i = match self.position(&put.id) {
            Some(i) => i,
            None => {
                if let Err(answer) = self.room_for_console() {
                    return self.refuse(session, id, answer);
                }
                self.consoles.push(Registered {
                    console: Console {
                        id: put.id.clone(),
                        name: put.name.clone(),
                        protocol: default_protocol(),
                        transport: default_transport(),
                        strips: default_strips(),
                    },
                    bank_offset: 0,
                    connected: false,
                    declared: false,
                    session: None,
                });
                self.record(session, id, Answer::Created);
                self.consoles.len() - 1
            }
        };
        let c = &mut self.consoles[i];
        if !c.declared {
            c.console.name = put.name.clone();
        }
        if let Some(p) = &put.protocol {
            c.console.protocol = p.clone();
        }
        if let Some(t) = &put.transport {
            c.console.transport = t.clone();
        }
        if let Some(n) = put.strips {
            c.console.strips = n;
        }
        self.publish_consoles()
    }

    fn remove_console(&mut self, session: &SessionKey, id: &str, console: &str) -> Effect<Edits> {
        let Some(i) = self.position(console) else {
            let message = format!("unknown console {console}");
            return self.refuse(session, id, Answer::NotFound { message });
        };
        if self.consoles[i].declared {
            let message = format!("console {console} is declared in the configuration");
            return self.refuse(session, id, Answer::Refused { message });
        }
        self.consoles.remove(i);
        self.publish_consoles()
    }

    /// The consoles of a loaded state merged with the declared ones: every
    /// declared console, in configuration order and named by it, with the
    /// session, connection and bank it had and, once registered, what the
    /// registration said; then the saved consoles that were never
    /// declared. One declared before and no longer is dropped.
    fn merge_consoles(&mut self, saved: Vec<Registered>) {
        let mut merged: Vec<Registered> = self
            .declared
            .iter()
            .map(|d| match saved.iter().find(|s| s.console.id == d.id) {
                Some(s) => {
                    let console = if s.session.is_some() {
                        Console {
                            name: d.name.clone(),
                            ..s.console.clone()
                        }
                    } else {
                        d.clone()
                    };
                    Registered {
                        console,
                        declared: true,
                        ..s.clone()
                    }
                }
                None => Mixer::declared_entry(d),
            })
            .collect();
        let runtime = saved
            .into_iter()
            .filter(|s| !s.declared && !self.declared.iter().any(|d| d.id == s.console.id));
        merged.extend(runtime);
        self.consoles = merged;
        self.state["consoles"] = self.console_list();
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
                // The schema types the value by its path, a nullable one
                // included.
                Some(_) => {
                    check(
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
        self.merge_consoles(saved.consoles);
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
                id,
                sources,
                replace,
            } => self.tally(&session, id, sources, *replace),
            Command::Register {
                id,
                console,
                bank_offset,
            } => self.register(origin, id, console, *bank_offset),
            Command::PutConsole { id, console } => self.put_console(&session, id, console),
            Command::RemoveConsole { id, console } => self.remove_console(&session, id, console),
        }
    }

    /// Releases every control `session` holds and disconnects its
    /// consoles.
    fn expire(&mut self, session: &SessionKey) -> Effect<Edits> {
        let held: Vec<String> = self
            .owners
            .iter()
            .filter(|(_, h)| h.key() == *session)
            .map(|(p, _)| p.clone())
            .collect();
        let mut consoles = false;
        for r in &mut self.consoles {
            if r.connected && r.session.as_ref() == Some(session) {
                r.connected = false;
                consoles = true;
            }
        }
        if held.is_empty() && !consoles {
            return Effect::Unchanged;
        }
        for path in &held {
            self.owners.remove(path);
        }
        let mut written = self.momentary_off(&held);
        if consoles {
            written.push(("/consoles".into(), self.console_list()));
        }
        Effect::Changed(Edits {
            values: self.write(written),
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
            consoles: self.consoles.clone(),
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
            self.consoles = saved.consoles;
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
pub(crate) fn set_at(root: &mut Value, path: &str, value: Value) {
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
    fn a_loudness_measurement_starts_and_pauses() {
        let mut m = mixer();
        let v = values(&patch(
            &mut m,
            "p",
            &[("/loudness/pgm/running", json!(false))],
        ));
        assert_eq!(v, vec![("/loudness/pgm/running".to_owned(), json!(false))]);
        assert!(matches!(
            m.check_write("/loudness/ch1/running", &json!(true)),
            Err(Answer::NotFound { .. })
        ));
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
        touch(&mut m, "a", 1, "/channels/ch2/fader");
        touch(&mut m, "a", 1, "/channels/ch1/fader");
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
        let owners = back.owners();
        let owned: Vec<&String> = owners.as_object().unwrap().keys().collect();
        assert_eq!(
            owned,
            ["/channels/ch1/fader"],
            "no owner of a control the shape lost"
        );
    }

    /// Controls whose initial value is null load back with what was set.
    #[test]
    fn nullable_controls_load_back() {
        let mut m = mixer();
        patch(
            &mut m,
            "p",
            &[
                ("/afv/ch1/enabled", json!(true)),
                ("/afv/ch1/source", json!("CAM1")),
                ("/mixMinus/mm1/interrupt/source", json!("ch2")),
            ],
        );
        let mut back = mixer();
        back.load(&m.save()).unwrap();
        assert!(back.dropped().is_empty(), "{:?}", back.dropped());
        assert_eq!(*at(&back, "/afv/ch1/source"), json!("CAM1"));
        assert_eq!(*at(&back, "/mixMinus/mm1/interrupt/source"), json!("ch2"));
    }

    #[test]
    fn forget_drops_the_answers_of_a_session() {
        let mut m = mixer();
        touch(&mut m, "a", 1, "/channels/ch1/fader");
        set(&mut m, "b", 1, "/channels/ch1/fader", json!(0));
        assert!(m.answer(&origin("b").key(), "b-1").is_some());
        m.forget(&origin("b").key());
        assert!(m.answer(&origin("b").key(), "b-1").is_none());
    }

    /// A set refused for its value leaves the seq of its control alone.
    #[test]
    fn a_refused_set_does_not_take_its_seq() {
        let mut m = mixer();
        assert_eq!(
            set(&mut m, "a", 5, "/channels/ch1/fader", json!(99)),
            Effect::Unchanged
        );
        assert!(matches!(
            m.answer(&origin("a").key(), "a-5"),
            Some(Answer::Invalid { .. })
        ));
        assert!(matches!(
            set(&mut m, "a", 5, "/channels/ch1/fader", json!(-1)),
            Effect::Changed(_)
        ));
    }

    /// A recall writes no id the current shape lacks.
    #[test]
    fn a_recall_skips_ids_the_shape_lacks() {
        let mut m = mixer();
        let scene = store(&mut m, "s1", None, "Opening");
        m.scenes.get_mut(&scene).unwrap()["vcas"]["vca1"]["members"] = json!(["ch1", "gone"]);
        m.scenes.get_mut(&scene).unwrap()["vcas"]["vca1"]["offset"] = json!(-3);
        let at_ = "2026-11-15T12:01:00.000Z".to_owned();
        let v = values(&rest(
            &mut m,
            Command::Recall {
                id: "r".into(),
                scene,
                at: at_,
            },
        ));
        assert!(v.contains(&("/vcas/vca1/offset".into(), json!(-3))));
        assert_eq!(*at(&m, "/vcas/vca1/members"), json!([]));
    }

    /// The stage follows bindings and tally whatever it reads now, as the
    /// mock does.
    #[test]
    fn a_tally_reasserts_a_stage_that_contradicts_it() {
        let mut m = mixer();
        set_at(&mut m.state, "/afv/ch1/open", json!(false));
        let sources = BTreeMap::from([("CAM9".to_owned(), "program".to_owned())]);
        let v = values(&rest(
            &mut m,
            Command::Tally {
                id: "t".into(),
                sources,
                replace: false,
            },
        ));
        assert!(v.contains(&("/afv/ch1/open".into(), json!(true))), "{v:?}");
    }

    #[test]
    fn tally_sources_are_bounded() {
        let mut m = mixer();
        let sources: BTreeMap<String, String> = (0..MAX_TALLY_SOURCES)
            .map(|n| (format!("CAM{n}"), "off".to_owned()))
            .collect();
        assert!(matches!(
            rest(
                &mut m,
                Command::Tally {
                    id: "t1".into(),
                    sources,
                    replace: false
                }
            ),
            Effect::Changed(_)
        ));
        let one = BTreeMap::from([("CAM0".to_owned(), "program".to_owned())]);
        assert!(matches!(
            rest(
                &mut m,
                Command::Tally {
                    id: "t2".into(),
                    sources: one,
                    replace: false
                }
            ),
            Effect::Changed(_)
        ));
        let more = BTreeMap::from([("NEW".to_owned(), "program".to_owned())]);
        assert_eq!(
            rest(
                &mut m,
                Command::Tally {
                    id: "t3".into(),
                    sources: more,
                    replace: false
                }
            ),
            Effect::Unchanged
        );
        assert!(matches!(
            m.answer(&rest_key(), "t3"),
            Some(Answer::Invalid { .. })
        ));
    }

    fn desk(id: &str, name: &str) -> Console {
        Console {
            id: id.into(),
            name: name.into(),
            protocol: "mcu".into(),
            transport: "bridge".into(),
            strips: 8,
        }
    }

    fn register(m: &mut Mixer, session: &str, console: &Console, bank: u64) -> Effect<Edits> {
        let command = Command::Register {
            id: format!("{session}-c{bank}-{}", console.id),
            console: console.clone(),
            bank_offset: bank,
        };
        m.apply(&origin(session), &serde_json::to_vec(&command).unwrap())
    }

    fn put(
        m: &mut Mixer,
        id: &str,
        console: &str,
        name: &str,
        strips: Option<u64>,
    ) -> Effect<Edits> {
        let put = ConsolePut {
            id: console.into(),
            name: name.into(),
            protocol: None,
            transport: None,
            strips,
        };
        rest(
            m,
            Command::PutConsole {
                id: id.into(),
                console: put,
            },
        )
    }

    fn remove(m: &mut Mixer, id: &str, console: &str) -> Effect<Edits> {
        rest(
            m,
            Command::RemoveConsole {
                id: id.into(),
                console: console.into(),
            },
        )
    }

    fn consoles(m: &Mixer) -> Vec<Value> {
        at(m, "/consoles").as_array().unwrap().clone()
    }

    fn handle(session: &str) -> Value {
        json!(OwnerHandle::of(session).as_str())
    }

    /// A registration publishes the whole list as one change; a bank
    /// change is another; the same registration again changes nothing.
    #[test]
    fn a_registration_publishes_the_console_and_its_bank() {
        let mut m = mixer();
        let xt = desk("xt-1", "X-Touch");
        let v = values(&register(&mut m, "a", &xt, 0));
        let status = json!({
            "id": "xt-1", "name": "X-Touch", "handle": handle("a"), "protocol": "mcu",
            "transport": "bridge", "connected": true, "bankOffset": 0, "strips": 8,
        });
        assert_eq!(v, vec![("/consoles".to_owned(), json!([status.clone()]))]);
        check(&contract().state, m.state()).unwrap();
        let v = values(&register(&mut m, "a", &xt, 8));
        assert_eq!(v[0].1[0]["bankOffset"], 8);
        assert_eq!(register(&mut m, "a", &xt, 8), Effect::Unchanged);
        // One session drives several units.
        register(&mut m, "a", &desk("xt-2", "Extender"), 16);
        let ids: Vec<Value> = consoles(&m).iter().map(|c| c["id"].clone()).collect();
        assert_eq!(ids, [json!("xt-1"), json!("xt-2")], "in registration order");
    }

    /// A console connected to one session is refused to another until the
    /// lease of the first expires, which disconnects it.
    #[test]
    fn a_console_is_claimed_only_once_its_session_is_gone() {
        let mut m = mixer();
        let xt = desk("xt-1", "X-Touch");
        register(&mut m, "a", &xt, 0);
        assert_eq!(register(&mut m, "b", &xt, 8), Effect::Unchanged);
        assert!(matches!(
            m.answer(&origin("b").key(), "b-c8-xt-1"),
            Some(Answer::Refused { .. })
        ));
        let Effect::Changed(e) = m.expire(&origin("a").key()) else {
            panic!("the console disconnects")
        };
        assert_eq!(e.values[0].0, "/consoles");
        assert_eq!(e.values[0].1[0]["connected"], false);
        assert_eq!(e.values[0].1[0]["handle"], handle("a"), "kept for display");
        assert_eq!(m.expire(&origin("a").key()), Effect::Unchanged);
        let v = values(&register(&mut m, "b", &xt, 8));
        assert_eq!(
            (&v[0].1[0]["handle"], &v[0].1[0]["connected"]),
            (&handle("b"), &json!(true))
        );
    }

    /// Declared consoles are there from the start, keep the configured
    /// name and are never removed; registered ones follow them.
    #[test]
    fn declared_consoles_come_first_and_keep_their_name() {
        let mut m = Mixer::with_consoles(
            Shape::parse(&small()).unwrap(),
            vec![desk("xt-1", "X-Touch"), desk("xt-2", "Extender")],
        );
        let list = consoles(&m);
        assert_eq!(list[1]["handle"], Value::Null);
        assert_eq!(
            (&list[1]["connected"], &list[1]["bankOffset"]),
            (&json!(false), &json!(0))
        );
        check(&contract().state, m.state()).unwrap();
        register(&mut m, "a", &desk("osc-1", "Tablet"), 0);
        let renamed = Console {
            strips: 4,
            ..desk("xt-2", "Bridge name")
        };
        register(&mut m, "a", &renamed, 8);
        let list = consoles(&m);
        assert_eq!(
            list.iter().map(|c| c["id"].clone()).collect::<Vec<_>>(),
            [json!("xt-1"), json!("xt-2"), json!("osc-1")]
        );
        assert_eq!(
            (&list[1]["name"], &list[1]["strips"]),
            (&json!("Extender"), &json!(4))
        );
        assert_eq!(remove(&mut m, "d1", "xt-2"), Effect::Unchanged);
        let Some(Answer::Refused { message }) = m.answer(&rest_key(), "d1") else {
            panic!()
        };
        assert!(
            message.contains("declared in the configuration"),
            "{message}"
        );
        values(&put(&mut m, "p1", "xt-1", "Renamed", Some(16)));
        assert_eq!(consoles(&m)[0]["name"], "X-Touch");
        assert_eq!(consoles(&m)[0]["strips"], 16);
    }

    /// REST creates a console no session registered, updates its
    /// descriptor without touching its session or bank, and removes it.
    #[test]
    fn rest_puts_and_removes_consoles() {
        let mut m = mixer();
        values(&put(&mut m, "p1", "desk", "Desk", None));
        assert_eq!(m.answer(&rest_key(), "p1"), Some(&Answer::Created));
        assert_eq!(
            m.console("desk").unwrap(),
            json!({
                "id": "desk", "name": "Desk", "handle": null, "protocol": "mcu",
                "transport": "bridge", "connected": false, "bankOffset": 0, "strips": 8,
            })
        );
        register(&mut m, "a", &desk("desk", "Desk"), 24);
        values(&put(&mut m, "p2", "desk", "Desk 2", Some(16)));
        assert_eq!(m.answer(&rest_key(), "p2"), None, "updated, not created");
        let c = m.console("desk").unwrap();
        assert_eq!(
            (
                &c["name"],
                &c["strips"],
                &c["bankOffset"],
                &c["handle"],
                &c["connected"]
            ),
            (
                &json!("Desk 2"),
                &json!(16),
                &json!(24),
                &handle("a"),
                &json!(true)
            )
        );
        assert_eq!(put(&mut m, "p3", "desk", "Desk 2", None), Effect::Unchanged);
        values(&remove(&mut m, "d1", "desk"));
        assert!(consoles(&m).is_empty());
        assert_eq!(remove(&mut m, "d2", "desk"), Effect::Unchanged);
        assert!(matches!(
            m.answer(&rest_key(), "d2"),
            Some(Answer::NotFound { .. })
        ));
    }

    #[test]
    fn runtime_consoles_are_bounded_and_declared_ones_do_not_count() {
        let mut m = Mixer::with_consoles(
            Shape::parse(&small()).unwrap(),
            vec![desk("xt-1", "X-Touch")],
        );
        for n in 0..MAX_CONSOLES {
            values(&register(&mut m, "a", &desk(&format!("c{n}"), "C"), 0));
        }
        assert_eq!(
            register(&mut m, "a", &desk("one-more", "C"), 0),
            Effect::Unchanged
        );
        assert!(matches!(
            m.answer(&origin("a").key(), "a-c0-one-more"),
            Some(Answer::Invalid { .. })
        ));
        assert_eq!(put(&mut m, "p", "one-more", "C", None), Effect::Unchanged);
        assert!(matches!(
            m.answer(&rest_key(), "p"),
            Some(Answer::Invalid { .. })
        ));
        values(&register(&mut m, "a", &desk("c0", "C"), 8));
    }

    /// Consoles and their sessions survive a snapshot; a snapshot taken
    /// before consoles were kept loads with the declared ones alone.
    #[test]
    fn consoles_load_back_and_merge_with_the_declared_ones() {
        let shape = || Shape::parse(&small()).unwrap();
        let mut m =
            Mixer::with_consoles(shape(), vec![desk("xt-1", "X-Touch"), desk("gone", "Gone")]);
        register(&mut m, "a", &desk("xt-1", "X-Touch"), 8);
        register(&mut m, "a", &desk("osc-1", "Tablet"), 0);
        let saved = m.save();
        let mut back =
            Mixer::with_consoles(shape(), vec![desk("new-1", "New"), desk("xt-1", "Renamed")]);
        back.load(&saved).unwrap();
        let list = consoles(&back);
        assert_eq!(
            list.iter().map(|c| c["id"].clone()).collect::<Vec<_>>(),
            [json!("new-1"), json!("xt-1"), json!("osc-1")],
            "declared in configuration order, then runtime; one no longer declared is dropped"
        );
        assert_eq!(
            (&list[1]["name"], &list[1]["bankOffset"], &list[1]["handle"]),
            (&json!("Renamed"), &json!(8), &handle("a"))
        );
        assert!(matches!(
            back.expire(&origin("a").key()),
            Effect::Changed(_)
        ));
        assert!(consoles(&back).iter().all(|c| c["connected"] == false));
        assert_eq!(
            remove(&mut back, "d", "xt-1"),
            Effect::Unchanged,
            "declared"
        );

        let mut installed = Mixer::default();
        installed.load(&saved).unwrap();
        assert_eq!(installed.text(), m.text());
        assert_eq!(
            register(&mut installed, "b", &desk("osc-1", "Tablet"), 0),
            Effect::Unchanged,
            "the session is kept"
        );

        let mut old: Value = serde_json::from_slice(&mixer().save()).unwrap();
        old.as_object_mut().unwrap().remove("consoles");
        let mut back = Mixer::with_consoles(shape(), vec![desk("xt-1", "X-Touch")]);
        back.load(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(consoles(&back).len(), 1);
        assert_eq!(consoles(&back)[0]["name"], "X-Touch");
    }
}
