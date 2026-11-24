//! The IS-05 connections: each port's last activation, the leg flows
//! activations name, scheduled activations and the version each change
//! stamps.

use std::collections::BTreeMap;

use samplez_replica::entry::SessionKey;
use samplez_replica::state_machine::{Apply, Effect};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::paths::{escape, tokens};
use super::{Answer, Edits, Mixer};
use crate::mix;

#[cfg(doc)]
use super::Command;

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
    pub(super) receivers: BTreeMap<String, Connection>,
    pub(super) senders: BTreeMap<String, Connection>,
    pub(super) legs: BTreeMap<String, Legs>,
    /// The scheduled activations, one per port at most.
    #[serde(default)]
    pub(super) scheduled: Vec<Scheduled>,
    /// The change that last changed each port's `/active`, by receiver
    /// and by sender; none for a port no change has changed.
    #[serde(default)]
    pub(super) receiver_changes: BTreeMap<String, PortChange>,
    #[serde(default)]
    pub(super) sender_changes: BTreeMap<String, PortChange>,
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

impl Scheduled {
    /// The activation it records when it fires ([`Command::Fire`]).
    pub fn fired(&self) -> Activation {
        let at = tai_text(self.at);
        let (mode, requested) = match &self.active.activation {
            Some(a) => (a.mode.clone(), a.requested.clone()),
            None => ("activate_scheduled_absolute".into(), Some(at.clone())),
        };
        Activation {
            mode,
            requested,
            time: at,
        }
    }
}

/// A channel's flows of input a and b that activations named.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Legs {
    pub(super) a: Option<String>,
    pub(super) b: Option<String>,
    /// The channel's configured flows of input a and b when an activation
    /// named one: a configuration that wires others drops the legs.
    #[serde(default)]
    pub(super) wired: Option<(String, Option<String>)>,
}

impl Mixer {
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
    pub(super) fn connect_reroutes(&self, channel: &str, active: &Connection) -> Vec<u32> {
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
    pub(super) fn connect(
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
    pub(super) fn schedule(
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

    pub(super) fn unschedule(&mut self, port: &PortRef) -> Effect<Edits> {
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
    pub(super) fn fire(
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
        let active = Connection {
            activation: Some(s.fired()),
            ..s.active.clone()
        };
        self.connect(session, id, &s.port, &active, ctx)
    }

    /// The scheduled activations, in the order they were taken.
    pub fn scheduled(&self) -> &[Scheduled] {
        &self.ports.scheduled
    }

    /// Whether scheduled activation `s` activates when it fires, as
    /// [`Mixer::fire`] checks it, rather than being dropped.
    pub fn fires(&self, s: &Scheduled) -> bool {
        self.check_connection(&s.port, &s.active).is_ok()
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
    pub(super) fn stamp(&mut self, id: &str, at_index: Option<u64>) {
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
    pub(super) fn follow_switches(&mut self, changed: &[(String, Value)]) {
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
}

/// The lowercase canonical UUID form MXL flow and NMOS ids take.
fn is_uuid(v: &str) -> bool {
    v.len() == 36
        && v.bytes().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(&c),
        })
}
