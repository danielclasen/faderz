//! The mixer's replicated state: the live state of model.schema.json over
//! the configured shape, touch ownership, the last seq per session and
//! control, scenes and consoles. Every replica applies the same commands
//! in log order through [`Product`], so all of it is decided here and
//! nowhere else, the engine targets included ([`Product::targets`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use samplez::{Fade, Targets};
use samplez_replica::entry::{Origin, SessionKey};
use samplez_replica::state_machine::{Apply, Changed, Effect, Factory, Product};
use serde_json::{Map, Value, json};

use crate::mix::{self, Wiring};
use crate::shape::Shape;

mod afv;
mod command;
mod consoles;
mod control;
mod loudness;
mod paths;
mod ports;
mod scenes;
mod snapshot;
mod targets;
mod validate;

pub use self::afv::MAX_TALLY_SOURCES;
pub use self::command::{Action, Answer, Command, Control, Crossfade, Edits, Transition, Write};
pub use self::consoles::{Console, ConsolePut, MAX_CONSOLES};
pub(crate) use self::consoles::{default_protocol, default_strips, default_transport};
pub(crate) use self::paths::set_at;
pub use self::paths::tokens;
pub use self::ports::{Activation, Connection, PortChange, PortRef, Ports, Scheduled, tai_text};

use self::consoles::Registered;
use self::control::Holder;
use self::snapshot::{
    FORMAT, FORMAT_WITHOUT_PORTS, Saved, Snapshot, fill_comp_mix, fill_mix_minus, fill_pan,
    snapshot_text,
};
use self::targets::Timing;

/// How far past this replica's estimate of the processed index, in
/// samples, a recall's crossfade starts: the longest lead the log stamps
/// a command with, the highest `maxApplyLeadMs` plus samplez's
/// `EXTRA_LEAD`. A recall taking effect by then plays its whole fade; one
/// taking effect later, such as one that switches inputs and waits for
/// an engine generation, joins its fade from the value each gain holds.
pub fn crossfade_lead(rate: u64) -> u64 {
    let ms = *crate::config::MAX_APPLY_LEAD_MS.end()
        + samplez_replica::schedule::EXTRA_LEAD.as_millis() as u64;
    ms * rate / 1000
}

/// The version of how a command applies, how [`Mixer::targets`] derives
/// engine targets from the state, and what save and load produce
/// ([`Product::DERIVATION_VERSION`], in samplez's compatibility key): a
/// change to any of them takes another version, so replicas deriving
/// differently never run side by side.
pub const DERIVATION_VERSION: u32 = 13;
#[derive(Default)]
pub struct Mixer {
    /// `None` only in a [`Default`] state before a snapshot is loaded.
    shape: Option<Arc<Shape>>,
    /// Shared, so a reader copies a pointer under the store lock; a
    /// write copies it only while a reader still holds the old one.
    state: Arc<Value>,
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
    /// Each channel's input flows and domain; empty without an engine.
    wiring: Vec<Wiring>,
    /// The IS-05 connections.
    ports: Ports,
    /// [`Mixer::wiring`] with the leg flows of [`Ports`].
    effective: Vec<Wiring>,
    /// Every flow an output writes or the set merges its members into: no
    /// receiver may read one.
    own: BTreeSet<String>,
    /// The engine targets of the state, kept by every write; none
    /// without an engine. Derived from the state alone, so neither
    /// replicated nor saved.
    derived: Option<mix::Derived>,
    /// The fade of each gain a crossfading recall or a tally's AFV
    /// transition changed, shaped as the targets' gains, or empty for
    /// none ([`Mixer::fading`]).
    fading: Vec<Vec<Option<Fade>>>,
    /// The ports whose `/active` the command being applied changed, for
    /// [`Mixer::stamp`]; empty between commands.
    touched: Vec<PortRef>,
}

impl Mixer {
    pub fn new(shape: Shape) -> Mixer {
        Mixer::with_consoles(shape, Vec::new())
    }

    /// A state with the `declared` consoles of the configuration, not
    /// connected yet.
    pub fn with_consoles(shape: Shape, declared: Vec<Console>) -> Mixer {
        Mixer::configured(shape, declared, Vec::new())
    }

    /// [`Mixer::with_consoles`] whose channels' inputs are `wiring`, one
    /// per channel: the state it derives engine targets from
    /// ([`Product::targets`]).
    pub fn configured(shape: Shape, declared: Vec<Console>, wiring: Vec<Wiring>) -> Mixer {
        let mut m = Mixer {
            state: Arc::new(shape.initial_state()),
            shape: Some(Arc::new(shape)),
            consoles: declared.iter().map(Mixer::declared_entry).collect(),
            declared,
            effective: wiring.clone(),
            wiring,
            ..Mixer::default()
        };
        let consoles = m.console_list();
        Arc::make_mut(&mut m.state)["consoles"] = consoles;
        m.derive();
        m
    }

    /// Builds [`Mixer::configured`]: the empty state every replica of a
    /// set opens and installs snapshots into.
    pub fn factory(
        shape: Shape,
        declared: Vec<Console>,
        wiring: Vec<Wiring>,
        own: BTreeSet<String>,
    ) -> Factory<Mixer> {
        Arc::new(move || {
            let mut m = Mixer::configured(shape.clone(), declared.clone(), wiring.clone());
            m.own.clone_from(&own);
            m
        })
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

    /// [`Mixer::shape`] shared: a pointer copy, for a reader that takes it
    /// out of the store lock.
    pub fn shared_shape(&self) -> Arc<Shape> {
        self.shape.clone().expect("a shape is configured or loaded")
    }

    /// [`Mixer::state`] shared: a pointer copy, for a reader that takes it
    /// out of the store lock.
    pub fn shared_state(&self) -> Arc<Value> {
        self.state.clone()
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
}

impl Product for Mixer {
    type Change = Edits;

    const DERIVATION_VERSION: u32 = crate::state::DERIVATION_VERSION;
    const DRIVES_ENGINE: bool = true;

    /// A command that switches an input is refused as [`Answer::Refused`]
    /// when the writer ordered it off the generation path, or when it
    /// switches inputs in more than one MXL domain.
    fn apply(&mut self, origin: &Origin, command: &[u8], ctx: &Apply) -> Effect<Edits> {
        let Ok(command) = serde_json::from_slice::<Command>(command) else {
            return Effect::Unchanged;
        };
        let session = origin.key();
        self.touched.clear();
        let timing = match &command {
            Command::Recall { fade: Some(c), .. } => Timing::All(*c),
            Command::Tally {
                transition: Some(t),
                ..
            } => Timing::Afv(*t),
            _ => Timing::Cut,
        };
        let effect = self.fading(timing, |m| match &command {
            Command::Control(c) => m.control(origin, c, ctx),
            Command::Patch { id, changes } => m.patch(&session, id, changes, ctx),
            Command::StoreScene {
                id,
                scene,
                name,
                at,
            } => m.store_scene(&session, id, scene.as_deref(), name.as_deref(), at),
            Command::DeleteScene { id, scene } => m.delete_scene(&session, id, scene),
            Command::Recall { id, scene, at, .. } => m.recall(&session, id, scene, at, ctx),
            Command::Tally {
                id,
                sources,
                replace,
                ..
            } => m.tally(&session, id, sources, *replace),
            Command::Register {
                id,
                console,
                bank_offset,
            } => m.register(origin, id, console, *bank_offset),
            Command::PutConsole { id, console } => m.put_console(&session, id, console),
            Command::RemoveConsole { id, console } => m.remove_console(&session, id, console),
            Command::Connect { id, port, active } => m.connect(&session, id, port, active, ctx),
            Command::Schedule {
                id,
                port,
                active,
                at,
            } => m.schedule(&session, id, port, active, *at),
            Command::Unschedule { port, .. } => m.unschedule(port),
            Command::Fire { id, schedule } => m.fire(&session, id, schedule, ctx),
            Command::ResetLoudness { id, ids } => m.reset_loudness(&session, id, ids),
            Command::ControlRequest { path, from, at, .. } => {
                m.control_request(&session, path, from, *at)
            }
        });
        self.stamp(command.id(), ctx.at_index);
        effect
    }

    /// [`mix::targets`] of the state, as every write keeps them, with
    /// the loudness measurements' run flags and reset epochs
    /// ([`mix::loudness`]) and the gains' fades ([`Mixer::fading`]);
    /// none without an engine's wiring.
    fn targets(&self) -> Option<Targets> {
        let mut t = self.derived.as_ref()?.targets().clone();
        self.overlay(&mut t);
        mix::loudness(self.shape(), self.state(), &mut t);
        t.fading.clone_from(&self.fading);
        Some(t)
    }

    fn reroutes(&self, _origin: &Origin, command: &[u8]) -> Vec<u32> {
        match serde_json::from_slice::<Command>(command) {
            Ok(Command::Connect {
                port: PortRef::Receiver(channel),
                active,
                ..
            }) => self.connect_reroutes(&channel, &active),
            Ok(Command::Fire { schedule, .. }) => (self.ports.scheduled.iter())
                .find(|s| s.id == schedule)
                .and_then(|s| match &s.port {
                    PortRef::Receiver(channel) => Some(self.connect_reroutes(channel, &s.active)),
                    PortRef::Sender(_) => None,
                })
                .unwrap_or_default(),
            Ok(c) => self.rerouted(&self.writes_of(&c)),
            Err(_) => Vec::new(),
        }
    }

    /// XXH3 over the shape, the declared consoles and the wiring: what
    /// [`Product::apply`], [`Product::load`] and [`Product::targets`] read
    /// besides the state.
    fn config_hash(&self) -> u64 {
        let config = json!({
            // Frozen: this input never moves, so a release keeps the
            // hash. A derivation change raises
            // Product::DERIVATION_VERSION instead.
            "derivation": 4,
            "shape": self.shape,
            "consoles": self.declared,
            "wiring": self.wiring,
            "own": self.own,
        });
        xxhash_rust::xxh3::xxh3_64(config.to_string().as_bytes())
    }

    /// Releases every control `session` holds and disconnects its
    /// consoles.
    fn expire(&mut self, session: &SessionKey) -> Effect<Edits> {
        self.touched.clear();
        let effect = self.fading(Timing::Cut, |m| m.end_lease(session));
        self.stamp("", None);
        effect
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
        let mut requests: Vec<&Value> = Vec::new();
        for c in changes {
            requests.extend(&c.change.requests);
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
        if !requests.is_empty() {
            delta.insert("requests".into(), json!(requests));
        }
        Value::Object(delta).to_string()
    }

    /// `shape`, `state` and `owners`, which the snapshot message carries.
    fn text(&self) -> String {
        snapshot_text(self.shape(), &self.state, &self.owners())
    }

    /// [`Product::text`] built after the store lock: under it only the
    /// shared shape and state are taken and the owners listed.
    fn text_later(&self) -> Option<Box<dyn FnOnce() -> String + Send>> {
        let (shape, state, owners) = (self.shared_shape(), self.shared_state(), self.owners());
        Some(Box::new(move || snapshot_text(&shape, &state, &owners)))
    }

    fn save(&self) -> Vec<u8> {
        let saved = Saved {
            shape: self.shape().clone(),
            state: Value::clone(&self.state),
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
            ports: self.ports.clone(),
            fading: (self.fading.iter())
                .map(|row| {
                    (row.iter())
                        .map(|f| f.map(|f| (f.from.to_bits(), f.start, f.len)))
                        .collect()
                })
                .collect(),
        };
        let snapshot = Snapshot {
            format: FORMAT,
            mixer: saved,
        };
        serde_json::to_vec(&snapshot).expect("the state serializes")
    }

    /// A state with a configured shape keeps it ([`Mixer::reconcile`]); a
    /// [`Default`] one, as a snapshot install builds, takes the saved one.
    /// A snapshot of another format than [`FORMAT`] is refused, one taken
    /// before formats were kept is read as it was written.
    fn load(&mut self, saved: &[u8]) -> Result<(), String> {
        let value: Value = serde_json::from_slice(saved).map_err(|e| e.to_string())?;
        let saved: Saved = match value.get("format") {
            None => serde_json::from_value(value).map_err(|e| e.to_string())?,
            Some(f) if *f == FORMAT || *f == FORMAT_WITHOUT_PORTS => {
                let snapshot: Snapshot =
                    serde_json::from_value(value).map_err(|e| e.to_string())?;
                snapshot.mixer
            }
            Some(f) => return Err(format!("snapshot format {f}: this binary reads {FORMAT}")),
        };
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
        self.fading = (saved.fading.iter())
            .map(|row| {
                (row.iter())
                    .map(|f| {
                        f.map(|(from, start, len)| Fade {
                            from: f32::from_bits(from),
                            start,
                            len,
                        })
                    })
                    .collect()
            })
            .collect();
        if self.shape.is_some() {
            self.reconcile(saved);
        } else {
            self.shape = Some(Arc::new(saved.shape));
            self.state = Arc::new(saved.state);
            fill_pan(Arc::make_mut(&mut self.state));
            fill_mix_minus(Arc::make_mut(&mut self.state));
            fill_comp_mix(Arc::make_mut(&mut self.state));
            // Taken before duck groups were kept: its shape has none.
            if self.state.get("ducks").is_none() {
                Arc::make_mut(&mut self.state)["ducks"] = json!({});
            }
            self.owners = saved.owners.into_iter().collect();
            self.dropped = Vec::new();
            self.consoles = saved.consoles;
            self.ports = saved.ports;
        }
        self.rewire();
        self.derive();
        // Fades shaped for other gains than the configuration's are
        // dropped with the rest the shape has no place for.
        let shaped = |gains: &Vec<Vec<f32>>| {
            self.fading.len() == gains.len()
                && (self.fading.iter().zip(gains)).all(|(f, g)| f.len() == g.len())
        };
        if !self
            .derived
            .as_ref()
            .is_some_and(|d| shaped(&d.targets().gains))
        {
            self.fading.clear();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
