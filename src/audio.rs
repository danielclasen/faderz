//! The samplez engine under the replicated state: started with the gain
//! matrix of the state it opens with, it follows every applied delta with
//! gain changes and input routes at the delta's `atIndex`, and its output
//! meters go to the primary connections as meter frames.
//!
//! The engine keeps its own log ([`Engine::start`]): the state is a set
//! of one voter, so the order the replica applies deltas in is the order
//! every change reaches the engine.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use samplez::rt::ProcessStatus;
use samplez::{Engine, EntryError, GainError};
use samplez_replica::Replica;
use samplez_service::ws::{Frame, Hub};
use samplez_service::{Counter, Gauge, Ops, Position, ReplicatedState};
use serde_json::Value;
use tokio::sync::Notify;
use tokio::time::{Instant, MissedTickBehavior};

use crate::meters::Layout;
use crate::mix;
use crate::shape::Shape;
use crate::state::{Mixer, set_at};

/// The meter period: 25 frames per second.
pub const METER_PERIOD: Duration = Duration::from_millis(40);
/// How long a change waits before it is offered again while the engine
/// holds as many changes as it takes.
const FULL_RETRY: Duration = Duration::from_millis(5);

pub struct Audio {
    engine: Engine,
    shape: Shape,
    /// Each channel's flows of input a and b.
    flows: Vec<(String, Option<String>)>,
    /// The flow each channel's input read when the engine started.
    started_on: Vec<String>,
    block: u64,
    ops: Ops,
    /// Resends asked for; [`Audio::follow`] notes the count it resent at.
    resends_asked: AtomicU64,
    resend: Notify,
    resends: Counter,
    resend_pending: Gauge,
}

/// What the engine was sent: every coefficient, and each input's flow.
struct Sent {
    matrix: Vec<Vec<f32>>,
    routed: Vec<String>,
}

impl Sent {
    /// Forgets every coefficient sent, so all are sent again: NaN equals
    /// no gain. Takes each input's flow from the engine, which reports the
    /// flow it reads: a route to it again would cost a generation, and
    /// one built on a loaded host can be late again.
    fn clear(&mut self, engine: &Engine) {
        self.matrix.iter_mut().flatten().for_each(|g| *g = f32::NAN);
        for (routed, input) in self.routed.iter_mut().zip(engine.status().inputs) {
            *routed = input.flow_id;
        }
    }
}

impl Audio {
    /// Starts the engine of `config` with the gain matrix of `state`, each
    /// channel reading the flow its selected input names.
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
        config.matrix = mix::matrix(shape, state);
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
        let engine = Engine::start(config, process, None).map_err(|e| format!("engine: {e}"))?;
        let metrics = ops.metrics();
        Ok(Audio {
            engine,
            shape: shape.clone(),
            flows,
            started_on,
            block,
            resends_asked: AtomicU64::new(0),
            resend: Notify::new(),
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
        self.resends_asked.fetch_add(1, Relaxed);
        self.resends.add(1);
        self.resend_pending.set(1.0);
        self.ops.set_engine_started(false);
        self.resend.notify_one();
    }

    /// Follows the deltas `replica` applies from `state` at `revision`,
    /// the state the engine started with, and every resend asked for.
    /// The deltas waiting are taken together and sent as one diff against
    /// the newest state. Never returns.
    pub async fn follow(&self, replica: &Replica<Mixer>, mut state: Value, mut revision: u64) {
        let mut sent = Sent {
            matrix: mix::matrix(&self.shape, &state),
            routed: self.started_on.clone(),
        };
        loop {
            let resent = tokio::select! {
                () = replica.applied(revision + 1) => None,
                () = self.resend.notified() => {
                    sent.clear(&self.engine);
                    Some(self.resends_asked.load(Relaxed))
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
            self.apply(&state, &mut sent, at).await;
            if resent.is_some_and(|n| n == self.resends_asked.load(Relaxed)) {
                self.resend_pending.set(0.0);
                self.ops.set_engine_started(true);
            }
        }
    }

    /// Sends the engine every coefficient of `state`'s matrix that differs
    /// from what it was sent, and a route for every channel whose selected
    /// flow differs, all at `at` rounded up to a block. When the engine no
    /// longer takes that index, the rest of the diff goes at the earliest
    /// one it does; while it holds as many changes as it takes, the next
    /// waits.
    async fn apply(&self, state: &Value, sent: &mut Sent, at: u64) {
        let mut at = at.next_multiple_of(self.block);
        let next = mix::matrix(&self.shape, state);
        for (input, (row, new)) in sent.matrix.iter_mut().zip(next).enumerate() {
            for (output, (g, n)) in row.iter_mut().zip(new).enumerate() {
                if *g == n {
                    continue;
                }
                loop {
                    match self.engine.set_gain(input, output, n, at) {
                        Ok(()) => {
                            *g = n;
                            break;
                        }
                        Err(GainError::TooNear { earliest, .. }) => {
                            tracing::debug!(at, earliest, "the rest of a diff applies later");
                            at = earliest.next_multiple_of(self.block);
                        }
                        Err(GainError::Full) => tokio::time::sleep(FULL_RETRY).await,
                        Err(GainError::Invalid(e)) => {
                            tracing::warn!(input, output, n, "gain change refused: {e}");
                            break;
                        }
                    }
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

    /// Publishes a meter frame of the engine's output meters every meter
    /// period, numbered by the periods since the first; beats the ops
    /// heartbeat while the RT thread's advances; and asks for a resend
    /// when the engine drops entries as late or reports another
    /// divergence. Never returns.
    pub async fn meter(&self, hub: &Hub, replica: &str) {
        let layout = Layout::of(&self.shape);
        let first = Instant::now();
        let mut ticks = tokio::time::interval_at(first, METER_PERIOD);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let counters = self.engine.counters();
        let late = || counters.command_late.load(Relaxed) + counters.generation_late.load(Relaxed);
        let (mut beats, mut lates, mut diverged) = (0, late(), self.engine.status().diverged);
        loop {
            let tick = ticks.tick().await;
            let now = counters.heartbeat_blocks.load(Relaxed);
            if now != beats {
                beats = now;
                self.ops.heartbeat().beat();
            }
            let status = self.engine.status();
            let now = late();
            if now != lates || status.diverged != diverged {
                tracing::warn!(late = now, diverged = ?status.diverged, "engine entries dropped: resending");
                (lates, diverged) = (now, status.diverged.clone());
                self.resend();
            }
            let periods = (tick - first).as_nanos() / METER_PERIOD.as_nanos();
            // The sequence wraps at 2^32 periods.
            let sequence = periods as u32;
            let frame = layout.frame(replica, sequence, &status.outputs);
            hub.publish(Frame::Meters(frame.into()));
        }
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
