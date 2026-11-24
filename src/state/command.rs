//! The commands the replicated log carries, and what applying one
//! answers and changes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Connection, Console, ConsolePut, PortRef};

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
    /// The channels, buses and auxes a `Recall` set to a delay of 0:
    /// their stored delay is past their delay line.
    Recalled { delays: Vec<String> },
}

/// What one command or expiry changed, for [`Product::delta`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Edits {
    pub(super) values: Vec<(String, Value)>,
    pub(super) owners: Vec<(String, Value)>,
    /// The delta's `origins` entry: client commands only.
    pub(super) origin: Option<Value>,
    /// Control requests the replicas deliver to the holder's connection,
    /// whichever replica it is on; the server takes them out of the
    /// delta before the clients see it.
    pub(super) requests: Vec<Value>,
}
