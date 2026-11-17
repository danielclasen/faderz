//! The samplez engine under the replicated state: started with the gain
//! matrix of the state it opens with, it is sent every channel's strip,
//! then follows every applied delta with strip and gain changes and input
//! routes at the delta's `atIndex`, and its meters go to the primary
//! connections as meter frames.
//!
//! The engine keeps its own log ([`Engine::start`]): the state is a set
//! of one voter, so the order the replica applies deltas in is the order
//! every change reaches the engine.

use std::sync::Mutex;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use samplez::rt::ProcessStatus;
use samplez::{Counters, Engine, EntryError, GainError};
use samplez_dsp::StripParam;
use samplez_replica::Replica;
use samplez_service::ws::{Frame, Hub};
use samplez_service::{Counter, Gauge, Ops, Position, ReplicatedState};
use serde_json::Value;
use tokio::sync::Notify;
use tokio::time::{Instant, MissedTickBehavior};

use crate::meters::{Layout, Readings};
use crate::shape::Shape;
use crate::state::{Mixer, set_at};
use crate::{mix, strip};

/// The meter period: 25 frames per second.
pub const METER_PERIOD: Duration = Duration::from_millis(40);
/// How long a change waits before it is offered again while the engine
/// holds as many changes as it takes.
const FULL_RETRY: Duration = Duration::from_millis(5);
/// The least time between two resends: each sends the whole matrix, and
/// [`Audio::follow`] waits for it.
const RESEND_EVERY: Duration = Duration::from_secs(1);

pub struct Audio {
    engine: Engine,
    shape: Shape,
    /// Each channel's flows of input a and b.
    flows: Vec<(String, Option<String>)>,
    /// The flow each channel's input read when the engine started.
    started_on: Vec<String>,
    block: u64,
    /// The engine's ramp length, samples.
    ramp: u64,
    ops: Ops,
    /// Resends asked for or held back; [`Audio::follow`] notes the count
    /// it resent at, and readiness changes only under this lock.
    resends_asked: Mutex<u64>,
    resend: Notify,
    /// Each channel's meter gain in the state last applied
    /// ([`mix::meter_taps`]).
    taps: Mutex<Vec<mix::MeterTap>>,
    resends: Counter,
    resend_pending: Gauge,
}

/// What the engine was sent: every coefficient, and each input's flow.
struct Sent {
    matrix: Vec<Vec<f32>>,
    routed: Vec<String>,
    /// Each input's strip parameters ([`strip::params`]); `None` for one
    /// not sent, as all are at start: the engine's strip passes its input
    /// unchanged until set.
    strips: Vec<Vec<Option<StripParam>>>,
}

impl Sent {
    /// Forgets every coefficient sent, so all are sent again: NaN equals
    /// no gain. Takes each input's flow from the engine, which reports the
    /// flow it reads: a route to it again would cost a generation, and
    /// one built on a loaded host can be late again.
    fn clear(&mut self, engine: &Engine) {
        self.matrix.iter_mut().flatten().for_each(|g| *g = f32::NAN);
        self.strips.iter_mut().flatten().for_each(|p| *p = None);
        for (routed, input) in self.routed.iter_mut().zip(engine.status().inputs) {
            *routed = input.flow_id;
        }
    }
}

impl Audio {
    /// Starts the engine of `config`, silent (its matrix all zeros), each
    /// channel reading the flow its selected input names; [`Audio::follow`]
    /// sends the strips and then the gains of `state`.
    ///
    /// # Errors
    ///
    /// When the engine does not start.
    pub fn start(
        mut config: samplez::Config,
        process: ProcessStatus,
        shape: &Shape,
        alt_flows: Vec<Option<String>>,
        state: &Value,
        ops: Ops,
    ) -> Result<Audio, String> {
        let flows: Vec<(String, Option<String>)> = config
            .inputs
            .iter()
            .zip(alt_flows)
            .map(|(i, b)| (i.flow_id.clone(), b))
            .collect();
        let started_on = selected(shape, &flows, state);
        for (input, flow) in config.inputs.iter_mut().zip(&started_on) {
            input.flow_id.clone_from(flow);
        }
        let block = u64::from(config.block_size);
        let ramp = u64::from(config.ramp_samples);
        let engine = Engine::start(config, process, None).map_err(|e| format!("engine: {e}"))?;
        let metrics = ops.metrics();
        Ok(Audio {
            engine,
            shape: shape.clone(),
            flows,
            started_on,
            block,
            ramp,
            resends_asked: Mutex::new(0),
            resend: Notify::new(),
            taps: Mutex::new(mix::meter_taps(shape, state)),
            resends: metrics.counter(
                "faderz_engine_resends_total",
                "Times every gain and input route was sent to the engine again \
                 because it dropped entries as late or diverged.",
                &[],
            ),
            resend_pending: metrics.gauge(
                "faderz_engine_resend_pending",
                "1 while a resend to the engine is pending; /readyz fails then.",
                &[],
            ),
            ops,
        })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Stops the engine and releases its writers, which deletes their
    /// flows.
    pub fn stop(self) {
        self.engine.stop();
    }

    /// Sends every gain and route to the engine again, as absolute
    /// targets, at the earliest index it takes. Until that is done
    /// `/readyz` fails: the output may not follow the state.
    pub fn resend(&self) {
        self.resends.add(1);
        self.hold();
        self.resend.notify_one();
    }

    /// Marks a resend pending: `/readyz` fails until one sent after this
    /// is done.
    fn hold(&self) {
        let mut asked = self.resends_asked.lock().expect("resend lock poisoned");
        *asked += 1;
        self.resend_pending.set(1.0);
        self.ops.set_engine_started(false);
    }

    /// Follows the deltas `replica` applies from `state` at `revision`,
    /// the state the engine started with, and every resend asked for.
    /// The deltas waiting are taken together and sent as one diff against
    /// the newest state. Never returns.
    pub async fn follow(&self, replica: &Replica<Mixer>, mut state: Value, mut revision: u64) {
        // The engine starts silent, its strips passing their input as it
        // is: the strips go first and the gains once their ramps are over,
        // so every output fades in processed. Ready from then on.
        let mut sent = Sent {
            matrix: mix::matrix(&self.shape, &state)
                .into_iter()
                .map(|row| vec![0.0; row.len()])
                .collect(),
            routed: self.started_on.clone(),
            strips: self
                .shape
                .channels
                .iter()
                .map(|c| vec![None; strip::params(&Value::Null, c.width).len()])
                .collect(),
        };
        self.apply(&state, &mut sent, 0, true).await;
        // A resend asked for meanwhile marks it ready once done. The lock
        // is held over the check and the store, so a hold() cannot fall
        // between them and be overwritten.
        {
            let asked = self.resends_asked.lock().expect("resend lock poisoned");
            if *asked == 0 {
                self.ops.set_engine_started(true);
            }
        }
        loop {
            let resent = tokio::select! {
                () = replica.applied(revision + 1) => None,
                () = self.resend.notified() => {
                    sent.clear(&self.engine);
                    Some(*self.resends_asked.lock().expect("resend lock poisoned"))
                }
            };
            let since = Position {
                epoch: replica.epoch(),
                revision,
            };
            let mut at = 0;
            match replica.deltas_since(since).await {
                Some(deltas) => {
                    for d in deltas {
                        let Ok(delta) = serde_json::from_str::<Value>(&d.text) else {
                            (state, revision) = state_of(replica);
                            break;
                        };
                        revision = d.revision;
                        at = at.max(delta["atIndex"].as_u64().unwrap_or(0));
                        for c in delta["changes"].as_array().into_iter().flatten() {
                            if let Some(path) = c["path"].as_str() {
                                set_at(&mut state, path, c["value"].clone());
                            }
                        }
                    }
                }
                // No longer held: taken from the state, as soon as the
                // engine takes it.
                None => (state, revision) = state_of(replica),
            }
            self.apply(&state, &mut sent, at, false).await;
            if let Some(n) = resent {
                let asked = self.resends_asked.lock().expect("resend lock poisoned");
                if *asked == n {
                    self.resend_pending.set(0.0);
                    self.ops.set_engine_started(true);
                }
            }
        }
    }

    /// Sends the engine every coefficient of `state`'s matrix that differs
    /// from what it was sent, and a route for every channel whose selected
    /// flow differs, all at `at` rounded up to a block. When the engine no
    /// longer takes that index, the rest of the diff goes at the earliest
    /// one it does; while it holds as many changes as it takes, the next
    /// waits. With `settle`, the gains wait for the strips' ramps.
    async fn apply(&self, state: &Value, sent: &mut Sent, at: u64, settle: bool) {
        let mut at = at.next_multiple_of(self.block);
        for (slot, (c, sent)) in self.shape.channels.iter().zip(&mut sent.strips).enumerate() {
            let next = strip::params(&state["channels"][&c.id], c.width);
            for (p, n) in sent.iter_mut().zip(next) {
                if *p != Some(n)
                    && self
                        .send(&mut at, |at| self.engine.set_strip(slot, n, at))
                        .await
                {
                    *p = Some(n);
                }
            }
        }
        if settle {
            at = (at + self.ramp).next_multiple_of(self.block);
        }
        *self.taps.lock().expect("taps lock poisoned") = mix::meter_taps(&self.shape, state);
        let next = mix::matrix(&self.shape, state);
        for (input, (row, new)) in sent.matrix.iter_mut().zip(next).enumerate() {
            for (output, (g, n)) in row.iter_mut().zip(new).enumerate() {
                if *g != n
                    && self
                        .send(&mut at, |at| self.engine.set_gain(input, output, n, at))
                        .await
                {
                    *g = n;
                }
            }
        }
        for (slot, flow) in selected(&self.shape, &self.flows, state)
            .into_iter()
            .enumerate()
        {
            while flow != sent.routed[slot] {
                match self.engine.route(slot, &flow, at) {
                    Ok(_) => sent.routed[slot].clone_from(&flow),
                    Err(EntryError::Full) => tokio::time::sleep(FULL_RETRY).await,
                    Err(e) => {
                        tracing::warn!(slot, flow, "input route refused: {e}");
                        break;
                    }
                }
            }
        }
    }

    /// Sends one change through `set` at `at`. When the engine no longer
    /// takes that index, it and the rest of the diff go at the earliest
    /// one it does; while it holds as many changes as it takes, it waits.
    /// Whether the engine took it.
    async fn send(&self, at: &mut u64, set: impl Fn(u64) -> Result<(), GainError>) -> bool {
        loop {
            match set(*at) {
                Ok(()) => return true,
                Err(GainError::TooNear { earliest, .. }) => {
                    tracing::debug!(at, earliest, "the rest of a diff applies later");
                    *at = earliest.next_multiple_of(self.block);
                }
                Err(GainError::Full) => tokio::time::sleep(FULL_RETRY).await,
                Err(GainError::Invalid(e)) => {
                    tracing::warn!("engine change refused: {e}");
                    return false;
                }
            }
        }
    }

    /// Publishes a meter frame of the engine's meters every meter period,
    /// numbered by the periods since the first; beats the ops heartbeat
    /// while the RT thread's advances; and asks for a resend
    /// when the engine drops entries as late or reports another
    /// divergence. Never returns.
    pub async fn meter(&self, hub: &Hub, replica: &str) {
        let layout = Layout::of(&self.shape);
        let first = Instant::now();
        let mut ticks = tokio::time::interval_at(first, METER_PERIOD);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let counters = self.engine.counters();
        let mut drops = Drops::new(dropped(counters), self.engine.status().diverged);
        let mut beats = 0;
        loop {
            let tick = ticks.tick().await;
            let now = counters.heartbeat_blocks.load(Relaxed);
            if now != beats {
                beats = now;
                self.ops.heartbeat().beat();
            }
            let status = self.engine.status();
            let late = dropped(counters);
            match drops.tick(late, &status.diverged, tick.into_std()) {
                Act::None => {}
                Act::Hold => self.hold(),
                Act::Resend { log } => {
                    if log {
                        tracing::warn!(?late, diverged = ?status.diverged, "engine entries dropped: resending");
                    }
                    self.resend();
                }
            }
            let periods = (tick - first).as_nanos() / METER_PERIOD.as_nanos();
            // The sequence wraps at 2^32 periods.
            let sequence = periods as u32;
            let taps = self.taps.lock().expect("taps lock poisoned").clone();
            let frame = layout.frame(replica, sequence, &Readings::of(&status, &taps));
            hub.publish(Frame::Meters(frame.into()));
        }
    }
}

/// What [`Drops::tick`] asks for.
#[derive(Debug, PartialEq, Eq)]
enum Act {
    None,
    /// Entries were dropped; the resend waits for [`RESEND_EVERY`].
    Hold,
    /// Resend now; `log` for the first resend of an episode.
    Resend {
        log: bool,
    },
}

/// The engine's counts of entries it did not apply: `command_late` and
/// `strip_refused`.
fn dropped(c: &Counters) -> [u64; 2] {
    [c.command_late.load(Relaxed), c.strip_refused.load(Relaxed)]
}

/// Whether the engine dropped entries: a count of [`dropped`] rose or its
/// divergence changed. A late route needs no resend: samplez holds
/// the slot and swaps the generation in when it arrives.
struct Drops {
    late: [u64; 2],
    diverged: Option<String>,
    /// Drops seen and not resent yet.
    pending: bool,
    last: Option<std::time::Instant>,
    /// Resends follow each other at most [`RESEND_EVERY`] apart.
    episode: bool,
}

impl Drops {
    fn new(late: [u64; 2], diverged: Option<String>) -> Drops {
        Drops {
            late,
            diverged,
            pending: false,
            last: None,
            episode: false,
        }
    }

    /// The engine's [`dropped`] counts and `diverged` at `now`.
    fn tick(&mut self, late: [u64; 2], diverged: &Option<String>, now: std::time::Instant) -> Act {
        let quiet = self.last.is_none_or(|t| now - t >= RESEND_EVERY);
        let seen = late != self.late || *diverged != self.diverged;
        if seen {
            self.late = late;
            self.diverged.clone_from(diverged);
        }
        if !(seen || self.pending) {
            if quiet {
                self.episode = false;
            }
            return Act::None;
        }
        if !quiet {
            let first = !self.pending;
            self.pending = true;
            return if first { Act::Hold } else { Act::None };
        }
        self.pending = false;
        self.last = Some(now);
        let log = !self.episode;
        self.episode = true;
        Act::Resend { log }
    }
}

/// The flow each channel's selected input reads.
fn selected(shape: &Shape, flows: &[(String, Option<String>)], state: &Value) -> Vec<String> {
    shape
        .channels
        .iter()
        .zip(flows)
        .map(
            |(c, (a, b))| match (state["channels"][&c.id]["input"].as_str(), b) {
                (Some("b"), Some(b)) => b.clone(),
                _ => a.clone(),
            },
        )
        .collect()
}

/// The state and revision `replica` holds now.
pub fn state_of(replica: &Replica<Mixer>) -> (Value, u64) {
    replica
        .store()
        .with_state(|sm| (sm.product.state().clone(), sm.revision))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rise of a drop count or a new divergence resends, at most once
    /// per RESEND_EVERY, logged once per episode; generation_late is not
    /// read at all.
    #[test]
    fn dropped_entries_resend_with_a_backoff() {
        let t0 = std::time::Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut d = Drops::new([0, 0], None);
        assert_eq!(d.tick([0, 0], &None, at(0)), Act::None);
        assert_eq!(d.tick([1, 0], &None, at(40)), Act::Resend { log: true });
        assert_eq!(d.tick([1, 0], &None, at(80)), Act::None);
        // Another drop within the second waits, and readiness fails meanwhile.
        assert_eq!(d.tick([2, 0], &None, at(120)), Act::Hold);
        assert_eq!(d.tick([3, 0], &None, at(160)), Act::None);
        assert_eq!(d.tick([3, 0], &None, at(1_040)), Act::Resend { log: false });
        // Quiet for a second: the next drop starts a new episode.
        assert_eq!(d.tick([3, 0], &None, at(2_100)), Act::None);
        let diverged = Some("entry 9 late".to_owned());
        assert_eq!(
            d.tick([3, 0], &diverged, at(2_140)),
            Act::Resend { log: true }
        );
        assert_eq!(d.tick([3, 0], &diverged, at(3_200)), Act::None);
    }

    /// A strip parameter the RT thread refused counts as a drop.
    #[test]
    fn a_refused_strip_entry_resends() {
        let t0 = std::time::Instant::now();
        let c = Counters::default();
        let mut d = Drops::new(dropped(&c), None);
        assert_eq!(d.tick(dropped(&c), &None, t0), Act::None);
        c.strip_refused.store(1, Relaxed);
        let at = t0 + Duration::from_millis(40);
        assert_eq!(d.tick(dropped(&c), &None, at), Act::Resend { log: true });
        c.command_late.store(1, Relaxed);
        let at = t0 + Duration::from_millis(1_100);
        assert_eq!(d.tick(dropped(&c), &None, at), Act::Resend { log: false });
    }
}
