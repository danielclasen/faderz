//! The samplez engine under the replicated state: started with the gain
//! matrix of the state it opens with, it follows every applied delta with
//! gain changes and input routes at the delta's `atIndex`, and its output
//! meters go to the primary connections as meter frames.
//!
//! The engine keeps its own log ([`Engine::start`]): the state is a set
//! of one voter, so the order the replica applies deltas in is the order
//! every change reaches the engine.

use std::time::Duration;

use samplez::rt::ProcessStatus;
use samplez::{Engine, EntryError, GainError};
use samplez_replica::Replica;
use samplez_service::ws::{Frame, Hub};
use samplez_service::{Ops, Position, ReplicatedState};
use serde_json::Value;

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
    block: u64,
}

impl Audio {
    /// Starts the engine of `config` with the gain matrix of `state` and
    /// routes each channel that selects input b to its flow.
    ///
    /// # Errors
    ///
    /// When the engine does not start or refuses a route.
    pub fn start(
        mut config: samplez::Config,
        process: ProcessStatus,
        shape: &Shape,
        alt_flows: Vec<Option<String>>,
        state: &Value,
    ) -> Result<Audio, String> {
        config.matrix = mix::matrix(shape, state);
        let flows = config
            .inputs
            .iter()
            .zip(alt_flows)
            .map(|(i, b)| (i.flow_id.clone(), b))
            .collect();
        let block = u64::from(config.block_size);
        let engine = Engine::start(config, process, None).map_err(|e| format!("engine: {e}"))?;
        let audio = Audio {
            engine,
            shape: shape.clone(),
            flows,
            block,
        };
        for (slot, flow) in audio.selected(state).into_iter().enumerate() {
            if flow != audio.flows[slot].0 {
                audio
                    .engine
                    .route(slot, &flow, 0)
                    .map_err(|e| format!("routing input {slot} to {flow}: {e}"))?;
            }
        }
        Ok(audio)
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Stops the engine and releases its writers, which deletes their
    /// flows.
    pub fn stop(self) {
        self.engine.stop();
    }

    /// The flow each channel's selected input reads.
    fn selected(&self, state: &Value) -> Vec<String> {
        self.shape
            .channels
            .iter()
            .zip(&self.flows)
            .map(
                |(c, (a, b))| match (state["channels"][&c.id]["input"].as_str(), b) {
                    (Some("b"), Some(b)) => b.clone(),
                    _ => a.clone(),
                },
            )
            .collect()
    }

    /// Follows the deltas `replica` applies from `state` at `revision`,
    /// the state the engine started with. Never returns.
    pub async fn follow(&self, replica: &Replica<Mixer>, mut state: Value, mut revision: u64) {
        let mut matrix = mix::matrix(&self.shape, &state);
        let mut routed = self.selected(&state);
        loop {
            replica.applied(revision + 1).await;
            let since = Position {
                epoch: replica.epoch(),
                revision,
            };
            match replica.deltas_since(since).await {
                Some(deltas) => {
                    for d in deltas {
                        revision = d.revision;
                        let Ok(delta) = serde_json::from_str::<Value>(&d.text) else {
                            continue;
                        };
                        let Some(at) = delta["atIndex"].as_u64() else {
                            continue;
                        };
                        for c in delta["changes"].as_array().into_iter().flatten() {
                            if let Some(path) = c["path"].as_str() {
                                set_at(&mut state, path, c["value"].clone());
                            }
                        }
                        self.apply(&state, &mut matrix, &mut routed, at).await;
                    }
                }
                // No longer held: taken from the state, as soon as the
                // engine takes it.
                None => {
                    (state, revision) = state_of(replica);
                    self.apply(&state, &mut matrix, &mut routed, 0).await;
                }
            }
        }
    }

    /// Sends the engine every coefficient of `state`'s matrix that differs
    /// from `matrix`, and a route for every channel whose selected flow
    /// differs from `routed`, at `at` rounded up to a block or, when the
    /// engine no longer takes that index, at the earliest it does.
    async fn apply(&self, state: &Value, matrix: &mut [Vec<f32>], routed: &mut [String], at: u64) {
        let at = at.next_multiple_of(self.block);
        let next = mix::matrix(&self.shape, state);
        for (input, (row, new)) in matrix.iter_mut().zip(next).enumerate() {
            for (output, (g, n)) in row.iter_mut().zip(new).enumerate() {
                if *g != n {
                    self.set_gain(input, output, n, at).await;
                    *g = n;
                }
            }
        }
        for (slot, flow) in self.selected(state).into_iter().enumerate() {
            while flow != routed[slot] {
                match self.engine.route(slot, &flow, at) {
                    Ok(_) => routed[slot].clone_from(&flow),
                    Err(EntryError::Full) => tokio::time::sleep(FULL_RETRY).await,
                    Err(e) => {
                        tracing::warn!(slot, flow, "input route refused: {e}");
                        break;
                    }
                }
            }
        }
    }

    async fn set_gain(&self, input: usize, output: usize, gain: f32, mut at: u64) {
        loop {
            match self.engine.set_gain(input, output, gain, at) {
                Ok(()) => return,
                Err(GainError::TooNear { earliest, .. }) => {
                    tracing::debug!(at, earliest, "a gain change applies late");
                    at = earliest.next_multiple_of(self.block);
                }
                Err(GainError::Full) => tokio::time::sleep(FULL_RETRY).await,
                Err(GainError::Invalid(e)) => {
                    tracing::warn!(input, output, gain, "gain change refused: {e}");
                    return;
                }
            }
        }
    }

    /// Publishes a meter frame of the engine's output meters every meter
    /// period, and beats the ops heartbeat while the RT thread's advances.
    /// Never returns.
    pub async fn meter(&self, hub: &Hub, ops: &Ops, replica: &str) {
        let layout = Layout::of(&self.shape);
        let mut ticks = tokio::time::interval(METER_PERIOD);
        let mut sequence = 0u32;
        let mut beats = 0;
        loop {
            ticks.tick().await;
            let now = self
                .engine
                .counters()
                .heartbeat_blocks
                .load(std::sync::atomic::Ordering::Relaxed);
            if now != beats {
                beats = now;
                ops.heartbeat().beat();
            }
            let status = self.engine.status();
            let frame = layout.frame(replica, sequence, &status.outputs);
            hub.publish(Frame::Meters(frame.into()));
            sequence = sequence.wrapping_add(1);
        }
    }
}

/// The state and revision `replica` holds now.
pub fn state_of(replica: &Replica<Mixer>) -> (Value, u64) {
    replica
        .store()
        .with_state(|sm| (sm.product.state().clone(), sm.revision))
}
