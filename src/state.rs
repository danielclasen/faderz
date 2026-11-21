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
use samplez_service::OwnerHandle;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::contract::{check, contract};
use crate::duck;
use crate::mix::{self, Wiring};
use crate::shape::Shape;

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
pub const DERIVATION_VERSION: u32 = 10;

/// The layout [`Product::save`] writes. A snapshot carries it beside
/// the state, which binaries before it read from the top level: they
/// refuse it as missing their fields rather than load part of it. 3
/// adds the IS-05 connections ([`Ports`]), which a binary reading 2
/// would drop.
const FORMAT: u32 = 3;
/// The layout before [`Ports`], read as one without connections.
const FORMAT_WITHOUT_PORTS: u32 = 2;

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
        /// None for a cut.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fade: Option<Crossfade>,
    },
    /// `POST /afv/tally` (one source) or `PUT /afv/tally` (`replace`: the
    /// sources not listed go `off`).
    Tally {
        id: String,
        sources: BTreeMap<String, String>,
        replace: bool,
        /// None for a cut, as logged before AFV stages faded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition: Option<Transition>,
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
    /// An IS-05 activation of a channel's receiver or an output's sender,
    /// as its `/active` shows it ([`crate::nmos`]).
    Connect {
        id: String,
        port: PortRef,
        active: Connection,
    },
    /// A scheduled IS-05 activation, taken when it is staged: the port's
    /// only pending one, which [`Command::Fire`] applies at `at`.
    Schedule {
        id: String,
        port: PortRef,
        active: Connection,
        /// TAI nanoseconds.
        at: u64,
    },
    /// Cancels the port's scheduled activation.
    Unschedule {
        id: String,
        port: PortRef,
    },
    /// Applies the scheduled activation `schedule` (its [`Command::Schedule`]
    /// id) as a [`Command::Connect`], when the port still has it pending.
    Fire {
        id: String,
        schedule: String,
    },
    /// `POST /loudness/reset`: counts a reset of each output's loudness
    /// measurement in the state, which every replica's engine follows at
    /// the delta's `atIndex`.
    ResetLoudness {
        id: String,
        ids: Vec<String>,
    },
    /// A WebSocket `request`: the holder of `path`, whichever replica its
    /// connection is on, is told `from` asks for it. `from` is the
    /// asker's owner record.
    ControlRequest {
        id: String,
        path: String,
        from: Value,
        /// When the asker's replica took it, Unix milliseconds: a replica
        /// that applies it much later does not hand it to the holder.
        at: u64,
    },
}

/// A recall's crossfade: every gain the recall changes ramps linearly
/// over `len` samples from sample index `start`. The receiving replica
/// chooses `start` and the log carries it, so every replica ramps from
/// the same index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crossfade {
    pub start: u64,
    pub len: u32,
}

/// A tally's transition: the sample index it takes effect at and the
/// `transitionMs` it carried. The receiving replica chooses `start` and
/// the log carries it, so every replica fades each AFV stage the tally
/// opens or closes from the same index ([`Mixer::afv_fades`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub start: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_ms: Option<u32>,
}

/// How [`Mixer::fading`] times the gain targets a command changes.
#[derive(Clone, Copy)]
enum Timing {
    /// None fades: the engine ramps each over the ramp length.
    Cut,
    /// Every one alike, a recall's crossfade.
    All(Crossfade),
    /// Those of each channel whose AFV stage a tally opens or closes, by
    /// that stage's times ([`Mixer::afv_fades`]); the others none.
    Afv(Transition),
}

impl Command {
    /// The client's command id.
    pub fn id(&self) -> &str {
        match self {
            Command::Control(c) => &c.id,
            Command::Patch { id, .. }
            | Command::StoreScene { id, .. }
            | Command::DeleteScene { id, .. }
            | Command::Recall { id, .. }
            | Command::Tally { id, .. }
            | Command::Register { id, .. }
            | Command::PutConsole { id, .. }
            | Command::RemoveConsole { id, .. }
            | Command::Connect { id, .. }
            | Command::Schedule { id, .. }
            | Command::Unschedule { id, .. }
            | Command::Fire { id, .. }
            | Command::ResetLoudness { id, .. }
            | Command::ControlRequest { id, .. } => id,
        }
    }
}

/// An IS-05 port: the receiver of a channel or the sender of an output,
/// by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PortRef {
    Receiver(String),
    Sender(String),
}

/// An IS-05 port's `/active`, ids as lowercase UUID text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Connection {
    pub master_enable: bool,
    /// `sender_id` of a receiver, `receiver_id` of a sender.
    pub peer: Option<String>,
    pub domain: Option<String>,
    pub flow: Option<String>,
    /// How it was activated, for `/active.activation` on every replica;
    /// none for an activation taken before it was kept.
    #[serde(default)]
    pub activation: Option<Activation>,
}

/// An IS-05 activation's `mode`, `requested_time` and TAI
/// `activation_time`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Activation {
    pub mode: String,
    pub requested: Option<String>,
    pub time: String,
}

/// TAI nanoseconds as IS-05 writes a time, `<seconds>:<nanoseconds>`.
pub fn tai_text(nanos: u64) -> String {
    format!("{}:{}", nanos / 1_000_000_000, nanos % 1_000_000_000)
}

/// The IS-05 connections the state keeps: each port's last activation
/// and each channel's leg flows the activations named in place of the
/// configured ones. A port without an activation is as configured.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ports {
    receivers: BTreeMap<String, Connection>,
    senders: BTreeMap<String, Connection>,
    legs: BTreeMap<String, Legs>,
    /// The scheduled activations, one per port at most.
    #[serde(default)]
    scheduled: Vec<Scheduled>,
    /// The change that last changed each port's `/active`, by receiver
    /// and by sender; none for a port no change has changed.
    #[serde(default)]
    receiver_changes: BTreeMap<String, PortChange>,
    #[serde(default)]
    sender_changes: BTreeMap<String, PortChange>,
}

/// A replicated change of a port's `/active`: the port's IS-04 version
/// from it on and the id of the command that made it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortChange {
    /// TAI nanoseconds: the time of the index the command's entry takes
    /// effect at ([`Apply::at_index`]), or the nanosecond after the
    /// port's previous version where that is not later, as
    /// `samplez_nmos::Nmos::replicated_change` sets a version, so every
    /// replica serves this one.
    pub version: u64,
    pub id: String,
}

/// A scheduled activation the state holds until it is fired or
/// cancelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scheduled {
    /// The id of its [`Command::Schedule`].
    pub id: String,
    pub port: PortRef,
    pub active: Connection,
    /// TAI nanoseconds.
    pub at: u64,
}

/// A channel's flows of input a and b that activations named.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Legs {
    a: Option<String>,
    b: Option<String>,
    /// The channel's configured flows of input a and b when an activation
    /// named one: a configuration that wires others drops the legs.
    #[serde(default)]
    wired: Option<(String, Option<String>)>,
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
    /// a declared or connected one to remove, or inputs a command switches
    /// that it may not ([`Product::apply`]).
    Refused { message: String },
    /// The console a `PutConsole` created.
    Created,
    /// The channels a `Recall` set to a delay of 0: their stored delay is
    /// past their delay line.
    Recalled { delays: Vec<String> },
}

/// What one command or expiry changed, for [`Product::delta`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Edits {
    values: Vec<(String, Value)>,
    owners: Vec<(String, Value)>,
    /// The delta's `origins` entry: client commands only.
    origin: Option<Value>,
    /// Control requests the replicas deliver to the holder's connection,
    /// whichever replica it is on; the server takes them out of the
    /// delta before the clients see it.
    requests: Vec<Value>,
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

/// The principal of every client while no authentication names one: not
/// an identity two sessions can be told apart by.
const ANONYMOUS: &str = "anonymous";

/// Tally sources the state holds at most: a source once named stays,
/// `off` after a `PUT` that leaves it out.
pub const MAX_TALLY_SOURCES: usize = 256;

/// Consoles registered at runtime the state holds at most, besides the
/// declared ones.
pub const MAX_CONSOLES: usize = 64;

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

/// A snapshot from [`FORMAT`] on.
#[derive(Serialize, Deserialize)]
struct Snapshot {
    format: u32,
    mixer: Saved,
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
    /// Absent before [`FORMAT`] 3.
    #[serde(default)]
    ports: Ports,
    /// [`Mixer::fading`], each fade as the bits of `from`, `start` and
    /// `len`. Absent before recalls crossfaded.
    #[serde(default)]
    fading: Vec<Vec<Option<(u32, u64, u32)>>>,
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

    /// Derives the engine targets of the whole state again.
    fn derive(&mut self) {
        self.derived = match (self.shape.as_deref(), self.wiring.is_empty()) {
            (Some(shape), false) => Some(mix::Derived::new(shape, &self.effective, &self.state)),
            _ => None,
        };
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

    /// The channel slots whose selected flow `written` changes, applied to
    /// the state as it is, ascending.
    fn rerouted(&self, written: &[(String, Value)]) -> Vec<u32> {
        let (Some(shape), false) = (self.shape.as_deref(), self.wiring.is_empty()) else {
            return Vec::new();
        };
        let mut after = Map::new();
        for (path, value) in written {
            if let [group, id, leaf] = tokens(path).as_slice()
                && group == "channels"
                && leaf == "input"
            {
                after.insert(id.clone(), value.clone());
            }
        }
        let flow = |w: &Wiring, input: Option<&Value>| match (input.and_then(Value::as_str), &w.b) {
            (Some("b"), Some(b)) => b.clone(),
            _ => w.a.clone(),
        };
        (shape.channels.iter().zip(&self.effective).enumerate())
            .filter(|(_, (c, w))| {
                after.get(&c.id).is_some_and(|input| {
                    flow(w, Some(input)) != flow(w, self.state["channels"][&c.id].get("input"))
                })
            })
            .map(|(slot, _)| slot as u32)
            .collect()
    }

    /// Why a command writing `written`, ordered as `ctx` says, is refused
    /// for the inputs it re-routes: off the generation path, or across
    /// more than one MXL domain, which one engine generation cannot
    /// switch together. A command the writer did not classify
    /// (`generation` `None`) is taken as it was before.
    fn reroute_refusal(&self, ctx: &Apply, written: &[(String, Value)]) -> Option<Answer> {
        let generation = ctx.generation?;
        let slots = self.rerouted(written);
        if slots.is_empty() {
            return None;
        }
        if !generation {
            let message =
                "the command switches inputs and was not ordered to: send it again".into();
            return Some(Answer::Refused { message });
        }
        let domains: BTreeSet<&str> = (slots.iter())
            .map(|&s| self.effective[s as usize].domain.as_str())
            .collect();
        (domains.len() > 1).then(|| Answer::Refused {
            message: format!(
                "the command switches inputs in more than one MXL domain ({})",
                domains.into_iter().collect::<Vec<_>>().join(", ")
            ),
        })
    }

    /// [`Mixer::wiring`] with the leg flows activations named.
    fn rewire(&mut self) {
        let Some(shape) = self.shape.clone() else {
            return;
        };
        self.effective = (shape.channels.iter().zip(&self.wiring))
            .map(|(c, w)| {
                let mut w = w.clone();
                if let Some(legs) = self.ports.legs.get(&c.id) {
                    if let Some(a) = &legs.a {
                        w.a.clone_from(a);
                    }
                    if let (Some(b), Some(_)) = (&legs.b, &w.b) {
                        w.b = Some(b.clone());
                    }
                }
                w
            })
            .collect();
    }

    /// Detaches the channels whose receiver is disabled and sets each
    /// output's enable, in `t` as derived from the state.
    fn overlay(&self, t: &mut Targets) {
        let shape = self.shape();
        for (flow, c) in t.flows.iter_mut().zip(&shape.channels) {
            if self
                .ports
                .receivers
                .get(&c.id)
                .is_some_and(|r| !r.master_enable)
            {
                *flow = None;
            }
        }
        t.outputs = (shape.outputs.iter())
            .map(|o| {
                self.ports
                    .senders
                    .get(&o.id)
                    .is_none_or(|s| s.master_enable)
            })
            .collect();
    }

    /// Each gain's fade ([`Mixer::fading`]) with the gain target it fades
    /// to, shaped as the targets' gains, or empty for none: what an engine
    /// following the state's deltas sends a gain change with.
    pub fn gain_fades(&self) -> Vec<Vec<Option<(f32, Fade)>>> {
        let Some(d) = &self.derived else {
            return Vec::new();
        };
        (self.fading.iter().zip(&d.targets().gains))
            .map(|(fades, gains)| {
                (fades.iter().zip(gains))
                    .map(|(f, &g)| f.map(|f| (g, f)))
                    .collect()
            })
            .collect()
    }

    /// What the engine reads and writes beside the state: the flows with
    /// the activations' leg flows, the detached channels and each
    /// output's enable ([`Product::targets`] without deriving them).
    pub fn engine_view(&self) -> mix::EngineView {
        let mut t = Targets {
            flows: vec![Some(String::new()); self.effective.len()],
            ..Targets::default()
        };
        self.overlay(&mut t);
        mix::EngineView {
            wiring: self.effective.clone(),
            detached: t.flows.iter().map(Option::is_none).collect(),
            outputs: t.outputs,
        }
    }

    /// Channel `id`'s receiver: its last activation, none before the
    /// first, and the flow its selected input reads; none for a channel
    /// the engine does not read.
    pub fn receiver(&self, id: &str) -> Option<(Option<&Connection>, &str)> {
        let slot = self.shape().channels.iter().position(|c| c.id == id)?;
        let w = self.effective.get(slot)?;
        Some((self.ports.receivers.get(id), mix::flow(&self.state, id, w)))
    }

    /// Output `id`'s sender's last activation, none before the first.
    pub fn sender(&self, id: &str) -> Option<&Connection> {
        self.ports.senders.get(id)
    }

    /// The flow slot `slot` reads, none for a detached one.
    fn target_flow(&self, slot: usize) -> Option<String> {
        let c = &self.shape().channels[slot];
        let detached = self
            .ports
            .receivers
            .get(&c.id)
            .is_some_and(|r| !r.master_enable);
        (!detached).then(|| mix::flow(&self.state, &c.id, &self.effective[slot]).to_owned())
    }

    /// The slot of channel `channel` when activating its receiver as
    /// `active` changes the flow it reads.
    fn connect_reroutes(&self, channel: &str, active: &Connection) -> Vec<u32> {
        let Some(slot) = (self.shape.as_deref())
            .filter(|_| !self.wiring.is_empty())
            .and_then(|s| s.channels.iter().position(|c| c.id == channel))
        else {
            return Vec::new();
        };
        let after = active.flow.clone().filter(|_| active.master_enable);
        if after == self.target_flow(slot) {
            Vec::new()
        } else {
            vec![slot as u32]
        }
    }

    /// Activates a port as `active`: a receiver's flow becomes the flow of
    /// its channel's selected input, and a disabled receiver detaches its
    /// channel; a sender's enable starts or stops its output.
    fn connect(
        &mut self,
        session: &SessionKey,
        id: &str,
        port: &PortRef,
        active: &Connection,
        ctx: &Apply,
    ) -> Effect<Edits> {
        if let Err(answer) = self.check_connection(port, active) {
            return self.refuse(session, id, answer);
        }
        let shape = self.shape.clone().expect("configured");
        match port {
            PortRef::Receiver(channel) => {
                let slot = (shape.channels.iter())
                    .position(|c| c.id == *channel)
                    .expect("checked");
                if ctx.generation == Some(false)
                    && !self.connect_reroutes(channel, active).is_empty()
                {
                    let message =
                        "the command switches inputs and was not ordered to: send it again".into();
                    return self.refuse(session, id, Answer::Refused { message });
                }
                if let Some(flow) = &active.flow {
                    let b = self.state["channels"][channel]["input"] == "b"
                        && self.wiring[slot].b.is_some();
                    let w = &self.wiring[slot];
                    let wired = Some((w.a.clone(), w.b.clone()));
                    let legs = self.ports.legs.entry(channel.clone()).or_default();
                    *(if b { &mut legs.b } else { &mut legs.a }) = Some(flow.clone());
                    legs.wired = wired;
                }
                self.ports.receivers.insert(channel.clone(), active.clone());
                self.rewire();
                if let Some(d) = &mut self.derived {
                    let path = format!("/channels/{}/input", escape(channel));
                    d.update(
                        &shape,
                        &self.effective,
                        &self.state,
                        &Value::Null,
                        [path.as_str()],
                    );
                }
            }
            PortRef::Sender(output) => {
                self.ports.senders.insert(output.clone(), active.clone());
            }
        }
        // An activation supersedes one scheduled for the port.
        self.ports.scheduled.retain(|s| s.port != *port);
        self.touch(port.clone());
        Effect::Changed(Edits::default())
    }

    /// Takes a scheduled activation: checked as [`Mixer::connect`] checks
    /// it, but for the generation path, which [`Command::Fire`] takes.
    fn schedule(
        &mut self,
        session: &SessionKey,
        id: &str,
        port: &PortRef,
        active: &Connection,
        at: u64,
    ) -> Effect<Edits> {
        if let Err(answer) = self.check_connection(port, active) {
            return self.refuse(session, id, answer);
        }
        self.ports.scheduled.retain(|s| s.port != *port);
        self.ports.scheduled.push(Scheduled {
            id: id.to_owned(),
            port: port.clone(),
            active: active.clone(),
            at,
        });
        Effect::Changed(Edits::default())
    }

    fn unschedule(&mut self, port: &PortRef) -> Effect<Edits> {
        let before = self.ports.scheduled.len();
        self.ports.scheduled.retain(|s| s.port != *port);
        if self.ports.scheduled.len() == before {
            Effect::Unchanged
        } else {
            Effect::Changed(Edits::default())
        }
    }

    /// Fires scheduled activation `schedule` when it is still pending;
    /// refused off the generation path, it stays pending.
    fn fire(
        &mut self,
        session: &SessionKey,
        id: &str,
        schedule: &str,
        ctx: &Apply,
    ) -> Effect<Edits> {
        let Some(s) = self
            .ports
            .scheduled
            .iter()
            .find(|s| s.id == schedule)
            .cloned()
        else {
            return Effect::Unchanged;
        };
        // One that can no longer be activated never will: dropped on every
        // replica alike. One ordered off the generation path is kept for
        // the next try.
        if let Err(answer) = self.check_connection(&s.port, &s.active) {
            self.record(session, id, answer);
            self.ports.scheduled.retain(|p| p.id != s.id);
            return Effect::Changed(Edits::default());
        }
        // The mode and requested time of the request that scheduled it,
        // when the schedule carries them: a relative activation stays
        // relative on `/active`. Without them it was asked for at `at`.
        // A replica of a release before this takes a schedule with them
        // and fires it as absolute all the same, so during a rolling
        // upgrade `/active` can show either until every replica runs
        // this release; the activation itself is the same on all.
        let at = tai_text(s.at);
        let (mode, requested) = match &s.active.activation {
            Some(a) => (a.mode.clone(), a.requested.clone()),
            None => ("activate_scheduled_absolute".into(), Some(at.clone())),
        };
        let active = Connection {
            activation: Some(Activation {
                mode,
                requested,
                time: at,
            }),
            ..s.active
        };
        self.connect(session, id, &s.port, &active, ctx)
    }

    /// The scheduled activations, in the order they were taken.
    pub fn scheduled(&self) -> &[Scheduled] {
        &self.ports.scheduled
    }

    /// The change that last changed `port`'s `/active`, none before the
    /// first.
    pub fn port_change(&self, port: &PortRef) -> Option<&PortChange> {
        match port {
            PortRef::Receiver(id) => self.ports.receiver_changes.get(id),
            PortRef::Sender(id) => self.ports.sender_changes.get(id),
        }
    }

    fn touch(&mut self, port: PortRef) {
        if !self.touched.contains(&port) {
            self.touched.push(port);
        }
    }

    /// Records command `id` as the change of each port it changed
    /// ([`PortChange`]), its entry taking effect at `at_index`.
    fn stamp(&mut self, id: &str, at_index: Option<u64>) {
        if self.touched.is_empty() {
            return;
        }
        let rate = u128::from(self.shape().sample_rate);
        let at = at_index.map_or(0, |i| {
            u64::try_from(u128::from(i) * 1_000_000_000 / rate).unwrap_or(u64::MAX)
        });
        for port in std::mem::take(&mut self.touched) {
            let (changes, key) = match port {
                PortRef::Receiver(id) => (&mut self.ports.receiver_changes, id),
                PortRef::Sender(id) => (&mut self.ports.sender_changes, id),
            };
            // A port starts at 0:0, so its first change is at least 1.
            let after = changes.get(&key).map_or(1, |c| c.version + 1);
            let change = PortChange {
                version: at.max(after),
                id: id.to_owned(),
            };
            changes.insert(key, change);
        }
    }

    /// Why `active` cannot be activated on `port`, as [`Mixer::connect`]
    /// refuses it.
    fn check_connection(&self, port: &PortRef, active: &Connection) -> Result<(), Answer> {
        let ids = [&active.peer, &active.domain, &active.flow];
        if let Some(bad) = ids.into_iter().flatten().find(|v| !is_uuid(v)) {
            return Err(Answer::Invalid {
                message: format!("{bad:?} is no lowercase UUID"),
            });
        }
        let shape = self.shape();
        match port {
            PortRef::Receiver(channel) => {
                if self.wiring.is_empty() || !shape.is_channel(channel) {
                    let message = format!("no receiver {channel}");
                    return Err(Answer::NotFound { message });
                }
                if active.master_enable && active.flow.is_none() {
                    let message = "an enabled receiver needs a flow".to_owned();
                    return Err(Answer::Invalid { message });
                }
                if let Some(flow) = active.flow.as_ref().filter(|f| self.own.contains(*f)) {
                    let message = format!("flow {flow} is written by this mixer's own outputs");
                    return Err(Answer::Invalid { message });
                }
            }
            PortRef::Sender(output) => {
                if !shape.outputs.iter().any(|o| o.id == *output) {
                    let message = format!("no sender {output}");
                    return Err(Answer::NotFound { message });
                }
            }
        }
        Ok(())
    }

    /// A switch between input a and b among `changed` shows in the
    /// channel's receiver: its flow becomes the newly selected input's.
    fn follow_switches(&mut self, changed: &[(String, Value)]) {
        for (path, _) in changed {
            if let [group, channel, leaf] = tokens(path).as_slice()
                && group == "channels"
                && leaf == "input"
                && let Some(slot) = self.shape().channels.iter().position(|c| c.id == *channel)
                && let Some(w) = self.effective.get(slot)
            {
                let flow = mix::flow(&self.state, channel, w).to_owned();
                if let Some(r) = self.ports.receivers.get_mut(channel) {
                    r.flow = Some(flow);
                }
                self.touch(PortRef::Receiver(channel.clone()));
            }
        }
    }

    /// The leaves a command would write, applied to the state as it is,
    /// for the writer's classification ([`Product::reroutes`]): those of a
    /// set, a patch or a recall, the only commands that write an input
    /// selection.
    fn writes_of(&self, command: &Command) -> Vec<(String, Value)> {
        match command {
            Command::Control(c) if c.action == Action::Set => {
                vec![(c.path.clone(), c.value.clone().unwrap_or(Value::Null))]
            }
            Command::Patch { changes, .. } => (changes.iter())
                .map(|c| (c.path.clone(), c.value.clone()))
                .collect(),
            Command::Recall { scene, .. } => (self.recall_writes(scene))
                .map(|(written, _)| written)
                .unwrap_or_default(),
            _ => Vec::new(),
        }
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
        } else if let [group, channel, leaf] = tokens(path).as_slice()
            && (group.as_str(), leaf.as_str()) == ("channels", "delay")
        {
            // The engine refuses a delay past the input's line.
            let line = (shape.channels.iter())
                .find(|c| &c.id == channel)
                .and_then(|c| c.max_delay);
            let samples = value.as_u64().unwrap_or(0);
            match line {
                None if samples > 0 => {
                    return Err(format!("channel {channel} has no delay line"));
                }
                Some(max) if samples > u64::from(max) => {
                    return Err(format!(
                        "channel {channel} delays at most {max} samples, its delay line"
                    ));
                }
                _ => {}
            }
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
    /// bindings and tally. The engine targets follow the leaves changed.
    fn write(&mut self, written: Vec<(String, Value)>) -> Vec<(String, Value)> {
        let all = derived(written);
        let vcas = (self.derived.is_some() && all.iter().any(|(p, _)| p.starts_with("/vcas")))
            .then(|| self.state["vcas"].clone());
        let afv = all
            .iter()
            .any(|(p, _)| p.starts_with("/afv/") || p.starts_with("/tally/"));
        let mut changed: Vec<(String, Value)> = Vec::new();
        for (path, value) in all {
            if self.state.pointer(&path) != Some(&value) {
                set_at(Arc::make_mut(&mut self.state), &path, value.clone());
                changed.retain(|(p, _)| *p != path);
                changed.push((path, value));
            }
        }
        if afv {
            for (channel, open) in self.afv_open() {
                if self.state["afv"][&channel]["open"] != open {
                    let path = format!("/afv/{channel}/open");
                    set_at(Arc::make_mut(&mut self.state), &path, json!(open));
                    changed.push((path, json!(open)));
                }
            }
        }
        if let (Some(d), Some(shape)) = (&mut self.derived, self.shape.as_deref()) {
            let vcas = vcas.as_ref().unwrap_or(&Value::Null);
            let paths = changed.iter().map(|(p, _)| p.as_str());
            d.update(shape, &self.effective, &self.state, vcas, paths);
        }
        self.follow_switches(&changed);
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

    /// Momentary controls among `paths` (IFB interrupt, duck hold) go back
    /// to off.
    fn momentary_off(&self, paths: &[String]) -> Vec<(String, Value)> {
        paths
            .iter()
            .filter(|p| momentary(p))
            .filter(|p| self.state.pointer(p) == Some(&json!(true)))
            .map(|p| (p.clone(), json!(false)))
            .collect()
    }

    fn control(&mut self, origin: &Origin, c: &Control, ctx: &Apply) -> Effect<Edits> {
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
        let other = holder.as_ref().filter(|h| h.key() != session);
        // A console takes a control its own principal holds on a screen.
        let takeover = other.is_some_and(|h| {
            kind == "console"
                && h.kind != "console"
                && h.principal == origin.principal
                && origin.principal != ANONYMOUS
        });
        if action != Action::Release
            && !takeover
            && let Some(h) = other
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
                if holder.is_some() && !takeover {
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
                    ..Edits::default()
                }
            }
            Action::Set => {
                let value = value.cloned().unwrap_or(Value::Null);
                if let Err(answer) = self.check_write(path, &value) {
                    return self.refuse(&session, id, answer);
                }
                // Held by nobody, a hold set true would outlast its client.
                if duck::is_hold(path) && value == true && holder.is_none() {
                    let message = format!("{path} is momentary: touch it before setting it true");
                    return self.refuse(&session, id, Answer::Invalid { message });
                }
                let written = vec![(path.to_owned(), value)];
                if let Some((_, message)) = self.broken_duck(&written) {
                    return self.refuse(&session, id, Answer::Invalid { message });
                }
                if let Some(answer) = self.reroute_refusal(ctx, &written) {
                    return self.refuse(&session, id, answer);
                }
                self.seen(&session, path, seq);
                let values = self.write(written);
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

    fn patch(
        &mut self,
        session: &SessionKey,
        id: &str,
        changes: &[Write],
        ctx: &Apply,
    ) -> Effect<Edits> {
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
            // No session holds a patch's controls to release them.
            if duck::is_hold(&c.path) && c.value == true {
                let message = format!("{} is momentary: set it on the WebSocket", c.path);
                return self.refuse(session, id, Answer::Invalid { message });
            }
        }
        let written: Vec<(String, Value)> = changes
            .iter()
            .map(|c| (c.path.clone(), c.value.clone()))
            .collect();
        if let Some((_, message)) = self.broken_duck(&written) {
            return self.refuse(session, id, Answer::Invalid { message });
        }
        if let Some(answer) = self.reroute_refusal(ctx, &written) {
            return self.refuse(session, id, answer);
        }
        changed(self.write(written))
    }

    /// The first duck group whose triggers or targets `written` sets and
    /// that breaks its rules ([`duck::check`]) on the state the writes
    /// leave, with why.
    fn broken_duck(&self, written: &[(String, Value)]) -> Option<(String, String)> {
        let mut groups: Vec<&str> = written
            .iter()
            .filter_map(|(p, _)| duck::members_of(p))
            .collect();
        if groups.is_empty() {
            return None;
        }
        groups.sort_unstable();
        groups.dedup();
        let mut after = json!({ "ducks": self.state["ducks"].clone() });
        for (path, value) in written.iter().filter(|(p, _)| p.starts_with("/ducks/")) {
            set_at(&mut after, path, value.clone());
        }
        groups.into_iter().find_map(|g| {
            let e = duck::check(&after["ducks"], g).err()?;
            Some((g.to_owned(), e))
        })
    }

    fn scene_list(&self) -> Vec<Value> {
        self.state["scenes"].as_array().cloned().unwrap_or_default()
    }

    /// The part of the state a scene stores: everything an operator sets
    /// on channels, buses, auxes, mix-minus outputs and VCAs, the AFV
    /// bindings without the stage tally drives, and the duck groups
    /// without their mode and hold.
    fn scene_data(&self) -> Value {
        let mut data = Map::new();
        for group in [
            "channels", "buses", "auxes", "mixMinus", "vcas", "afv", "ducks",
        ] {
            data.insert(group.into(), self.state[group].clone());
        }
        let drop = |data: &mut Map<String, Value>, group: &str, keys: &[&str]| {
            let elements = data.get_mut(group).and_then(Value::as_object_mut);
            for element in elements.into_iter().flat_map(|e| e.values_mut()) {
                for key in keys {
                    element.as_object_mut().map(|e| e.remove(*key));
                }
            }
        };
        drop(&mut data, "afv", &["open"]);
        drop(&mut data, "ducks", &["mode", "hold"]);
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

    /// [`Product::expire`]: releases every control `session` holds and
    /// disconnects its consoles.
    fn end_lease(&mut self, session: &SessionKey) -> Effect<Edits> {
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
            ..Edits::default()
        })
    }

    /// Runs `f`, a command or a lease expiry, and keeps
    /// [`Mixer::fading`] for each gain target it changes: with a fade
    /// from `timing`, the gain ramps from the value it has at the fade's
    /// start, its running fade's value there or its old target, to the
    /// new target; without, it loses its fade, and the engine ramps it
    /// over the ramp length from the value it holds when the change
    /// applies. A gain left as it was keeps its fade. Every gain is the
    /// one the engine takes, an invalid one 0 ([`samplez_dsp::valid_gain`]).
    fn fading<T>(&mut self, timing: Timing, f: impl FnOnce(&mut Mixer) -> T) -> T {
        // Fades ended by the index the recall was received at, its
        // logged start less the lead, go: no replica applies the recall
        // earlier, so their gains hold the scene's values by then, and a
        // fade still running where the recall applies is not cut short.
        if let Timing::All(c) = timing {
            let received = c
                .start
                .saturating_sub(crossfade_lead(u64::from(self.shape().sample_rate)));
            for cell in self.fading.iter_mut().flatten() {
                if cell.is_some_and(|f| f.start + u64::from(f.len) <= received) {
                    *cell = None;
                }
            }
            if self.fading.iter().flatten().all(Option::is_none) {
                self.fading.clear();
            }
        }
        let opens = matches!(timing, Timing::Afv(_)).then(|| self.afv_opens());
        let before = match &self.derived {
            Some(d) if !matches!(timing, Timing::Cut) || !self.fading.is_empty() => {
                Some(d.targets().gains.clone())
            }
            _ => None,
        };
        let out = f(self);
        let (Some(before), Some(d)) = (before, &self.derived) else {
            return out;
        };
        let after = &d.targets().gains;
        // Each row's fade; none past the end.
        let timed = match (timing, opens) {
            (Timing::Afv(t), Some(opens)) => self.afv_fades(&opens, t),
            (Timing::All(c), _) => vec![Some(c); after.len()],
            _ => Vec::new(),
        };
        if self.fading.is_empty() {
            self.fading = after.iter().map(|row| vec![None; row.len()]).collect();
        }
        let valid = |g: f32| if samplez_dsp::valid_gain(g) { g } else { 0.0 };
        let rows = after.iter().zip(&before).zip(&mut self.fading);
        for (r, ((row, was), fades)) in rows.enumerate() {
            let fade = timed.get(r).copied().flatten();
            for ((&gain, &old), cell) in row.iter().zip(was).zip(fades) {
                if valid(gain).to_bits() == valid(old).to_bits() {
                    continue;
                }
                let old = valid(old);
                *cell = fade.map(|c| Fade {
                    from: cell.map_or(old, |f| f.value_at(old, c.start)),
                    start: c.start,
                    len: c.len,
                });
            }
        }
        if self.fading.iter().flatten().all(Option::is_none) {
            self.fading.clear();
        }
        out
    }

    /// Each channel's AFV stage open, by slot.
    fn afv_opens(&self) -> Vec<bool> {
        (self.shape().channels.iter())
            .map(|c| self.state["afv"][&c.id]["open"] == true)
            .collect()
    }

    /// The fade of each row of the gains for the tally `t`, the stages
    /// having been open as `opens` says before it: per model
    /// `AfvBinding`, a stage the tally opens rises from `t.start` over its
    /// `riseMs`, one it closes waits its `holdMs` from there and falls
    /// over its `fallMs`, `t.transitionMs` in place of both times. The
    /// rows of a channel whose stage stays, and the line-up tone's, get
    /// none.
    fn afv_fades(&self, opens: &[bool], t: Transition) -> Vec<Option<Crossfade>> {
        let rate = u64::from(self.shape().sample_rate);
        let samples = |ms: u64| ms * rate / 1000;
        let mut rows = Vec::new();
        for (c, &was) in self.shape().channels.iter().zip(opens) {
            let b = &self.state["afv"][&c.id];
            let open = b["open"] == true;
            let ms = |key: &str| b[key].as_f64().unwrap_or(0.0) as u64;
            let fade = (open != was).then(|| {
                let (hold, len) = if open {
                    (0, ms("riseMs"))
                } else {
                    (ms("holdMs"), ms("fallMs"))
                };
                let len = t.transition_ms.map_or(len, u64::from);
                Crossfade {
                    start: t.start + samples(hold),
                    len: samples(len) as u32,
                }
            });
            rows.extend(std::iter::repeat_n(fade, c.width));
        }
        rows
    }

    /// Recalls `scene`: every stored leaf whose recall-safe group its
    /// element does not keep, owned controls included, and
    /// `sceneRecalled`.
    fn recall(
        &mut self,
        session: &SessionKey,
        id: &str,
        scene: &str,
        at: &str,
        ctx: &Apply,
    ) -> Effect<Edits> {
        let Some((mut written, delays)) = self.recall_writes(scene) else {
            let message = format!("unknown scene {scene}");
            return self.refuse(session, id, Answer::NotFound { message });
        };
        if let Some(answer) = self.reroute_refusal(ctx, &written) {
            return self.refuse(session, id, answer);
        }
        if !delays.is_empty() {
            self.record(session, id, Answer::Recalled { delays });
        }
        written.push(("/sceneRecalled".into(), json!({ "id": scene, "at": at })));
        changed(self.write(written))
    }

    /// The leaves a recall of `scene` writes, none for an unknown scene,
    /// and the channels whose stored delay their delay line no longer
    /// takes: those recall a delay of 0.
    fn recall_writes(&self, scene: &str) -> Option<(Leaves, Vec<String>)> {
        let data = self.scenes.get(scene)?;
        let mut written = Vec::new();
        let mut short = Vec::new();
        for (path, mut v) in self.recalled(data) {
            let Some(current) = self.state.pointer(&path) else {
                continue;
            };
            // A scene keeps the ids and delays of the shape it was stored
            // in.
            if let Err(e) = self.check_references(&path, &v) {
                let Some(channel) = delayed_channel(&path) else {
                    continue;
                };
                tracing::warn!(scene, channel, "recall sets the delay to 0: {e}");
                short.push(channel);
                v = json!(0);
            }
            if *current != v {
                written.push((path, v));
            }
        }
        // With a channel added since the scene was stored, a group's
        // triggers can pass the limit: that group keeps its routing, as
        // if it were recall-safe, so its members and destinations stay a
        // pair an operator set.
        while let Some((group, e)) = self.broken_duck(&written) {
            tracing::warn!(
                scene,
                duck = group,
                "recall keeps the routing of the duck group: {e}"
            );
            written.retain(|(p, _)| duck::routing_of(p) != Some(group.as_str()));
        }
        Some((written, short))
    }

    /// The leaves of the scene `data` a recall writes: those of the groups
    /// recall-safe does not keep.
    fn recalled(&self, data: &Value) -> Vec<(String, Value)> {
        let mut written = Vec::new();
        for (group, elements) in data.as_object().into_iter().flatten() {
            for (element, value) in elements.as_object().into_iter().flatten() {
                let safe = &self.state["recallSafe"][element];
                for (path, v) in leaves(value, &format!("/{group}/{element}")) {
                    let Some(g) = recall_group(&path) else {
                        continue;
                    };
                    if !safe.as_array().is_some_and(|s| s.iter().any(|x| x == g)) {
                        written.push((path, v));
                    }
                }
            }
        }
        written
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

    /// Puts a request for `path` from `asker` in the log, addressed to the
    /// control's holder: nothing when nobody else holds it. Every replica
    /// applies it, and the one the holder's primary connection is on
    /// delivers it ([`crate::app::App::publish`]).
    fn control_request(
        &self,
        asker: &SessionKey,
        path: &str,
        from: &Value,
        at: u64,
    ) -> Effect<Edits> {
        let Some(holder) = self.owner_of(path) else {
            return Effect::Unchanged;
        };
        if holder == *asker {
            return Effect::Unchanged;
        }
        let to = json!({ "principal": holder.principal, "session": holder.session });
        Effect::Changed(Edits {
            requests: vec![json!({ "path": path, "from": from, "to": to, "at": at })],
            ..Edits::default()
        })
    }

    /// Counts one more reset in each of `ids`' measurements
    /// (`/loudness/<id>/resets`), the same on every replica; the engines
    /// restart the measurement at the delta's `atIndex`. An id that is no
    /// output refuses the whole command.
    fn reset_loudness(&mut self, session: &SessionKey, id: &str, ids: &[String]) -> Effect<Edits> {
        let shape = self.shape();
        if let Some(bad) = ids.iter().find(|i| !shape.is_output_bus(i)) {
            let message = format!("unknown bus {bad}");
            return self.refuse(session, id, Answer::NotFound { message });
        }
        let ids: std::collections::BTreeSet<&String> = ids.iter().collect();
        let written = (ids.into_iter())
            .map(|bus| {
                let n = self.state["loudness"][bus.as_str()]["resets"].as_u64();
                (
                    format!("/loudness/{}/resets", escape(bus)),
                    json!(n.unwrap_or(0) + 1),
                )
            })
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
        // Removed now, it would come back at its session's next
        // registration.
        if self.consoles[i].connected {
            let message = format!("console {console} is connected");
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
        Arc::make_mut(&mut self.state)["consoles"] = self.console_list();
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
                if let Some(channel) = delayed_channel(&path) {
                    tracing::warn!(channel, %value, "the delay line no longer takes the saved delay: 0");
                }
                dropped.push(path);
            }
        }
        for group in ["tally", "scenes", "sceneRecalled"] {
            state[group] = saved.state[group].clone();
        }
        self.state = Arc::new(state);
        self.owners = saved
            .owners
            .into_iter()
            .filter(|(p, _)| self.exists(p))
            .collect();
        self.dropped = dropped;
        self.merge_consoles(saved.consoles);
        // Connections of ports the configuration no longer has go.
        let mut ports = saved.ports;
        ports.receivers.retain(|id, _| shape.is_channel(id));
        ports.legs.retain(|id, _| shape.is_channel(id));
        // Leg flows activated over a configuration that wired the channel
        // otherwise are no longer what the operator configured.
        let wired = |id: &String| {
            let slot = shape.channels.iter().position(|c| c.id == *id)?;
            self.wiring.get(slot).map(|w| (w.a.clone(), w.b.clone()))
        };
        let stale: Vec<String> = (ports.legs.iter())
            .filter(|(id, l)| l.wired.is_some() && l.wired != wired(id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            ports.legs.remove(&id);
            ports.receivers.remove(&id);
        }
        let own_port = |p: &PortRef| match p {
            PortRef::Receiver(c) => shape.is_channel(c),
            PortRef::Sender(o) => shape.outputs.iter().any(|x| x.id == *o),
        };
        ports.scheduled.retain(|s| own_port(&s.port));
        ports
            .senders
            .retain(|id, _| shape.outputs.iter().any(|o| o.id == *id));
        self.ports = ports;
    }
}

/// The snapshot message's `shape`, `state` and `owners`.
fn snapshot_text(shape: &Shape, state: &Value, owners: &Value) -> String {
    json!({ "shape": shape, "state": state, "owners": owners }).to_string()
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

/// A snapshot an older binary saved has no surround position in the pan
/// of its channels: the initial one fills it in, as a configured state
/// does ([`Mixer::reconcile`]), so a write can reach it.
fn fill_pan(state: &mut Value) {
    let channels = state["channels"].as_object_mut().into_iter().flatten();
    for pan in channels.filter_map(|(_, c)| c.get_mut("pan")?.as_object_mut()) {
        for (leaf, initial) in [("frontRear", -1), ("spread", 1), ("lfeSend", -90)] {
            pan.entry(leaf).or_insert(json!(initial));
        }
    }
}

/// `written` and the leaves a loudness preset or target written among
/// them sets besides.
fn derived(written: Vec<(String, Value)>) -> Vec<(String, Value)> {
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
    all
}

/// An IFB interrupt or a duck hold, which goes back to off when
/// released.
fn momentary(path: &str) -> bool {
    path.starts_with("/mixMinus/") && path.ends_with("/interrupt/active") || duck::is_hold(path)
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

/// Leaves to write, as paths and values.
type Leaves = Vec<(String, Value)>;

/// The channel whose delay `path` is.
fn delayed_channel(path: &str) -> Option<String> {
    match tokens(path).as_slice() {
        [group, channel, leaf] if group == "channels" && leaf == "delay" => Some(channel.clone()),
        _ => None,
    }
}

/// The reference tokens of a JSON pointer, unescaped.
pub fn tokens(path: &str) -> Vec<String> {
    path.split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect()
}

/// The lowercase canonical UUID form MXL flow and NMOS ids take.
fn is_uuid(v: &str) -> bool {
    v.len() == 36
        && v.bytes().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(&c),
        })
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
        "ducks" => duck::recall_group(tokens.get(2..)?),
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
    use crate::strip;
    use samplez_dsp::StripParam;
    use samplez_replica::entry::Command as Entry;
    use samplez_service::Role;

    fn mixer() -> Mixer {
        Mixer::new(Shape::parse(&small()).unwrap())
    }

    /// A client snapshot built after the store lock is the one built
    /// under it, and holds the state it was taken at.
    #[test]
    fn a_snapshot_text_built_later_is_the_text_of_the_state_taken() {
        let mut m = mixer();
        set(&mut m, "a", 1, "/channels/ch1/fader", json!(-6));
        touch(&mut m, "a", 2, "/channels/ch1/fader");
        let before = m.text();
        assert!(before.contains(r#""/channels/ch1/fader""#), "{before}");
        let later = m.text_later().expect("the mixer builds its text later");
        set(&mut m, "a", 3, "/channels/ch1/fader", json!(-12));
        touch(&mut m, "b", 1, "/channels/ch2/fader");
        assert_eq!(later(), before);
        assert_ne!(m.text(), before);
    }

    /// A reader takes the state out of the store lock as a pointer: what
    /// it took stays as it was while writes go on, and the next reader
    /// sees them.
    #[test]
    fn a_shared_state_stays_as_taken_while_writes_go_on() {
        let mut m = mixer();
        let taken = m.shared_state();
        assert!(Arc::ptr_eq(&taken, &m.shared_state()), "a pointer copy");
        set(&mut m, "a", 1, "/channels/ch1/fader", json!(-6));
        assert_eq!(taken["channels"]["ch1"]["fader"], json!(-90));
        assert_eq!(m.shared_state()["channels"]["ch1"]["fader"], json!(-6));
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
            &Apply::default(),
        )
    }

    fn touch(m: &mut Mixer, session: &str, seq: u64, path: &str) -> Effect<Edits> {
        let id = format!("{session}-t{seq}");
        m.apply(
            &origin(session),
            &control(Action::Touch, &id, seq, path, None),
            &Apply::default(),
        )
    }

    fn release(m: &mut Mixer, session: &str, seq: u64, path: &str) -> Effect<Edits> {
        let id = format!("{session}-r{seq}");
        m.apply(
            &origin(session),
            &control(Action::Release, &id, seq, path, None),
            &Apply::default(),
        )
    }

    fn rest(m: &mut Mixer, command: Command) -> Effect<Edits> {
        m.apply(
            &origin("rest"),
            &serde_json::to_vec(&command).unwrap(),
            &Apply::default(),
        )
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

    /// A reset counts up in the state of each bus it names, once however
    /// often it is named, so every replica's engine restarts the
    /// measurement at the delta's index; an id that is no bus refuses the
    /// whole command and counts nothing.
    #[test]
    fn a_loudness_reset_is_counted_in_the_state() {
        let mut m = mixer();
        let reset = |m: &mut Mixer, id: &str, ids: &[&str]| {
            let ids = ids.iter().map(|i| (*i).to_owned()).collect();
            rest(m, Command::ResetLoudness { id: id.into(), ids })
        };
        assert_eq!(at(&m, "/loudness/pgm/resets"), &json!(0));
        let v = values(&reset(&mut m, "r1", &["pgm", "pgm", "cf"]));
        assert_eq!(
            v,
            vec![
                ("/loudness/cf/resets".to_owned(), json!(1)),
                ("/loudness/pgm/resets".to_owned(), json!(1)),
            ]
        );
        let v = values(&reset(&mut m, "r2", &["pgm"]));
        assert_eq!(v, vec![("/loudness/pgm/resets".to_owned(), json!(2))]);
        assert!(matches!(
            reset(&mut m, "r3", &["pgm", "ch1"]),
            Effect::Unchanged
        ));
        assert!(matches!(
            m.answer(&rest_key(), "r3"),
            Some(Answer::NotFound { .. })
        ));
        assert_eq!(at(&m, "/loudness/pgm/resets"), &json!(2));
        // A write of the count by a client is no control.
        assert!(matches!(
            m.check_write("/loudness/pgm/resets", &json!(0)),
            Err(Answer::Invalid { .. })
        ));
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
                    transition: None,
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
                transition: None,
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
                fade: None,
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
                    at: at_.into(),
                    fade: None,
                }
            ),
            Effect::Unchanged
        );
        assert!(matches!(
            m.answer(&rest_key(), "r2"),
            Some(Answer::NotFound { .. })
        ));
    }

    fn ctl(action: Action, path: &str, value: Option<Value>) -> Command {
        Command::Control(Control {
            id: "c".into(),
            action,
            kind: "ui".into(),
            seq: 1,
            path: path.into(),
            value,
        })
    }

    /// A state far from the initial one: AFV bound, a tally, a scene
    /// recalled and changed since, a talk held; and that scene.
    fn busy() -> (Mixer, String) {
        let mut m = mixer();
        patch(
            &mut m,
            "p1",
            &[
                ("/afv/ch1/enabled", json!(true)),
                ("/afv/ch1/source", json!("CAM1")),
                ("/afv/ch2/enabled", json!(true)),
                ("/afv/ch2/source", json!("CAM2")),
            ],
        );
        let scene = store(&mut m, "s1", None, "Opening");
        rest(
            &mut m,
            Command::Recall {
                id: "r1".into(),
                scene: scene.clone(),
                at: "2026-11-15T12:01:00.000Z".into(),
                fade: None,
            },
        );
        patch(
            &mut m,
            "p2",
            &[
                ("/channels/ch1/fader", json!(-10)),
                ("/channels/st1/legTrim/1", json!(3)),
                ("/afv/ch1/source", json!("CAM2")),
                ("/recallSafe/ch2", json!(["fader"])),
            ],
        );
        rest(
            &mut m,
            Command::Tally {
                id: "t1".into(),
                sources: BTreeMap::from([("CAM1".to_owned(), "program".to_owned())]),
                replace: false,
                transition: None,
            },
        );
        let talk = "/mixMinus/mm1/interrupt/active";
        touch(&mut m, "rest", 1, talk);
        set(&mut m, "rest", 2, talk, json!(true));
        touch(&mut m, "rest", 1, "/ducks/comm/hold");
        set(&mut m, "rest", 2, "/ducks/comm/hold", json!(true));
        (m, scene)
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
                fade: None,
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
                generation: None,
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
                ("/ducks/comm/channels/ch2/depth", json!(20)),
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
        assert!(
            dropped.contains(&"/ducks/comm/channels/ch2/depth".to_owned()),
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
                fade: None,
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
        set_at(Arc::make_mut(&mut m.state), "/afv/ch1/open", json!(false));
        let sources = BTreeMap::from([("CAM9".to_owned(), "program".to_owned())]);
        let v = values(&rest(
            &mut m,
            Command::Tally {
                id: "t".into(),
                sources,
                replace: false,
                transition: None,
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
                    replace: false,
                    transition: None,
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
                    replace: false,
                    transition: None,
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
                    replace: false,
                    transition: None,
                }
            ),
            Effect::Unchanged
        );
        assert!(matches!(
            m.answer(&rest_key(), "t3"),
            Some(Answer::Invalid { .. })
        ));
    }

    fn origin_of(principal: &str, session: &str) -> Origin {
        Origin {
            principal: principal.into(),
            ..origin(session)
        }
    }

    /// A control command of `kind` from `principal`'s `session`.
    fn act(
        m: &mut Mixer,
        (principal, session, kind): (&str, &str, &str),
        action: Action,
        seq: u64,
        path: &str,
        value: Option<Value>,
    ) -> Effect<Edits> {
        let command = Command::Control(Control {
            id: format!("{session}-{seq}"),
            action,
            kind: kind.into(),
            seq,
            path: path.into(),
            value,
        });
        m.apply(
            &origin_of(principal, session),
            &serde_json::to_vec(&command).unwrap(),
            &Apply::default(),
        )
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
        m.apply(
            &origin(session),
            &serde_json::to_vec(&command).unwrap(),
            &Apply::default(),
        )
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
        // Connected, it would come back at the next registration.
        assert_eq!(remove(&mut m, "d0", "desk"), Effect::Unchanged);
        let Some(Answer::Refused { message }) = m.answer(&rest_key(), "d0") else {
            panic!("{:?}", m.answer(&rest_key(), "d0"))
        };
        assert_eq!(message, "console desk is connected");
        assert_eq!(consoles(&m).len(), 1);
        m.expire(&origin("a").key());
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

    /// A console takes a control its principal holds on a screen: a touch
    /// moves the ownership, a set writes and leaves it.
    #[test]
    fn a_console_takes_over_a_control_its_principal_holds_on_a_screen() {
        let mut m = mixer();
        let path = "/channels/ch1/fader";
        let screen = ("alice", "screen", "ui");
        let surface = ("alice", "surface", "console");
        act(&mut m, screen, Action::Touch, 1, path, None);
        let Effect::Changed(e) = act(&mut m, surface, Action::Set, 1, path, Some(json!(-6))) else {
            panic!("the set is written")
        };
        assert_eq!(e.values, vec![(path.to_owned(), json!(-6))]);
        assert!(e.owners.is_empty());
        assert_eq!(m.owners()[path]["handle"], handle("screen"));
        let Effect::Changed(e) = act(&mut m, surface, Action::Touch, 2, path, None) else {
            panic!("the touch takes it")
        };
        assert_eq!(e.owners[0].1["handle"], handle("surface"));
        assert_eq!(e.owners[0].1["kind"], "console");
        // The screen does not take it back.
        assert_eq!(
            act(&mut m, screen, Action::Touch, 2, path, None),
            Effect::Unchanged
        );
        assert!(matches!(
            m.answer(&origin_of("alice", "screen").key(), "screen-2"),
            Some(Answer::Conflict { .. })
        ));
    }

    /// Another principal's console, and every anonymous console, still
    /// gets 409; a console does not take from another console.
    #[test]
    fn no_takeover_across_principals_consoles_or_anonymous_clients() {
        let refused = |holder: (&str, &str, &str), taker: (&str, &str, &str)| {
            let mut m = mixer();
            let path = "/channels/ch1/fader";
            act(&mut m, holder, Action::Touch, 1, path, None);
            for (seq, action) in [(1, Action::Set), (2, Action::Touch)] {
                let value = (action == Action::Set).then(|| json!(-6));
                assert_eq!(
                    act(&mut m, taker, action, seq, path, value),
                    Effect::Unchanged
                );
                let key = origin_of(taker.0, taker.1).key();
                assert!(
                    matches!(
                        m.answer(&key, &format!("{}-{seq}", taker.1)),
                        Some(Answer::Conflict { .. })
                    ),
                    "{holder:?} {taker:?} {action:?}"
                );
            }
            assert_eq!(m.owners()[path]["handle"], handle(holder.1));
        };
        refused(("alice", "screen", "ui"), ("bob", "surface", "console"));
        refused(
            ("anonymous", "screen", "ui"),
            ("anonymous", "surface", "console"),
        );
        refused(
            ("alice", "desk", "console"),
            ("alice", "surface", "console"),
        );
        refused(("alice", "screen", "ui"), ("alice", "other", "ui"));
    }
    /// The small shape with six more mono channels: nine in all.
    fn nine_channels() -> Mixer {
        let mut v = small();
        let more = (4..=9).map(|n| {
            json!({ "id": format!("ch{n}"), "name": "C", "width": 1, "layout": "mono", "altInput": false })
        });
        v["channels"].as_array_mut().unwrap().extend(more);
        Mixer::new(Shape::parse(&v).unwrap())
    }

    fn invalid(m: &Mixer, session: &str, id: &str) -> String {
        match m.answer(&origin(session).key(), id) {
            Some(Answer::Invalid { message }) => message.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// Every leaf of a duck group is a control with its range; ids it
    /// does not have are not found.
    #[test]
    fn duck_writes_take_their_ranges_and_the_shapes_ids() {
        let m = mixer();
        let ok = |path: &str, value: Value| m.check_write(path, &value).unwrap();
        let refused = |path: &str, value: Value| m.check_write(path, &value).unwrap_err();
        ok("/ducks/comm/mode", json!("off"));
        ok("/ducks/comm/keyTap", json!("postFader"));
        ok("/ducks/comm/threshold", json!(-60));
        ok("/ducks/comm/keyLpf/freq", json!(20000));
        ok("/ducks/comm/channels/st1/depth", json!(40));
        ok("/ducks/comm/destinations/mm1", json!(true));
        ok("/recallSafe/comm", json!(["routing"]));
        for (path, value) in [
            ("/ducks/comm/mode", json!("bypass")),
            ("/ducks/comm/threshold", json!(-61)),
            ("/ducks/comm/hysteresis", json!(13)),
            ("/ducks/comm/keyHpf/freq", json!(501)),
            ("/ducks/comm/keyLpf/freq", json!(999)),
            ("/ducks/comm/attackMs", json!(0.5)),
            ("/ducks/comm/holdMs", json!(5001)),
            ("/ducks/comm/releaseMs", json!(9)),
            ("/ducks/comm/channels/ch1/depth", json!(41)),
            ("/ducks/comm/channels/ch1/trigger", json!(1)),
        ] {
            let e = refused(path, value.clone());
            assert!(matches!(e, Answer::Invalid { .. }), "{path} {value}: {e:?}");
        }
        for path in [
            "/ducks/nope/mode",
            "/ducks/comm/channels/ch9/trigger",
            "/ducks/comm/destinations/mon",
            "/ducks/comm/channels",
        ] {
            let e = refused(path, json!(true));
            assert!(matches!(e, Answer::NotFound { .. }), "{path}: {e:?}");
        }
    }

    /// A channel is never trigger and target of one group, whichever is
    /// written first or in one patch; the rule reads the state the writes
    /// leave, so a patch swapping both is taken.
    #[test]
    fn a_channel_is_not_trigger_and_target_of_one_group() {
        let mut m = mixer();
        let trigger = "/ducks/comm/channels/ch1/trigger";
        let target = "/ducks/comm/channels/ch1/target";
        values(&set(&mut m, "a", 1, trigger, json!(true)));
        assert_eq!(set(&mut m, "a", 1, target, json!(true)), Effect::Unchanged);
        let e = invalid(&m, "a", "a-1");
        assert!(e.contains("trigger and target"), "{e}");
        assert_eq!(*at(&m, target), json!(false));
        assert_eq!(
            patch(&mut m, "p1", &[(target, json!(true))]),
            Effect::Unchanged
        );
        assert!(matches!(
            m.answer(&rest_key(), "p1"),
            Some(Answer::Invalid { .. })
        ));
        values(&patch(
            &mut m,
            "p2",
            &[(trigger, json!(false)), (target, json!(true))],
        ));
        assert_eq!(
            (at(&m, trigger), at(&m, target)),
            (&json!(false), &json!(true))
        );
        // Another channel triggers the same group.
        values(&set(
            &mut m,
            "a",
            2,
            "/ducks/comm/channels/ch2/trigger",
            json!(true),
        ));
    }

    /// A group keys from at most eight triggers; the ninth is refused, by
    /// a set or a patch, and a patch carrying it applies nothing.
    #[test]
    fn a_ninth_trigger_is_refused() {
        let mut m = nine_channels();
        let ids = ["ch1", "ch2", "st1", "ch4", "ch5", "ch6", "ch7", "ch8"];
        let path = |c: &str| format!("/ducks/comm/channels/{c}/trigger");
        let eight: Vec<(String, Value)> = ids.iter().map(|c| (path(c), json!(true))).collect();
        let eight: Vec<(&str, Value)> =
            eight.iter().map(|(p, v)| (p.as_str(), v.clone())).collect();
        values(&patch(&mut m, "p1", &eight));
        assert_eq!(
            set(&mut m, "a", 1, &path("ch9"), json!(true)),
            Effect::Unchanged
        );
        let e = invalid(&m, "a", "a-1");
        assert!(e.contains("at most 8 triggers"), "{e}");
        let (ch1, ch9) = (path("ch1"), path("ch9"));
        let ninth = [
            (ch9.as_str(), json!(true)),
            ("/ducks/comm/threshold", json!(-40)),
        ];
        assert_eq!(patch(&mut m, "p2", &ninth), Effect::Unchanged);
        assert_eq!(
            *at(&m, "/ducks/comm/threshold"),
            json!(-30),
            "nothing applied"
        );
        let swap = [(ch1.as_str(), json!(false)), (ch9.as_str(), json!(true))];
        values(&patch(&mut m, "p3", &swap));
    }

    /// The hold is momentary: only the session holding it sets it true,
    /// a second operator is refused with the holder named, and it goes
    /// back to false on release or when the lease ends. A patch, which
    /// no session holds, cannot set it.
    #[test]
    fn a_duck_hold_is_momentary() {
        let mut m = mixer();
        let hold = "/ducks/comm/hold";
        assert_eq!(set(&mut m, "a", 1, hold, json!(true)), Effect::Unchanged);
        assert!(invalid(&m, "a", "a-1").contains("momentary"));
        assert_eq!(
            patch(&mut m, "p1", &[(hold, json!(true))]),
            Effect::Unchanged
        );
        assert!(matches!(
            m.answer(&rest_key(), "p1"),
            Some(Answer::Invalid { .. })
        ));
        touch(&mut m, "a", 2, hold);
        values(&set(&mut m, "a", 3, hold, json!(true)));
        assert_eq!(touch(&mut m, "b", 1, hold), Effect::Unchanged);
        let Some(Answer::Conflict { conflicts }) = m.answer(&origin("b").key(), "b-t1") else {
            panic!()
        };
        assert_eq!(conflicts[0]["owner"]["handle"], handle("a"));
        assert_eq!(conflicts[0]["current"], json!(true));
        let Effect::Changed(e) = release(&mut m, "a", 4, hold) else {
            panic!()
        };
        assert_eq!(e.values, vec![(hold.to_owned(), json!(false))]);
        touch(&mut m, "b", 2, hold);
        set(&mut m, "b", 3, hold, json!(true));
        let Effect::Changed(e) = m.expire(&origin("b").key()) else {
            panic!()
        };
        assert_eq!(e.values, vec![(hold.to_owned(), json!(false))]);
        assert_eq!(*at(&m, hold), json!(false));
    }

    /// A scene stores the duck setup without mode and hold; a recall
    /// writes membership and destinations under routing and the detector
    /// and depths under dynamics, honours recall-safe, and never touches
    /// mode or hold.
    #[test]
    fn a_recall_restores_the_duck_setup_but_never_mode_or_hold() {
        let mut m = mixer();
        patch(
            &mut m,
            "p1",
            &[
                ("/ducks/comm/channels/ch1/trigger", json!(true)),
                ("/ducks/comm/channels/st1/target", json!(true)),
                ("/ducks/comm/channels/st1/depth", json!(15)),
                ("/ducks/comm/destinations/aux1", json!(true)),
                ("/ducks/comm/threshold", json!(-40)),
                ("/ducks/comm/mode", json!("on")),
            ],
        );
        let scene = store(&mut m, "s1", None, "Commentary");
        let stored = &m.scenes[&scene]["ducks"]["comm"];
        assert!(
            stored.get("mode").is_none() && stored.get("hold").is_none(),
            "{stored}"
        );
        patch(
            &mut m,
            "p2",
            &[
                ("/ducks/comm/channels/ch1/trigger", json!(false)),
                ("/ducks/comm/channels/st1/target", json!(false)),
                ("/ducks/comm/channels/st1/depth", json!(6)),
                ("/ducks/comm/destinations/aux1", json!(false)),
                ("/ducks/comm/threshold", json!(-20)),
                ("/ducks/comm/mode", json!("off")),
            ],
        );
        touch(&mut m, "a", 1, "/ducks/comm/hold");
        set(&mut m, "a", 2, "/ducks/comm/hold", json!(true));
        let recall = |m: &mut Mixer, id: &str| {
            values(&rest(
                m,
                Command::Recall {
                    id: id.into(),
                    scene: scene.clone(),
                    at: "2026-11-15T12:01:00.000Z".into(),
                    fade: None,
                },
            ))
        };
        let v = recall(&mut m, "r1");
        for (path, value) in [
            ("/ducks/comm/channels/ch1/trigger", json!(true)),
            ("/ducks/comm/channels/st1/target", json!(true)),
            ("/ducks/comm/channels/st1/depth", json!(15)),
            ("/ducks/comm/destinations/aux1", json!(true)),
            ("/ducks/comm/threshold", json!(-40)),
        ] {
            assert!(v.contains(&(path.to_owned(), value)), "{path}: {v:?}");
        }
        assert_eq!(*at(&m, "/ducks/comm/mode"), json!("off"));
        assert_eq!(*at(&m, "/ducks/comm/hold"), json!(true));

        patch(
            &mut m,
            "p3",
            &[
                ("/ducks/comm/channels/ch1/trigger", json!(false)),
                ("/ducks/comm/channels/st1/depth", json!(6)),
                ("/ducks/comm/threshold", json!(-20)),
                ("/recallSafe/comm", json!(["dynamics"])),
            ],
        );
        let v = recall(&mut m, "r2");
        assert!(v.contains(&("/ducks/comm/channels/ch1/trigger".into(), json!(true))));
        assert_eq!(*at(&m, "/ducks/comm/threshold"), json!(-20), "recall-safe");
        assert_eq!(
            *at(&m, "/ducks/comm/channels/st1/depth"),
            json!(6),
            "recall-safe"
        );
    }

    /// A recall that would bring a group past eight triggers, with eight
    /// in the scene and a channel the scene does not know triggering,
    /// leaves that group's routing (members and destinations) as it is,
    /// recalls the rest and says so in the log.
    #[test]
    fn a_recall_keeps_the_routing_of_a_group_it_would_overfill() {
        let mut m = nine_channels();
        let scene = store(&mut m, "s1", None, "Opening");
        let data = m.scenes.get_mut(&scene).unwrap();
        let channels = data["ducks"]["comm"]["channels"].as_object_mut().unwrap();
        channels.remove("ch9");
        for c in channels.values_mut() {
            c["trigger"] = json!(true);
        }
        data["ducks"]["comm"]["channels"]["ch1"]["depth"] = json!(30);
        data["ducks"]["comm"]["threshold"] = json!(-45);
        data["ducks"]["comm"]["destinations"]["aux1"] = json!(true);
        values(&set(
            &mut m,
            "a",
            1,
            "/ducks/comm/channels/ch9/trigger",
            json!(true),
        ));
        let log = Log::default();
        let subscriber = tracing_subscriber::fmt().with_writer(log.clone()).finish();
        let v = tracing::subscriber::with_default(subscriber, || {
            values(&rest(
                &mut m,
                Command::Recall {
                    id: "r".into(),
                    scene: scene.clone(),
                    at: "2026-11-15T12:01:00.000Z".into(),
                    fade: None,
                },
            ))
        });
        assert!(!v.iter().any(|(p, _)| p.ends_with("/trigger")), "{v:?}");
        assert_eq!(*at(&m, "/ducks/comm/destinations/aux1"), json!(false));
        let log = log.text();
        assert!(
            log.contains("WARN") && log.contains(&scene) && log.contains("duck=\"comm\""),
            "{log}"
        );
        assert!(v.contains(&("/ducks/comm/channels/ch1/depth".into(), json!(30))));
        assert!(v.contains(&("/ducks/comm/threshold".into(), json!(-45))));
        duck::check(&m.state()["ducks"], "comm").unwrap();
    }

    /// A log in memory.
    #[derive(Clone, Default)]
    struct Log(Arc<std::sync::Mutex<Vec<u8>>>);

    impl Log {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl std::io::Write for Log {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
        type Writer = Log;

        fn make_writer(&'a self) -> Log {
            self.clone()
        }
    }

    /// A snapshot taken before duck groups were kept installs with none.
    #[test]
    fn a_snapshot_without_duck_groups_installs_with_none() {
        let mut v = small();
        v.as_object_mut().unwrap().remove("ducks");
        let old = Mixer::new(Shape::parse(&v).unwrap());
        // Taken before snapshots carried their format, too.
        let mut snapshot: Value = serde_json::from_slice(&old.save()).unwrap();
        let mut saved = snapshot["mixer"].take();
        saved["state"].as_object_mut().unwrap().remove("ducks");
        saved["shape"].as_object_mut().unwrap().remove("ducks");
        let mut installed = Mixer::default();
        installed
            .load(&serde_json::to_vec(&saved).unwrap())
            .unwrap();
        assert_eq!(installed.state()["ducks"], json!({}));
        check(&contract().state, installed.state()).unwrap();
    }

    /// The small shape with input b on both mono channels, its channels
    /// wired to flows `a<n>` and `b<n>`: ch1 and st1 in domain d, ch2 in
    /// domain e.
    fn wired() -> Mixer {
        let mut shape = small();
        shape["channels"][1]["altInput"] = json!(true);
        let wire = |a: &str, b: Option<&str>, domain: &str| Wiring {
            a: a.into(),
            b: b.map(Into::into),
            domain: domain.into(),
        };
        Mixer::configured(
            Shape::parse(&shape).unwrap(),
            Vec::new(),
            vec![
                wire("a1", Some("b1"), "d"),
                wire("a2", Some("b2"), "e"),
                wire("a3", None, "d"),
            ],
        )
    }

    fn ordered(generation: Option<bool>) -> Apply {
        Apply {
            generation,
            at_index: None,
        }
    }

    /// The flows of channels reading `ids`, then the slots of the line-up
    /// tone's mono and stereo generators, which read none.
    fn flows(ids: &[&str]) -> Vec<Option<String>> {
        (ids.iter().map(|id| Some((*id).to_owned())))
            .chain([None, None])
            .collect()
    }

    fn gain_bits(t: &Targets) -> Vec<Vec<u32>> {
        let row = |r: &Vec<f32>| r.iter().map(|g| g.to_bits()).collect();
        t.gains.iter().map(row).collect()
    }

    /// The targets of a busy state are what the engine of a set of one is
    /// sent for it: the gain matrix, each channel's strip and the flow of
    /// its selected input, bit for bit, also after a save and load.
    #[test]
    fn the_targets_are_what_the_engine_is_sent() {
        let mut m = wired();
        patch(
            &mut m,
            "p1",
            &[
                ("/vcas/vca1/members", json!(["ch1", "st1"])),
                ("/vcas/vca1/offset", json!(-6)),
                ("/channels/ch1/fader", json!(-10)),
                ("/channels/ch1/sends/aux1/on", json!(true)),
                ("/channels/ch1/sends/aux1/level", json!(-3)),
                ("/channels/ch1/gate/enabled", json!(true)),
                ("/channels/ch1/gate/threshold", json!(-40)),
                ("/channels/st1/legTrim/1", json!(3)),
            ],
        );
        let scene = store(&mut m, "s1", None, "Opening");
        patch(
            &mut m,
            "p2",
            &[
                ("/channels/ch1/fader", json!(0)),
                ("/channels/ch2/mute", json!(true)),
                ("/channels/ch1/input", json!("b")),
            ],
        );
        rest(
            &mut m,
            Command::Recall {
                id: "r1".into(),
                scene,
                at: "2026-11-15T12:01:00.000Z".into(),
                fade: None,
            },
        );
        assert_eq!(at(&m, "/channels/ch1/fader"), &json!(-10), "recalled");
        set(&mut m, "s", 1, "/channels/ch1/input", json!("b"));
        let t = m.targets().unwrap();
        let shape = m.shape();
        let strips: Vec<Vec<StripParam>> = (shape.channels.iter())
            .map(|c| strip::params(&m.state()["channels"][&c.id], c.width))
            .chain([Vec::new(), Vec::new()])
            .collect();
        assert_eq!(
            gain_bits(&t),
            gain_bits(&Targets {
                gains: mix::matrix(shape, m.state()),
                ..Targets::default()
            })
        );
        assert_eq!(format!("{:?}", t.strips), format!("{strips:?}"));
        assert_eq!(t.flows, flows(&["b1", "a2", "a3"]));
        // ch1 under the VCA, -10 - 6 dB, panned centre onto PGM left.
        let pgm_left = (10f64.powf(-16.0 / 20.0) * std::f64::consts::FRAC_PI_4.cos()) as f32;
        assert_eq!(t.gains[0][0].to_bits(), pgm_left.to_bits());
        assert!(t.gains[1].iter().all(|g| *g == 0.0), "ch2 is muted");
        let mut loaded = wired();
        loaded.load(&m.save()).unwrap();
        let back = loaded.targets().unwrap();
        assert_eq!(gain_bits(&back), gain_bits(&t));
        assert_eq!(format!("{back:?}"), format!("{t:?}"));
    }

    /// A mixer without an engine's wiring has no targets.
    #[test]
    fn no_wiring_no_targets() {
        assert_eq!(mixer().targets(), None);
        assert!(wired().targets().is_some());
    }

    /// An AFV stage closing and opening by tally, and its off level set
    /// while closed, move its channel's rows in the targets kept, to
    /// those of the whole state.
    #[test]
    fn an_afv_stage_moves_its_channels_rows() {
        let mut m = wired();
        let kept = |m: &Mixer| {
            let kept = m.targets().unwrap();
            let whole = mix::targets(m.shape(), &m.wiring, m.state());
            assert_eq!(gain_bits(&kept), gain_bits(&whole));
            kept.gains
        };
        let tally = |m: &mut Mixer, source: &str, state: &str| {
            rest(
                m,
                Command::Tally {
                    id: format!("t-{source}-{state}"),
                    sources: [(source.to_owned(), state.to_owned())].into(),
                    replace: false,
                    transition: None,
                },
            )
        };
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let pgm_left_is = |g: f32, db: f64| (f64::from(g) - h * mix::gain(db)).abs() < 1e-6;
        patch(
            &mut m,
            "p1",
            &[
                ("/channels/ch1/fader", json!(0)),
                ("/channels/ch2/fader", json!(0)),
                ("/afv/ch1/enabled", json!(true)),
                ("/afv/ch1/offLevel", json!(-20)),
                ("/afv/ch1/source", json!("cam1")),
                ("/afv/ch2/enabled", json!(true)),
                ("/afv/ch2/offLevel", json!(-30)),
                ("/afv/ch2/source", json!("cam2")),
            ],
        );
        assert!(pgm_left_is(kept(&m)[0][0], 0.0));
        tally(&mut m, "cam1", "preview");
        assert_eq!(at(&m, "/afv/ch1/open"), &json!(false));
        assert!(pgm_left_is(kept(&m)[0][0], -20.0), "closed by tally");
        tally(&mut m, "cam1", "program");
        assert!(pgm_left_is(kept(&m)[0][0], 0.0), "opened by tally");
        tally(&mut m, "cam2", "preview");
        assert!(pgm_left_is(kept(&m)[1][0], -30.0), "closed by tally");
        set(&mut m, "s", 1, "/afv/ch2/offLevel", json!(-10));
        let gains = kept(&m);
        assert!(pgm_left_is(gains[1][0], -10.0), "off level set");
        assert!(pgm_left_is(gains[0][0], 0.0), "ch1 stays open");
    }

    /// [`wide_with`] with ch4 a tone channel at 440 Hz, -20 dBFS, wired
    /// to no flow.
    fn wide_toned(fold: bool) -> Mixer {
        let m = wide_with(fold);
        let mut shape = m.shape().clone();
        let slot = (shape.channels.iter().position(|c| c.id == "ch4")).unwrap();
        shape.channels[slot].alt_input = false;
        shape.channels[slot].tone = Some(crate::shape::ChannelTone {
            frequency: 440,
            level: -20.0,
        });
        let mut wiring = m.wiring.clone();
        wiring[slot] = Wiring {
            a: String::new(),
            b: None,
            domain: String::new(),
        };
        Mixer::configured(shape, Vec::new(), wiring)
    }

    /// The small shape with more channels, a 5.1 one among them, a
    /// second VCA, a stereo aux and a direct out, each channel wired to
    /// flows of its own in one domain.
    /// With `fold` its programme bus is 5.1, carried by a 6-leg output
    /// and a stereo one that folds it down.
    fn wide_with(fold: bool) -> Mixer {
        let mut shape = small();
        if fold {
            shape["buses"][0] = json!({ "id": "pgm", "name": "PGM", "kind": "program", "width": 6, "layout": "5.1" });
            shape["mixMinus"] = json!([]);
            shape["outputs"] = json!([
                { "id": "out-51", "name": "PGM 5.1", "source": "pgm", "width": 6, "layout": "5.1" },
                { "id": "out-lr", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
            ]);
        }
        let channel = |id: &str, width: usize, layout: &str| json!({ "id": id, "name": id, "width": width, "layout": layout, "altInput": true });
        let more = [
            channel("ch3", 1, "mono"),
            channel("st2", 2, "stereo"),
            channel("c51", 6, "5.1"),
            channel("ch4", 1, "mono"),
        ];
        shape["channels"].as_array_mut().unwrap().extend(more);
        shape["channels"][3]["maxDelay"] = json!(960);
        shape["channels"][5]["maxDelay"] = json!(48_000);
        shape["vcas"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "id": "vca2", "name": "MUSIC" }));
        shape["auxes"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "id": "aux2", "name": "FX", "width": 2, "layout": "stereo" }));
        shape["outputs"].as_array_mut().unwrap().push(
            json!({ "id": "o-ch1", "name": "DO 1", "source": "ch1", "width": 1, "layout": "mono" }),
        );
        let shape = Shape::parse(&shape).unwrap();
        let wiring = (shape.channels.iter())
            .map(|c| Wiring {
                a: format!("a-{}", c.id),
                b: c.alt_input.then(|| format!("b-{}", c.id)),
                domain: "d".into(),
            })
            .collect();
        Mixer::configured(shape, Vec::new(), wiring)
    }

    /// The targets a mixer keeps from write to write are those derived
    /// from its whole state, bit for bit, after every command of random
    /// sequences: sets, patches, touches and releases, scenes stored and
    /// recalled, tally, expiries, refused commands and a save and load;
    /// on the shape of [`wide_with`] and on one whose programme bus a stereo
    /// output folds down, a duck group among its destinations.
    #[test]
    fn the_kept_targets_are_those_of_the_whole_state_bit_for_bit() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let channels = ["ch1", "ch2", "st1", "ch3", "st2", "c51", "ch4"];
        let elements = [
            "buses/pgm",
            "buses/cf",
            "auxes/aux1",
            "auxes/aux2",
            "mixMinus/mm1",
        ];
        let sources = ["pgm", "cf", "aux1", "aux2", "mm1"];
        // One random leaf the targets read, or one beside them.
        let leaf = |next: &mut dyn FnMut(u64) -> u64| -> (String, Value) {
            let ch = channels[next(channels.len() as u64) as usize];
            let db = json!(next(1100) as f64 / 10.0 - 90.0);
            let flag = json!(next(2) == 0);
            let some = |next: &mut dyn FnMut(u64) -> u64| -> Value {
                json!(channels.iter().filter(|_| next(3) == 0).collect::<Vec<_>>())
            };
            match next(42) {
                0 => (format!("/channels/{ch}/fader"), db),
                1 => (format!("/channels/{ch}/mute"), flag),
                2 => (
                    format!("/channels/{ch}/pan/lr"),
                    json!(next(201) as f64 / 100.0 - 1.0),
                ),
                3 => (
                    format!("/channels/{ch}/routes/{}", ["pgm", "cf"][next(2) as usize]),
                    flag,
                ),
                4 => {
                    let aux = ["aux1", "aux2"][next(2) as usize];
                    let key = ["on", "level", "pre", "pan"][next(4) as usize];
                    let value = match key {
                        "level" => db,
                        "pan" => json!(next(201) as f64 / 100.0 - 1.0),
                        _ => flag,
                    };
                    (format!("/channels/{ch}/sends/{aux}/{key}"), value)
                }
                5 => (
                    format!("/channels/{ch}/directOut/tap"),
                    json!(["input", "preFader", "postFader"][next(3) as usize]),
                ),
                6 => (format!("/vcas/vca{}/members", 1 + next(2)), some(next)),
                7 => (
                    format!("/vcas/vca{}/offset", 1 + next(2)),
                    json!(next(400) as f64 / 10.0 - 30.0),
                ),
                8 => (format!("/vcas/vca{}/mute", 1 + next(2)), flag),
                9 => (format!("/{}/fader", elements[next(5) as usize]), db),
                10 => (format!("/{}/mute", elements[next(5) as usize]), flag),
                11 => (
                    format!("/{}/polarity/{}", elements[next(4) as usize], next(2)),
                    flag,
                ),
                12 => ("/mixMinus/mm1/exclude".into(), some(next)),
                13 => (
                    format!("/afv/{ch}/{}", ["enabled", "open"][next(2) as usize]),
                    flag,
                ),
                14 => (format!("/afv/{ch}/source"), json!("cam1")),
                15 => (format!("/afv/{ch}/offLevel"), json!(-(next(60) as f64))),
                16 => ("/monitor/source".into(), json!(sources[next(5) as usize])),
                17 => {
                    let key = [
                        "level",
                        "dim",
                        "cut",
                        "mono",
                        "downmix",
                        "talkbackDim",
                        "dimDepth",
                    ][next(7) as usize];
                    let value = match key {
                        "level" => db,
                        "dimDepth" => json!(next(40) as f64),
                        "downmix" => json!(["lr", "loRo"][next(2) as usize]),
                        _ => flag,
                    };
                    (format!("/monitor/{key}"), value)
                }
                18 => (
                    format!(
                        "/monitor/{}/{}",
                        ["speakerMute", "speakerSolo"][next(2) as usize],
                        next(2)
                    ),
                    flag,
                ),
                19 => (
                    format!("/channels/{ch}/trim"),
                    json!(next(41) as f64 - 20.0),
                ),
                20 => (
                    format!("/channels/{ch}/legTrim/0"),
                    json!(next(13) as f64 - 6.0),
                ),
                21 => (format!("/channels/{ch}/polarity/0"), flag),
                22 => (format!("/channels/{ch}/hpf/enabled"), flag),
                23 => (
                    format!("/channels/{ch}/eq/bands/{}/gain", next(4)),
                    json!(next(31) as f64 - 15.0),
                ),
                24 => (
                    format!("/channels/{ch}/gate/threshold"),
                    json!(-(next(80) as f64)),
                ),
                25 => (format!("/channels/{ch}/comp/enabled"), flag),
                26 => (format!("/channels/{ch}/dynamicsBeforeEq"), flag),
                27 => (
                    format!("/channels/{ch}/input"),
                    json!(["a", "b"][next(2) as usize]),
                ),
                28 => (format!("/channels/{ch}/meterTap"), json!("preFader")),
                29 | 30 => {
                    let key = ["trigger", "target"][next(2) as usize];
                    (format!("/ducks/comm/channels/{ch}/{key}"), flag)
                }
                31 => (format!("/ducks/comm/channels/{ch}/depth"), json!(next(41))),
                32 => (
                    format!("/ducks/comm/destinations/{}", sources[next(5) as usize]),
                    flag,
                ),
                33 => (
                    "/ducks/comm/keyTap".into(),
                    json!(["preFader", "preFaderOpen", "postFader"][next(3) as usize]),
                ),
                34 => {
                    let at = [format!("channels/{ch}"), format!("vcas/vca{}", 1 + next(2))];
                    let mode = ["off", "off", "pfl", "afl"][next(4) as usize];
                    (format!("/{}/solo", at[next(2) as usize]), json!(mode))
                }
                35 => {
                    let source = channels.get(next(channels.len() as u64 + 1) as usize);
                    ("/mixMinus/mm1/interrupt/source".into(), json!(source))
                }
                36 => ("/mixMinus/mm1/interrupt/active".into(), flag),
                37 => (
                    if next(2) == 0 {
                        format!("/channels/{ch}/image")
                    } else {
                        "/monitor/image".into()
                    },
                    json!(["normal", "leftToBoth", "rightToBoth", "swap"][next(4) as usize]),
                ),
                // Within ch3's and c51's lines; any but 0 is refused on
                // the others.
                38 => (format!("/channels/{ch}/delay"), json!(next(3) * 480)),
                // ch4 is a tone channel; the others have no tone to write.
                39 => match next(3) {
                    0 => (
                        format!("/channels/{ch}/tone/frequency"),
                        json!(20 + next(19_981)),
                    ),
                    1 => (
                        format!("/channels/{ch}/tone/level"),
                        json!(-(next(601) as f64) / 10.0),
                    ),
                    _ => (format!("/channels/{ch}/tone/ident"), flag),
                },
                // The line-up tone, on any of the elements it can replace.
                40 => match next(4) {
                    0 => ("/tone/on".into(), flag),
                    1 => (
                        "/tone/targets".into(),
                        json!(sources.iter().filter(|_| next(2) == 0).collect::<Vec<_>>()),
                    ),
                    2 => ("/tone/level".into(), json!(-(next(301) as f64) / 10.0)),
                    _ => ("/tone/ident".into(), flag),
                },
                _ => ("/metering/loudness/preset".into(), json!("atsc")),
            }
        };
        let mut seq = 0;
        let mut changed = 0;
        for round in 0..150 {
            let fold = round % 2 == 1;
            let mut m = wide_toned(fold);
            let mut scenes: Vec<String> = Vec::new();
            for step in 0..60 {
                seq += 1;
                let session = ["a", "b"][next(2) as usize];
                let effect = match next(20) {
                    0..=9 => {
                        let (path, value) = leaf(&mut next);
                        set(&mut m, session, seq, &path, value)
                    }
                    10 => {
                        let writes: Vec<(String, Value)> =
                            (0..1 + next(8)).map(|_| leaf(&mut next)).collect();
                        let writes: Vec<(&str, Value)> = writes
                            .iter()
                            .map(|(p, v)| (p.as_str(), v.clone()))
                            .collect();
                        patch(&mut m, &format!("p{seq}"), &writes)
                    }
                    11 => touch(&mut m, session, seq, &leaf(&mut next).0),
                    12 => release(&mut m, session, seq, &leaf(&mut next).0),
                    13 => {
                        scenes.push(store(&mut m, &format!("s{seq}"), None, "S"));
                        Effect::Unchanged
                    }
                    14 => match scenes.get(next(scenes.len().max(1) as u64) as usize) {
                        Some(scene) => rest(
                            &mut m,
                            Command::Recall {
                                id: format!("r{seq}"),
                                scene: scene.clone(),
                                at: "2026-11-15T12:00:00.000Z".into(),
                                fade: None,
                            },
                        ),
                        None => Effect::Unchanged,
                    },
                    15 => {
                        let state = ["program", "preview", "off"][next(3) as usize];
                        let replace = next(2) == 0;
                        // A transition fades the stages it moves, which a
                        // save and load keeps.
                        let transition = (next(2) == 0).then(|| Transition {
                            start: next(96_000),
                            transition_ms: (next(2) == 0).then(|| next(500) as u32),
                        });
                        rest(
                            &mut m,
                            Command::Tally {
                                id: format!("t{seq}"),
                                sources: [("cam1".to_owned(), state.to_owned())].into(),
                                replace,
                                transition,
                            },
                        )
                    }
                    16 => m.expire(&origin(session).key()),
                    // Refused: an unknown control, and an input switch
                    // off the generation path.
                    17 => set(&mut m, session, seq, "/channels/nope/fader", json!(0)),
                    18 => {
                        let ch = channels[next(channels.len() as u64) as usize];
                        let other = if m.state()["channels"][ch]["input"] == "a" {
                            "b"
                        } else {
                            "a"
                        };
                        apply_as(&mut m, &input(ch, other), Some(false))
                    }
                    _ => {
                        let before = m.targets().unwrap();
                        let mut loaded = wide_toned(fold);
                        loaded.load(&m.save()).unwrap();
                        assert_eq!(
                            format!("{:?}", loaded.targets()),
                            format!("{:?}", Some(before))
                        );
                        m = loaded;
                        Effect::Unchanged
                    }
                };
                if let Effect::Changed(_) = effect {
                    changed += 1;
                }
                let kept = m.targets().unwrap();
                let whole = mix::targets(m.shape(), &m.wiring, m.state());
                let at = format!("round {round} step {step}");
                assert_eq!(gain_bits(&kept), gain_bits(&whole), "{at}");
                assert_eq!(
                    format!("{:?}", kept.strips),
                    format!("{:?}", whole.strips),
                    "{at}"
                );
                assert_eq!(kept.flows, whole.flows, "{at}");
                assert_eq!(kept.delays, whole.delays, "{at}");
                assert_eq!(kept.tones, whole.tones, "{at}");
                assert_eq!(
                    format!("{:?}", kept.ducks),
                    format!("{:?}", whole.ducks),
                    "{at}"
                );
                assert_eq!(kept.masks, whole.masks, "{at}");
            }
        }
        assert!(
            changed > 150 * 60 / 3,
            "{changed} commands changed the state"
        );
    }

    /// On a programme bus a stereo output folds down, each write of a
    /// fader, a pan or a duck group keeps the folded columns and the
    /// masks onto them those of the whole state, and the folded columns
    /// are the fold of the bus (centre at -3 dB on both sides).
    #[test]
    fn a_folded_output_and_its_duck_mask_follow_each_write() {
        let mut m = wide_with(true);
        let kept = |m: &Mixer| {
            let kept = m.targets().unwrap();
            let whole = mix::targets(m.shape(), &m.wiring, m.state());
            assert_eq!(gain_bits(&kept), gain_bits(&whole));
            assert_eq!(kept.masks, whole.masks);
            kept
        };
        patch(
            &mut m,
            "p1",
            &[
                ("/channels/c51/fader", json!(0)),
                ("/channels/c51/routes/pgm", json!(true)),
                ("/ducks/comm/channels/c51/target", json!(true)),
                ("/ducks/comm/destinations/pgm", json!(true)),
            ],
        );
        kept(&m);
        set(&mut m, "s", 1, "/buses/pgm/fader", json!(-6));
        let t = kept(&m);
        // Rows ch1, ch2, st1 x2, ch3, st2 x2, c51 x6: the centre is the
        // third leg. Columns: out-51 x6, out-lr x2.
        let row = 7 + 2;
        let h = std::f64::consts::FRAC_1_SQRT_2 * mix::gain(-6.0);
        assert!((f64::from(t.gains[row][6]) - h).abs() < 1e-6, "Lo of C");
        assert!((f64::from(t.gains[row][7]) - h).abs() < 1e-6, "Ro of C");
        assert_eq!(t.masks[row][6..8], [1, 1], "ducked on the fold");
        set(&mut m, "s", 2, "/ducks/comm/destinations/pgm", json!(false));
        assert_eq!(kept(&m).masks[row][6..8], [0, 0]);
        set(&mut m, "s", 3, "/channels/c51/fader", json!(-10));
        kept(&m);
    }

    fn input(ch: &str, value: &str) -> Command {
        ctl(
            Action::Set,
            &format!("/channels/{ch}/input"),
            Some(json!(value)),
        )
    }

    /// The slots a command re-routes are those whose selected flow it
    /// changes: not a selection it leaves as it is, nor input b of a
    /// channel without one.
    #[test]
    fn a_command_reroutes_the_slots_whose_flow_it_changes() {
        let mut m = wired();
        let reroutes =
            |m: &Mixer, c: &Command| m.reroutes(&origin("rest"), &serde_json::to_vec(c).unwrap());
        assert_eq!(reroutes(&m, &input("ch1", "b")), [0]);
        assert_eq!(reroutes(&m, &input("ch1", "a")), Vec::<u32>::new());
        assert_eq!(reroutes(&m, &input("st1", "b")), Vec::<u32>::new());
        let fader = ctl(Action::Set, "/channels/ch1/fader", Some(json!(-3)));
        assert_eq!(reroutes(&m, &fader), Vec::<u32>::new());
        let both = Command::Patch {
            id: "p".into(),
            changes: ["ch1", "ch2"]
                .map(|ch| Write {
                    path: format!("/channels/{ch}/input"),
                    value: json!("b"),
                })
                .to_vec(),
        };
        assert_eq!(reroutes(&m, &both), [0, 1]);
        // A recall re-routes the inputs it switches back.
        let scene = store(&mut m, "s1", None, "A");
        patch(&mut m, "p1", &[("/channels/ch2/input", json!("b"))]);
        let recall = Command::Recall {
            id: "r".into(),
            scene,
            at: "2026-11-15T12:00:00.000Z".into(),
            fade: None,
        };
        assert_eq!(reroutes(&m, &recall), [1]);
    }

    fn apply_as(m: &mut Mixer, c: &Command, generation: Option<bool>) -> Effect<Edits> {
        let bytes = serde_json::to_vec(c).unwrap();
        m.apply(&origin("rest"), &bytes, &ordered(generation))
    }

    fn refused(m: &Mixer, id: &str) -> bool {
        matches!(m.answer(&rest_key(), id), Some(Answer::Refused { .. }))
    }

    /// A command that switches an input applies on the generation path,
    /// and is refused, changing nothing, off it; one the writer did not
    /// classify applies as before.
    #[test]
    fn a_reroute_off_the_generation_path_is_refused() {
        let off = ctl(Action::Set, "/channels/ch1/input", Some(json!("b")));
        let mut m = wired();
        assert!(matches!(
            apply_as(&mut m, &off, Some(false)),
            Effect::Unchanged
        ));
        assert!(refused(&m, "c"));
        assert_eq!(at(&m, "/channels/ch1/input"), &json!("a"));
        assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some("a1"));
        for generation in [Some(true), None] {
            let mut m = wired();
            assert!(matches!(
                apply_as(&mut m, &off, generation),
                Effect::Changed(_)
            ));
            assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some("b1"));
        }
        // Commands that switch no input apply off it.
        let mut m = wired();
        let fader = ctl(Action::Set, "/channels/ch1/fader", Some(json!(-3)));
        assert!(matches!(
            apply_as(&mut m, &fader, Some(false)),
            Effect::Changed(_)
        ));
    }

    /// A recall that switches inputs in two domains is refused on the
    /// generation path, on every replica alike: one engine generation
    /// cannot switch both. In one domain it applies.
    #[test]
    fn a_recall_switching_inputs_in_two_domains_is_refused() {
        let mut m = wired();
        let scene = store(&mut m, "s1", None, "B");
        patch(
            &mut m,
            "p1",
            &[
                ("/channels/ch1/input", json!("b")),
                ("/channels/ch2/input", json!("b")),
            ],
        );
        let two = Command::Recall {
            id: "r2".into(),
            scene: scene.clone(),
            at: "2026-11-15T12:00:00.000Z".into(),
            fade: None,
        };
        let mut other = wired();
        other.load(&m.save()).unwrap();
        for m in [&mut m, &mut other] {
            assert!(matches!(apply_as(m, &two, Some(true)), Effect::Unchanged));
            assert!(refused(m, "r2"));
            assert_eq!(m.targets().unwrap().flows, flows(&["b1", "b2", "a3"]));
        }
        // The writer's classification of it is both slots.
        assert_eq!(
            m.reroutes(&origin("rest"), &serde_json::to_vec(&two).unwrap()),
            [0, 1]
        );
        // In one domain it applies.
        patch(&mut m, "p2", &[("/channels/ch2/input", json!("a"))]);
        let one = Command::Recall {
            id: "r1".into(),
            scene,
            at: "2026-11-15T12:00:00.000Z".into(),
            fade: None,
        };
        assert!(matches!(
            apply_as(&mut m, &one, Some(true)),
            Effect::Changed(_)
        ));
        assert_eq!(m.targets().unwrap().flows, flows(&["a1", "a2", "a3"]));
    }

    fn recall_faded(m: &mut Mixer, id: &str, scene: &str, fade: Option<Crossfade>) -> Targets {
        let recall = Command::Recall {
            id: id.into(),
            scene: scene.into(),
            at: "2026-11-15T12:00:00.000Z".into(),
            fade,
        };
        assert!(matches!(apply_as(m, &recall, None), Effect::Changed(_)));
        m.targets().unwrap()
    }

    /// A crossfading recall fades each gain it changes, a mute's to or
    /// from silence included, from the start index the command carries,
    /// from the gain's value there to its new target; a gain it leaves
    /// gets no fade, and a recall without one is a cut. Mid-fade a new
    /// recall fades from where the running fade has got to, a fader move
    /// drops the fades of the gains it moves and keeps the others', and a
    /// snapshot carries the fades.
    #[test]
    fn a_crossfading_recall_fades_every_gain_it_changes() {
        let mut m = wired();
        let faders = |db: i32| {
            [
                ("/channels/ch1/fader", json!(db)),
                ("/channels/ch2/fader", json!(db)),
            ]
        };
        patch(&mut m, "p0", &faders(0));
        let open = m.targets().unwrap().gains;
        let scene = store(&mut m, "s1", None, "A");
        patch(&mut m, "p1", &faders(-20));
        patch(&mut m, "p2", &[("/channels/ch2/mute", json!(true))]);
        let low = m.targets().unwrap().gains;
        let quiet = store(&mut m, "s2", None, "B");
        let cut = recall_faded(&mut m, "r0", &scene, None);
        assert_eq!(cut.gains, open);
        assert!(cut.fading.is_empty(), "a cut fades nothing");
        recall_faded(&mut m, "r1", &quiet, None);
        let xf = Crossfade {
            start: 96_000,
            len: 48_000,
        };
        let t = recall_faded(&mut m, "r2", &scene, Some(xf));
        assert_eq!(t.gains, open);
        let mut faded = BTreeSet::new();
        for (i, row) in t.gains.iter().enumerate() {
            for (o, &g) in row.iter().enumerate() {
                let want = (low[i][o].to_bits() != g.to_bits()).then_some(Fade {
                    from: low[i][o],
                    start: xf.start,
                    len: xf.len,
                });
                assert_eq!(t.fading[i][o], want, "gain {i} -> {o}");
                if want.is_some() {
                    faded.insert(i);
                }
            }
        }
        assert_eq!(
            faded,
            BTreeSet::from([0, 1]),
            "ch1's fader, ch2's fader and mute"
        );
        assert!(low[1].iter().all(|&g| g == 0.0), "ch2 fades from silence");
        // Half way, a recall back to B fades from where A's fade is.
        let back = Crossfade {
            start: xf.start + u64::from(xf.len) / 2,
            len: 4_800,
        };
        let t = recall_faded(&mut m, "r3", &quiet, Some(back));
        assert_eq!(t.gains, low);
        let mut halfway = 0;
        for (i, row) in t.gains.iter().enumerate() {
            for (o, &g) in row.iter().enumerate() {
                if open[i][o].to_bits() == g.to_bits() {
                    continue;
                }
                let from = Fade {
                    from: low[i][o],
                    start: xf.start,
                    len: xf.len,
                }
                .value_at(open[i][o], back.start);
                let (lo, hi) = (low[i][o].min(open[i][o]), low[i][o].max(open[i][o]));
                assert!(lo < from && from < hi, "gain {i} -> {o} mid-fade: {from}");
                assert_eq!(
                    t.fading[i][o],
                    Some(Fade {
                        from,
                        start: back.start,
                        len: back.len,
                    })
                );
                halfway += 1;
            }
        }
        assert!(halfway > 0);
        // A fader move mid-fade drops ch1's fades and keeps ch2's.
        patch(&mut m, "p3", &[("/channels/ch1/fader", json!(-10))]);
        let t = m.targets().unwrap();
        assert!(t.fading[0].iter().all(Option::is_none), "{:?}", t.fading[0]);
        assert!(t.fading[1].iter().any(Option::is_some), "{:?}", t.fading[1]);
        let mut other = wired();
        other.load(&m.save()).unwrap();
        assert_eq!(other.targets().unwrap().fading, t.fading);
        // A cut over running fades drops those of the gains it changes.
        let t = recall_faded(&mut m, "r4", &scene, None);
        assert!(t.fading.is_empty(), "{:?}", t.fading);
    }

    /// A recall received at or after a running fade's end drops that fade
    /// where it leaves the gain alone: the gain holds the scene's value, and
    /// the fades a set carries stay bounded by the gains still moving.
    #[test]
    fn a_later_recall_drops_the_fades_that_ended() {
        let xf = Crossfade {
            start: 96_000,
            len: 48_000,
        };
        let end = xf.start + u64::from(xf.len);
        // A recall starting at s was received at s less the lead.
        let lead = crossfade_lead(48_000);
        // Both channels fade up to A; C then takes ch1 down again from
        // `start`, leaving ch2 at A's value.
        let ch2_after = |start: u64| {
            let mut m = wired();
            let faders = |a: i32, b: i32| {
                [
                    ("/channels/ch1/fader", json!(a)),
                    ("/channels/ch2/fader", json!(b)),
                ]
            };
            patch(&mut m, "p0", &faders(0, 0));
            let a = store(&mut m, "s1", None, "A");
            patch(&mut m, "p1", &faders(-20, 0));
            let c = store(&mut m, "s2", None, "C");
            patch(&mut m, "p2", &faders(-20, -20));
            let t = recall_faded(&mut m, "r0", &a, Some(xf));
            assert!(t.fading[0].iter().any(Option::is_some), "ch1 fades");
            assert!(t.fading[1].iter().any(Option::is_some), "ch2 fades");
            let t = recall_faded(&mut m, "r1", &c, Some(Crossfade { start, len: 4_800 }));
            assert!(t.fading[0].iter().any(Option::is_some), "ch1 fades anew");
            t.fading[1].clone()
        };
        assert!(
            ch2_after(end + lead - 1).iter().any(Option::is_some),
            "received before A's end, ch2's fade runs on"
        );
        let ended = ch2_after(end + lead);
        assert!(ended.iter().all(Option::is_none), "{ended:?}");
    }

    /// A tally fades the gains of each channel whose AFV stage it moves,
    /// from the start its transition carries, per model `AfvBinding`: a
    /// stage it closes waits its holdMs and falls over its fallMs, one it
    /// opens rises over its riseMs from the start, and transitionMs takes
    /// the place of both lengths, the hold kept. A channel whose stage is
    /// disabled gets none, a fader move drops the fades of the gains it
    /// moves, and a tally without a transition is a cut.
    #[test]
    fn a_tally_fades_the_afv_stages_it_moves_on_their_times() {
        let mut m = wired();
        patch(
            &mut m,
            "p0",
            &[
                ("/channels/ch1/fader", json!(0)),
                ("/channels/ch2/fader", json!(0)),
                ("/channels/st1/fader", json!(0)),
                ("/afv/ch1/enabled", json!(true)),
                ("/afv/ch1/source", json!("cam1")),
                ("/afv/ch1/offLevel", json!(-20)),
                ("/afv/ch1/riseMs", json!(100)),
                ("/afv/ch1/holdMs", json!(250)),
                ("/afv/ch1/fallMs", json!(500)),
                ("/afv/st1/enabled", json!(true)),
                ("/afv/st1/source", json!("cam1")),
                ("/afv/st1/offLevel", json!(-30)),
                ("/afv/st1/riseMs", json!(20)),
                ("/afv/st1/holdMs", json!(0)),
                ("/afv/st1/fallMs", json!(40)),
                // Bound to the same source, but disabled.
                ("/afv/ch2/source", json!("cam1")),
            ],
        );
        let rate = u64::from(m.shape().sample_rate);
        let samples = |ms: u64| ms * rate / 1000;
        let tally = |m: &mut Mixer, id: &str, state: &str, transition| {
            let sources = [("cam1".to_owned(), state.to_owned())].into();
            let tally = Command::Tally {
                id: id.into(),
                sources,
                replace: false,
                transition,
            };
            assert!(matches!(rest(m, tally), Effect::Changed(_)));
            m.targets().unwrap()
        };
        // Every gain that moved fades as `want` gives for its row, the
        // start and length, from the target it had: each fade before
        // ended by then. ch1's row and st1's two move, ch2's does not.
        let check = |t: &Targets, before: &[Vec<f32>], want: &dyn Fn(usize) -> (u64, u64)| {
            let mut moved = BTreeSet::new();
            for (r, row) in t.gains.iter().enumerate() {
                for (o, &g) in row.iter().enumerate() {
                    let changed = before[r][o].to_bits() != g.to_bits();
                    let fade = changed.then(|| {
                        let (start, len) = want(r);
                        Fade {
                            from: before[r][o],
                            start,
                            len: len as u32,
                        }
                    });
                    assert_eq!(t.fading[r][o], fade, "gain {r} -> {o}");
                    if changed {
                        moved.insert(r);
                    }
                }
            }
            assert_eq!(moved, BTreeSet::from([0, 2, 3]));
        };
        let open = m.targets().unwrap().gains;
        let start = 96_000;
        let timed = |start| {
            Some(Transition {
                start,
                transition_ms: None,
            })
        };
        let closed = tally(&mut m, "t1", "preview", timed(start));
        assert_eq!(at(&m, "/afv/ch2/open"), &json!(true));
        check(&closed, &open, &|r| match r {
            0 => (start + samples(250), samples(500)),
            _ => (start, samples(40)),
        });
        let start = 200_000;
        let opened = tally(&mut m, "t2", "program", timed(start));
        assert_eq!(opened.gains, open);
        check(&opened, &closed.gains, &|r| match r {
            0 => (start, samples(100)),
            _ => (start, samples(20)),
        });
        let start = 300_000;
        let transition = Some(Transition {
            start,
            transition_ms: Some(60),
        });
        let closed = tally(&mut m, "t3", "preview", transition);
        check(&closed, &open, &|r| match r {
            0 => (start + samples(250), samples(60)),
            _ => (start, samples(60)),
        });
        // A fader move mid-transition drops ch1's fades and keeps st1's.
        patch(&mut m, "p1", &[("/channels/ch1/fader", json!(-10))]);
        let t = m.targets().unwrap();
        assert!(t.fading[0].iter().all(Option::is_none), "{:?}", t.fading[0]);
        assert!(t.fading[2].iter().any(Option::is_some), "{:?}", t.fading[2]);
        let cut = tally(&mut m, "t4", "program", None);
        assert!(cut.fading.is_empty(), "{:?}", cut.fading);
    }

    /// The configuration hash follows the shape, the declared consoles
    /// and each input's flows and domain, and not the state.
    #[test]
    fn the_config_hash_covers_the_configuration_alone() {
        let mut m = wired();
        let hash = m.config_hash();
        assert_eq!(wired().config_hash(), hash);
        set(&mut m, "s", 1, "/channels/ch1/fader", json!(-3));
        assert_eq!(m.config_hash(), hash, "not the state");
        let mut other = wired();
        other.wiring[1].domain = "d".into();
        assert_ne!(other.config_hash(), hash, "a domain");
        let mut other = wired();
        other.wiring[0].b = Some("b9".into());
        assert_ne!(other.config_hash(), hash, "a flow");
        let other = Mixer::configured(m.shape().clone(), vec![desk("x", "X")], m.wiring.clone());
        assert_ne!(other.config_hash(), hash, "a declared console");
        assert_ne!(mixer().config_hash(), hash, "the shape and wiring");
    }

    /// Replicas share a set only at one configuration hash, so a release
    /// that moved it for an unchanged configuration could not roll into
    /// a running set: the hash of a fixed configuration is pinned. A
    /// derivation change raises Product::DERIVATION_VERSION, never this.
    #[test]
    fn the_config_hash_of_a_fixed_configuration_never_moves() {
        assert_eq!(wired().config_hash(), 1_102_375_237_759_052_186);
    }

    /// `wired` with a delay line of `line` samples on ch1 and none on
    /// the others.
    fn delayed(line: u32) -> Mixer {
        let m = wired();
        let mut shape = m.shape().clone();
        shape.channels[0].max_delay = Some(line);
        Mixer::configured(shape, Vec::new(), m.wiring.clone())
    }

    /// A channel's delay is its input slot's delay target, 0 by default,
    /// and reaches the engine as one `Delay` entry. A write is refused
    /// past the channel's line and, on a channel without one, for
    /// anything but 0: the engine would refuse the entry. A loaded state
    /// whose delay the configured line no longer takes drops it to 0.
    #[test]
    fn a_channels_delay_is_its_input_slots_within_its_line() {
        let mut m = delayed(960);
        let before = m.targets().unwrap();
        assert_eq!(before.delays, [0, 0, 0]);
        patch(&mut m, "p1", &[("/channels/ch1/delay", json!(960))]);
        let after = m.targets().unwrap();
        assert_eq!(after.delays, [960, 0, 0]);
        let entries: Vec<String> = (before.diff(&after).iter())
            .filter(|e| matches!(e, samplez::engine::Entry::Delay { .. }))
            .map(|e| format!("{e:?}"))
            .collect();
        assert_eq!(entries, ["Delay { slot: 0, samples: 960 }"]);
        assert_ne!(
            m.config_hash(),
            wired().config_hash(),
            "the line is configuration"
        );

        let refused = |m: &Mixer, path: &str, value: Value| match m.check_write(path, &value) {
            Err(Answer::Invalid { message }) => message,
            other => panic!("{path} {value}: {other:?}"),
        };
        let e = refused(&m, "/channels/ch1/delay", json!(961));
        assert!(e.contains("ch1 delays at most 960 samples"), "{e}");
        let e = refused(&m, "/channels/ch2/delay", json!(1));
        assert!(e.contains("ch2 has no delay line"), "{e}");
        m.check_write("/channels/ch2/delay", &json!(0)).unwrap();
        patch(&mut m, "p2", &[("/channels/ch1/delay", json!(961))]);
        assert_eq!(
            m.state()["channels"]["ch1"]["delay"],
            960,
            "nothing applied"
        );

        let mut shorter = delayed(480);
        shorter.load(&m.save()).unwrap();
        assert_eq!(shorter.state()["channels"]["ch1"]["delay"], 0);
        assert_eq!(shorter.dropped, ["/channels/ch1/delay"]);
        assert_eq!(shorter.targets().unwrap().delays, [0, 0, 0]);
        let mut same = delayed(960);
        same.load(&m.save()).unwrap();
        assert_eq!(same.targets().unwrap().delays, [960, 0, 0]);
    }

    /// `wired` with st1 a tone channel starting at 440 Hz, -20 dBFS, as
    /// the configuration wires one: to no flow.
    fn toned() -> Mixer {
        let m = wired();
        let mut shape = m.shape().clone();
        shape.channels[2].tone = Some(crate::shape::ChannelTone {
            frequency: 440,
            level: -20.0,
        });
        let mut wiring = m.wiring.clone();
        wiring[2] = Wiring {
            a: String::new(),
            b: None,
            domain: String::new(),
        };
        Mixer::configured(shape, Vec::new(), wiring)
    }

    /// A tone channel's generator is its input slot's tone target, from
    /// the state's `tone`, which starts as configured, and the slot reads
    /// no flow; other slots have none. A write reaches the engine as one
    /// `Tone` entry. The state takes exactly samplez's ranges: a value
    /// past one is refused, never clamped, and a channel reading a flow
    /// has no tone to write. A load keeps a saved tone, and a state
    /// saved without one starts the generator as configured.
    #[test]
    fn a_tone_channels_generator_is_its_input_slots_tone() {
        use samplez_dsp::{TONE_FREQUENCY_HZ, TONE_LEVEL_DB, ToneParams};
        let mut m = toned();
        check(&contract().state, m.state()).unwrap();
        let before = m.targets().unwrap();
        let tone = |frequency_hz, level_db, ident| ToneParams {
            frequency_hz,
            level_db,
            ident,
        };
        // The line-up tone's mono and stereo generators' slots last, at
        // its initial -18 dBFS.
        let line_up = Some(tone(1000, -18.0, false));
        assert_eq!(
            before.tones,
            [None, None, Some(tone(440, -20.0, false)), line_up, line_up]
        );
        let a = |id: &str| Some(id.to_owned());
        assert_eq!(before.flows, [a("a1"), a("a2"), None, None, None]);
        assert!(!m.engine_view().detached[2], "not an IS-05 detach");
        assert_ne!(m.config_hash(), wired().config_hash());

        patch(
            &mut m,
            "p1",
            &[
                ("/channels/st1/tone/frequency", json!(1000)),
                ("/channels/st1/tone/level", json!(-6.5)),
                ("/channels/st1/tone/ident", json!(true)),
            ],
        );
        let after = m.targets().unwrap();
        assert_eq!(after.tones[2], Some(tone(1000, -6.5, true)));
        let whole = mix::targets(m.shape(), &m.wiring, m.state());
        assert_eq!(
            after.tones, whole.tones,
            "kept as derived from the whole state"
        );
        let entries: Vec<String> = (before.diff(&after).iter())
            .map(|e| format!("{e:?}"))
            .collect();
        assert_eq!(
            entries,
            ["Tone { slot: 2, frequency_hz: 1000, level_db: -6.5, ident: true }"]
        );

        let (hz, db) = (TONE_FREQUENCY_HZ, TONE_LEVEL_DB);
        let path = |leaf: &str| format!("/channels/st1/tone/{leaf}");
        for (leaf, value) in [
            ("frequency", json!(hz.start())),
            ("frequency", json!(hz.end())),
            ("level", json!(db.start())),
            ("level", json!(db.end())),
        ] {
            m.check_write(&path(leaf), &value).unwrap();
        }
        for (leaf, value) in [
            ("frequency", json!(hz.start() - 1)),
            ("frequency", json!(hz.end() + 1)),
            ("frequency", json!(440.5)),
            ("level", json!(f64::from(*db.start()) - 0.01)),
            ("level", json!(f64::from(*db.end()) + 0.01)),
            ("ident", json!(1)),
        ] {
            match m.check_write(&path(leaf), &value) {
                Err(Answer::Invalid { .. }) => {}
                other => panic!("{leaf} {value}: {other:?}"),
            }
        }
        patch(&mut m, "p2", &[("/channels/st1/tone/level", json!(3))]);
        assert_eq!(
            at(&m, "/channels/st1/tone/level"),
            &json!(-6.5),
            "not clamped"
        );
        for path in ["/channels/ch1/tone/level", "/tone/frequency"] {
            match m.check_write(path, &json!(-6)) {
                Err(Answer::NotFound { .. }) => {}
                other => panic!("{path}: {other:?}"),
            }
        }

        let mut loaded = toned();
        loaded.load(&m.save()).unwrap();
        assert_eq!(loaded.targets().unwrap().tones, after.tones);
        let mut fresh = toned();
        fresh.load(&wired().save()).unwrap();
        assert_eq!(fresh.targets().unwrap().tones, before.tones);
    }

    /// A scene's delay that the channel's line, shortened since the scene
    /// was stored, no longer takes recalls 0 rather than leaving the
    /// delay as it was, and the recall's answer names the channel, so
    /// the console can say so. One within the line recalls as stored,
    /// with no answer.
    #[test]
    fn a_recalled_delay_past_the_line_is_0_and_answered() {
        let mut m = delayed(960);
        patch(&mut m, "p1", &[("/channels/ch1/delay", json!(960))]);
        let scene = store(&mut m, "s1", None, "Lip sync");
        let recall = |m: &mut Mixer, id: &str| {
            rest(
                m,
                Command::Recall {
                    id: id.into(),
                    scene: scene.clone(),
                    at: "2026-11-15T12:01:00.000Z".into(),
                    fade: None,
                },
            )
        };

        let mut shorter = delayed(480);
        shorter.load(&m.save()).unwrap();
        patch(&mut shorter, "p2", &[("/channels/ch1/delay", json!(240))]);
        recall(&mut shorter, "r1");
        assert_eq!(shorter.state()["channels"]["ch1"]["delay"], 0);
        assert_eq!(shorter.targets().unwrap().delays, [0, 0, 0]);
        assert_eq!(
            shorter.answer(&rest_key(), "r1"),
            Some(&Answer::Recalled {
                delays: vec!["ch1".into()]
            })
        );

        patch(&mut m, "p3", &[("/channels/ch1/delay", json!(0))]);
        recall(&mut m, "r2");
        assert_eq!(m.state()["channels"]["ch1"]["delay"], 960);
        assert_eq!(m.answer(&rest_key(), "r2"), None);
    }

    /// A snapshot carries its format: a binary that reads the earlier
    /// layout refuses it, and one of another format is refused, while a
    /// snapshot of the earlier layout still loads.
    #[test]
    fn a_snapshot_carries_its_format() {
        let (m, _) = busy();
        let saved = m.save();
        assert!(
            serde_json::from_slice::<Saved>(&saved).is_err(),
            "earlier binaries refuse it"
        );
        let mut v: Value = serde_json::from_slice(&saved).unwrap();
        v["format"] = json!(FORMAT + 1);
        let e = mixer().load(&serde_json::to_vec(&v).unwrap()).unwrap_err();
        assert!(e.contains(&format!("format {}", FORMAT + 1)), "{e}");
        let earlier = serde_json::to_vec(&v["mixer"]).unwrap();
        let mut back = mixer();
        back.load(&earlier).unwrap();
        assert_eq!(back.state(), m.state());
    }

    const F1: &str = "0d7f6b8e-4a2c-4e8f-9b1a-2c3d4e5f6a01";
    const F2: &str = "0d7f6b8e-4a2c-4e8f-9b1a-2c3d4e5f6a02";
    const DOMAIN: &str = "6f1f1d1e-56f6-4c4c-8d32-9e7f4a3b2c10";

    fn conn(enabled: bool, flow: Option<&str>) -> Connection {
        Connection {
            activation: None,
            master_enable: enabled,
            peer: None,
            domain: Some(DOMAIN.into()),
            flow: flow.map(Into::into),
        }
    }

    fn connect(m: &mut Mixer, id: &str, port: PortRef, active: Connection) -> Effect<Edits> {
        let c = Command::Connect {
            id: id.into(),
            port,
            active,
        };
        apply_as(m, &c, Some(true))
    }

    fn receiver(ch: &str) -> PortRef {
        PortRef::Receiver(ch.into())
    }

    /// A receiver's activation is its channel's selected input: the
    /// engine reads the flow it names, and a disabled receiver detaches
    /// the channel whichever flow it names. The replicated state carries
    /// it, so every replica routes alike.
    #[test]
    fn a_receiver_activation_routes_its_channel_and_a_disable_detaches_it() {
        let mut m = wired();
        let e = connect(&mut m, "n1", receiver("ch1"), conn(true, Some(F1)));
        assert!(matches!(e, Effect::Changed(_)));
        assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some(F1));
        let (shown, selected) = m.receiver("ch1").unwrap();
        assert_eq!((shown, selected), (Some(&conn(true, Some(F1))), F1));
        connect(&mut m, "n2", receiver("ch1"), conn(false, Some(F2)));
        assert_eq!(m.targets().unwrap().flows[0], None);
        assert!(m.engine_view().detached[0]);
        connect(&mut m, "n3", receiver("ch1"), conn(true, Some(F2)));
        assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some(F2));
        // The other channels keep their configured flows.
        assert_eq!(m.targets().unwrap().flows[1].as_deref(), Some("a2"));
    }

    /// A switch between inputs a and b moves the receiver to the other
    /// input's flow and keeps the flow an activation named for the input
    /// it was on.
    #[test]
    fn a_switch_between_inputs_shows_in_the_receiver() {
        let mut m = wired();
        connect(&mut m, "n1", receiver("ch2"), conn(true, Some(F1)));
        patch(&mut m, "p1", &[("/channels/ch2/input", json!("b"))]);
        assert_eq!(
            m.receiver("ch2").unwrap().0.unwrap().flow.as_deref(),
            Some("b2")
        );
        assert_eq!(m.targets().unwrap().flows[1].as_deref(), Some("b2"));
        patch(&mut m, "p2", &[("/channels/ch2/input", json!("a"))]);
        assert_eq!(
            m.receiver("ch2").unwrap().0.unwrap().flow.as_deref(),
            Some(F1)
        );
        assert_eq!(m.targets().unwrap().flows[1].as_deref(), Some(F1));
        // On input b an activation sets input b's flow.
        patch(&mut m, "p3", &[("/channels/ch2/input", json!("b"))]);
        connect(&mut m, "n2", receiver("ch2"), conn(true, Some(F2)));
        patch(&mut m, "p4", &[("/channels/ch2/input", json!("a"))]);
        assert_eq!(m.targets().unwrap().flows[1].as_deref(), Some(F1));
        patch(&mut m, "p5", &[("/channels/ch2/input", json!("b"))]);
        assert_eq!(m.targets().unwrap().flows[1].as_deref(), Some(F2));
    }

    /// A sender's `master_enable` starts and stops its output; an output
    /// never activated is enabled.
    #[test]
    fn a_sender_activation_enables_its_output() {
        let mut m = wired();
        assert_eq!(m.targets().unwrap().outputs, [true]);
        let out = || PortRef::Sender("out-pgm".into());
        connect(&mut m, "n1", out(), conn(false, None));
        assert_eq!(m.targets().unwrap().outputs, [false]);
        assert_eq!(m.sender("out-pgm"), Some(&conn(false, None)));
        connect(&mut m, "n2", out(), conn(true, Some(F1)));
        assert_eq!(m.targets().unwrap().outputs, [true]);
    }

    /// An activation that changes the flow a channel reads is ordered on
    /// the generation path like an input switch, and refused off it; one
    /// that changes none is not.
    #[test]
    fn an_activation_switching_an_input_needs_the_generation_path() {
        let m = wired();
        let c = |active| {
            let c = Command::Connect {
                id: "n1".into(),
                port: receiver("ch1"),
                active,
            };
            serde_json::to_vec(&c).unwrap()
        };
        let same = Connection {
            flow: Some("a1".into()),
            ..conn(true, None)
        };
        assert_eq!(m.reroutes(&origin("rest"), &c(conn(true, Some(F1)))), [0]);
        assert_eq!(m.reroutes(&origin("rest"), &c(conn(false, None))), [0]);
        assert!(m.reroutes(&origin("rest"), &c(same)).is_empty());
        let mut m = wired();
        let off = Command::Connect {
            id: "n2".into(),
            port: receiver("ch1"),
            active: conn(true, Some(F1)),
        };
        assert!(matches!(
            apply_as(&mut m, &off, Some(false)),
            Effect::Unchanged
        ));
        assert!(refused(&m, "n2"));
        assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some("a1"));
    }

    #[test]
    fn activations_of_unknown_ports_or_without_a_flow_are_refused() {
        let mut m = wired();
        let invalid = |m: &Mixer, id: &str| {
            matches!(
                m.answer(&rest_key(), id),
                Some(Answer::Invalid { .. } | Answer::NotFound { .. })
            )
        };
        connect(&mut m, "n1", receiver("nope"), conn(true, Some(F1)));
        connect(
            &mut m,
            "n2",
            PortRef::Sender("nope".into()),
            conn(true, None),
        );
        connect(&mut m, "n3", receiver("ch1"), conn(true, None));
        connect(&mut m, "n4", receiver("ch1"), conn(true, Some("A1")));
        for id in ["n1", "n2", "n3", "n4"] {
            assert!(invalid(&m, id), "{id}");
        }
        assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some("a1"));
    }

    /// The connections survive a snapshot, so a restarted replica
    /// routes and shows them as before; a snapshot of format 2 has none.
    #[test]
    fn connections_load_back_from_a_snapshot() {
        let mut m = wired();
        connect(&mut m, "n1", receiver("ch1"), conn(false, Some(F1)));
        connect(&mut m, "n2", receiver("ch2"), conn(true, Some(F2)));
        connect(
            &mut m,
            "n3",
            PortRef::Sender("out-pgm".into()),
            conn(false, None),
        );
        let saved = m.save();
        let mut back = wired();
        back.load(&saved).unwrap();
        assert_eq!(back.targets(), m.targets());
        assert_eq!(back.receiver("ch1"), m.receiver("ch1"));
        assert_eq!(back.sender("out-pgm"), m.sender("out-pgm"));
        let mut v: Value = serde_json::from_slice(&saved).unwrap();
        v["format"] = json!(FORMAT_WITHOUT_PORTS);
        v["mixer"].as_object_mut().unwrap().remove("ports");
        let mut back = wired();
        back.load(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(back.targets(), wired().targets());
    }

    /// A change of a port's `/active` records its version, the TAI time
    /// of the index its entry takes effect at, so every replica gives
    /// the port the same one; a second change at that time takes the
    /// next nanosecond, as samplez-nmos sets a version. A switch between
    /// a channel's inputs changes its receiver, a fader write changes no
    /// port, and a snapshot keeps the versions.
    #[test]
    fn a_port_change_records_the_version_of_its_entry() {
        let mut m = wired();
        let rate = u64::from(m.shape().sample_rate);
        let run = |m: &mut Mixer, id: &str, active: Connection, at_index: Option<u64>| {
            let c = Command::Connect {
                id: id.into(),
                port: receiver("ch1"),
                active,
            };
            let ctx = Apply {
                generation: Some(true),
                at_index,
            };
            m.apply(&origin("rest"), &serde_json::to_vec(&c).unwrap(), &ctx)
        };
        // 2.5 s after the epoch of the flows' indexes.
        run(&mut m, "n1", conn(true, Some(F1)), Some(rate * 5 / 2));
        let change = |m: &Mixer, port: &PortRef| m.port_change(port).cloned();
        let first = PortChange {
            version: 2_500_000_000,
            id: "n1".into(),
        };
        assert_eq!(change(&m, &receiver("ch1")), Some(first));
        run(&mut m, "n2", conn(false, Some(F1)), Some(rate * 5 / 2));
        let second = PortChange {
            version: 2_500_000_001,
            id: "n2".into(),
        };
        assert_eq!(change(&m, &receiver("ch1")), Some(second.clone()));
        let pgm = PortRef::Sender("out-pgm".into());
        assert_eq!(change(&m, &pgm), None, "no change of the sender");

        assert_eq!(change(&m, &receiver("ch2")), None);
        set(&mut m, "s", 1, "/channels/ch2/fader", json!(-10));
        assert_eq!(change(&m, &receiver("ch2")), None, "a fader is no port");
        set(&mut m, "s", 2, "/channels/ch2/input", json!("b"));
        let switched = PortChange {
            version: 1,
            id: "s-2".into(),
        };
        assert_eq!(change(&m, &receiver("ch2")), Some(switched.clone()));

        let mut back = wired();
        back.load(&m.save()).unwrap();
        assert_eq!(change(&back, &receiver("ch1")), Some(second));
        assert_eq!(change(&back, &receiver("ch2")), Some(switched));
    }

    /// The state is a function of the log: a snapshot, one an older
    /// binary saved without the surround position included, and a log
    /// replayed from its first entry hold the same state, whatever the
    /// log wrote, the deprecated `fr` among it, which the panner never
    /// reads. A mono channel on a surround bus sounds as it did before
    /// the field, the L/R pair of its `lr`.
    #[test]
    fn a_snapshot_and_a_replayed_log_hold_the_same_state() {
        let shape = Shape::parse(&crate::shape::tests::surround()).unwrap();
        let writes = [
            ("/channels/ch1/fader", json!(0)),
            ("/channels/ch1/routes/pgm", json!(true)),
            ("/channels/ch1/pan/lr", json!(0.3)),
            // The deprecated leaves an old console could write.
            ("/channels/ch1/pan/fr", json!(0)),
            ("/channels/ch1/pan/divergence", json!(0)),
            ("/channels/ch1/pan/lfe", json!(-6)),
        ];
        let mut replayed = Mixer::new(shape.clone());
        for (n, (path, value)) in writes.into_iter().enumerate() {
            set(&mut replayed, "a", n as u64 + 1, path, value);
        }
        let scene = store(&mut replayed, "c1", None, "Opening");
        let saved = replayed.save();
        let theta = 1.3 * std::f64::consts::FRAC_PI_4;
        let before = [libm::cos(theta), libm::sin(theta), 0.0, 0.0, 0.0, 0.0];
        let legs = |m: &Mixer| -> Vec<f64> {
            let g = crate::mix::matrix(&shape, m.state());
            g[0][..6].iter().map(|g| f64::from(*g)).collect()
        };
        let close = |got: Vec<f64>| {
            assert!(
                got.iter().zip(before).all(|(g, w)| (g - w).abs() < 1e-6),
                "{got:?} vs {before:?}"
            );
        };
        close(legs(&replayed));
        // A snapshot of this binary.
        let mut loaded = Mixer::new(shape.clone());
        loaded.load(&saved).unwrap();
        assert_eq!(loaded.text(), replayed.text());
        close(legs(&loaded));
        // One an older binary saved: no surround position in it or in its scenes.
        let mut old: Value = serde_json::from_slice(&saved).unwrap();
        let strip = |channels: &mut Value| {
            let pan = channels["ch1"]["pan"].as_object_mut().unwrap();
            for leaf in ["frontRear", "spread", "lfeSend"] {
                pan.remove(leaf);
            }
        };
        strip(&mut old["mixer"]["state"]["channels"]);
        for stored in old["mixer"]["scenes"].as_array_mut().unwrap() {
            strip(&mut stored[1]["channels"]);
        }
        let mut back = Mixer::new(shape.clone());
        back.load(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(back.text(), replayed.text());
        close(legs(&back));
        // A snapshot install builds a default state that takes the saved
        // one, the missing surround position filled in.
        let mut installed = Mixer::default();
        installed.load(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(installed.text(), replayed.text());
        close(legs(&installed));
        // The stored scene recalls the same.
        set(&mut back, "b", 1, "/channels/ch1/pan/lr", json!(-1));
        let recalled = Command::Recall {
            id: "r1".into(),
            scene,
            at: "2026-11-19T00:00:00Z".into(),
            fade: None,
        };
        rest(&mut back, recalled);
        close(legs(&back));
        // The initial state has the surround position at the front, full
        // spread and no LFE send, a stereo channel's included.
        for ch in ["ch1", "st1"] {
            let pan = format!("/channels/{ch}/pan");
            assert_eq!(*at(&replayed, &format!("{pan}/frontRear")), json!(-1));
            assert_eq!(*at(&replayed, &format!("{pan}/spread")), json!(1));
            assert_eq!(*at(&replayed, &format!("{pan}/lfeSend")), json!(-90));
        }
    }

    /// A control request is ordered only for a control somebody else
    /// holds, and carries the holder it is for and the asker's record;
    /// it changes no value and no owner.
    #[test]
    fn a_control_request_names_the_holder_it_is_for() {
        let mut m = mixer();
        let path = "/channels/ch1/fader";
        let ask = |m: &mut Mixer, session: &str| {
            let request = Command::ControlRequest {
                id: format!("q-{session}"),
                path: path.into(),
                from: json!({ "name": session }),
                at: 1_000,
            };
            m.apply(
                &origin(session),
                &serde_json::to_vec(&request).unwrap(),
                &Apply::default(),
            )
        };
        assert_eq!(ask(&mut m, "b"), Effect::Unchanged, "nobody holds it");
        touch(&mut m, "a", 1, path);
        assert_eq!(
            ask(&mut m, "a"),
            Effect::Unchanged,
            "the holder asks itself"
        );
        let Effect::Changed(edits) = ask(&mut m, "b") else {
            panic!("a request for a held control");
        };
        assert!(edits.values.is_empty() && edits.owners.is_empty());
        assert_eq!(
            edits.requests,
            vec![json!({
                "path": path,
                "from": { "name": "b" },
                "to": { "principal": "anonymous", "session": "a" },
                "at": 1_000,
            })]
        );
    }

    const OWN: &str = "0d7f6b8e-4a2c-4e8f-9b1a-2c3d4e5f6a0f";

    /// [`wired`] whose outputs write or merge into [`OWN`].
    fn owning() -> Mixer {
        let m = wired();
        let (shape, wiring) = (m.shape().clone(), m.wiring.clone());
        Mixer::factory(shape, Vec::new(), wiring, BTreeSet::from([OWN.to_owned()]))()
    }

    /// No receiver reads a flow the mixer's outputs write or merge into,
    /// whatever path the command came by: it would feed the mix back.
    #[test]
    fn a_receiver_is_never_connected_to_an_own_output() {
        let mut m = owning();
        connect(&mut m, "n1", receiver("ch1"), conn(true, Some(OWN)));
        assert!(matches!(
            m.answer(&rest_key(), "n1"),
            Some(Answer::Invalid { .. })
        ));
        assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some("a1"));
        let s = Command::Schedule {
            id: "n2".into(),
            port: receiver("ch1"),
            active: conn(true, Some(OWN)),
            at: 1,
        };
        apply_as(&mut m, &s, Some(true));
        assert!(matches!(
            m.answer(&rest_key(), "n2"),
            Some(Answer::Invalid { .. })
        ));
        assert!(m.scheduled().is_empty());
        // The own flows are part of the configuration replicas compare.
        assert_ne!(owning().config_hash(), wired().config_hash());
    }

    fn schedule(m: &mut Mixer, id: &str, port: PortRef, active: Connection, at: u64) {
        let c = Command::Schedule {
            id: id.into(),
            port,
            active,
            at,
        };
        assert!(matches!(apply_as(m, &c, None), Effect::Changed(_)));
    }

    fn fire(m: &mut Mixer, id: &str, schedule: &str, generation: Option<bool>) -> Effect<Edits> {
        let c = Command::Fire {
            id: id.into(),
            schedule: schedule.into(),
        };
        apply_as(m, &c, generation)
    }

    /// A fired activation shows on `/active` as it was asked for: the
    /// mode and requested time of the request that scheduled it, the
    /// activation time when it fired; one scheduled without them as an
    /// absolute activation at that time.
    #[test]
    fn a_fired_activation_keeps_the_mode_and_requested_time_it_was_asked_with() {
        let activation = |m: &Mixer| {
            m.receiver("ch1")
                .and_then(|(c, _)| c.cloned())
                .and_then(|c| c.activation)
                .unwrap()
        };
        let mut m = wired();
        let relative = Connection {
            activation: Some(Activation {
                mode: "activate_scheduled_relative".into(),
                requested: Some("0:200000000".into()),
                time: String::new(),
            }),
            ..conn(true, Some(F1))
        };
        schedule(&mut m, "s1", receiver("ch1"), relative, 7_000_000_123);
        assert!(matches!(
            fire(&mut m, "f1", "s1", Some(true)),
            Effect::Changed(_)
        ));
        assert_eq!(
            activation(&m),
            Activation {
                mode: "activate_scheduled_relative".into(),
                requested: Some("0:200000000".into()),
                time: "7:123".into(),
            }
        );
        schedule(
            &mut m,
            "s2",
            receiver("ch1"),
            conn(true, Some(F2)),
            8_000_000_000,
        );
        assert!(matches!(
            fire(&mut m, "f2", "s2", Some(true)),
            Effect::Changed(_)
        ));
        assert_eq!(
            activation(&m),
            Activation {
                mode: "activate_scheduled_absolute".into(),
                requested: Some("8:0".into()),
                time: "8:0".into(),
            }
        );
    }

    /// A scheduled activation is state from when it is staged, so any
    /// leader fires it; it changes nothing until fired, fires once, is
    /// refused off the generation path and kept for the next try, and a
    /// cancel or a later activation of the port removes it.
    #[test]
    fn a_scheduled_activation_is_kept_until_fired_or_cancelled() {
        let mut m = wired();
        schedule(&mut m, "s1", receiver("ch1"), conn(true, Some(F1)), 1_000);
        assert_eq!(m.scheduled().len(), 1);
        assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some("a1"));
        let c = Command::Fire {
            id: "f0".into(),
            schedule: "s1".into(),
        };
        assert_eq!(
            m.reroutes(&origin("rest"), &serde_json::to_vec(&c).unwrap()),
            [0]
        );
        assert!(matches!(
            fire(&mut m, "f0", "s1", Some(false)),
            Effect::Unchanged
        ));
        assert_eq!(m.scheduled().len(), 1, "kept for the next try");
        assert!(matches!(
            fire(&mut m, "f1", "s1", Some(true)),
            Effect::Changed(_)
        ));
        assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some(F1));
        assert!(m.scheduled().is_empty());
        assert!(matches!(
            fire(&mut m, "f2", "s1", Some(true)),
            Effect::Unchanged
        ));

        // Cancelled, it never fires.
        schedule(&mut m, "s2", receiver("ch1"), conn(true, Some(F2)), 2_000);
        let cancel = Command::Unschedule {
            id: "u1".into(),
            port: receiver("ch1"),
        };
        assert!(matches!(
            apply_as(&mut m, &cancel, None),
            Effect::Changed(_)
        ));
        assert!(matches!(
            fire(&mut m, "f3", "s2", Some(true)),
            Effect::Unchanged
        ));
        assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some(F1));

        // An immediate activation supersedes it; a second schedule of
        // the port replaces the first.
        schedule(&mut m, "s3", receiver("ch1"), conn(true, Some(F2)), 3_000);
        schedule(&mut m, "s4", receiver("ch1"), conn(false, None), 4_000);
        assert_eq!(m.scheduled().len(), 1);
        connect(&mut m, "n1", receiver("ch1"), conn(true, Some(F2)));
        assert!(m.scheduled().is_empty());

        // A snapshot carries what is still pending.
        schedule(
            &mut m,
            "s5",
            PortRef::Sender("out-pgm".into()),
            conn(false, None),
            5_000,
        );
        let mut back = wired();
        back.load(&m.save()).unwrap();
        assert_eq!(back.scheduled(), m.scheduled());
    }

    /// A scheduled activation that can no longer be activated, here
    /// because the flow became one of the mixer's own, is dropped when it
    /// fires rather than kept and tried again for ever.
    #[test]
    fn a_schedule_that_cannot_fire_is_dropped() {
        let mut m = wired();
        schedule(&mut m, "s1", receiver("ch1"), conn(true, Some(OWN)), 1_000);
        let mut own = owning();
        own.load(&m.save()).unwrap();
        assert_eq!(own.scheduled().len(), 1);
        assert!(matches!(
            fire(&mut own, "f1", "s1", Some(true)),
            Effect::Changed(_)
        ));
        assert!(own.scheduled().is_empty());
        assert!(matches!(
            own.answer(&rest_key(), "f1"),
            Some(Answer::Invalid { .. })
        ));
        assert_eq!(own.targets().unwrap().flows[0].as_deref(), Some("a1"));
        // A fired one carries its time as its activation.
        schedule(
            &mut m,
            "s2",
            receiver("ch1"),
            conn(true, Some(F1)),
            7_000_000_001,
        );
        fire(&mut m, "f2", "s2", Some(true));
        let shown = m
            .receiver("ch1")
            .unwrap()
            .0
            .unwrap()
            .activation
            .clone()
            .unwrap();
        assert_eq!(
            (shown.mode.as_str(), shown.time.as_str()),
            ("activate_scheduled_absolute", "7:1")
        );
    }

    /// Leg flows an activation named over a configuration that wired the
    /// channel otherwise are dropped with its receiver's connection: the
    /// channel reads what is configured now.
    #[test]
    fn activated_legs_go_when_the_configured_flows_change() {
        let mut m = wired();
        connect(&mut m, "n1", receiver("ch1"), conn(true, Some(F1)));
        connect(&mut m, "n2", receiver("ch2"), conn(true, Some(F2)));
        let saved = m.save();
        let mut shape = small();
        shape["channels"][1]["altInput"] = json!(true);
        let wire = |a: &str, b: Option<&str>, domain: &str| Wiring {
            a: a.into(),
            b: b.map(Into::into),
            domain: domain.into(),
        };
        let mut rewired = Mixer::configured(
            Shape::parse(&shape).unwrap(),
            Vec::new(),
            vec![
                wire("x1", Some("b1"), "d"),
                wire("a2", Some("b2"), "e"),
                wire("a3", None, "d"),
            ],
        );
        rewired.load(&saved).unwrap();
        let flows = rewired.targets().unwrap().flows;
        assert_eq!(flows[0].as_deref(), Some("x1"), "ch1's wiring changed");
        assert_eq!(rewired.receiver("ch1").unwrap().0, None);
        assert_eq!(flows[1].as_deref(), Some(F2), "ch2's wiring did not");
    }

    /// A disable that names a flow keeps it as the flow of the selected
    /// input, so an enable without one reads it: IS-05 keeps the
    /// transport parameters of a disabled port.
    #[test]
    fn a_disable_naming_a_flow_sets_the_selected_inputs_flow() {
        let mut m = wired();
        connect(&mut m, "n1", receiver("ch1"), conn(false, Some(F1)));
        assert_eq!(m.targets().unwrap().flows[0], None);
        connect(&mut m, "n2", receiver("ch1"), conn(true, Some(F1)));
        assert_eq!(m.targets().unwrap().flows[0].as_deref(), Some(F1));
        assert_eq!(m.engine_view().wiring[0].a, F1);
    }
}
