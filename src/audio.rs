//! The samplez engine under the replicated state: started with the gain
//! matrix of the state it opens with, it is sent every channel's strip,
//! then follows every applied delta with strip, duck and gain changes and
//! input routes at the delta's `atIndex`, and its meters go to the primary
//! connections as meter frames.
//!
//! In a set of one the engine keeps its own log ([`Engine::start`]): the
//! order the replica applies deltas in is the order every change reaches
//! the engine. In a set of more ([`Audio::start_set`]) the engine follows
//! the replicated log ([`Engine::start_replicated`]): samplez-replica
//! hands it the entries each log entry derived from the state, and this
//! module only meters it and reports its inputs' health to the log.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use openraft::ServerState;
use samplez::rt::ProcessStatus;
use samplez::{
    Counters, DuckSetting, DuckTargets, Engine, Entry, EntryError, GainError, InputState,
    InputStatus, Replication, Report, Targets,
};
use samplez_dsp::StripParam;
use samplez_replica::Replica;
use samplez_replica::bridge::EngineAudio;
use samplez_replica::contact::Timing;
use samplez_replica::entry::Body;
use samplez_replica::state_machine::StateMachine;
use samplez_service::ws::{Frame, Hub};
use samplez_service::{Counter, Gauge, Metrics, Ops, Position, ReplicatedState};
use serde_json::{Map, Value, json};
use tokio::sync::Notify;
use tokio::time::{Instant, MissedTickBehavior};

use crate::config::Set;
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

/// A replica's engine in a set of more than one ([`Audio::start_set`]).
pub struct SetEngine {
    pub audio: Audio,
    /// What the replica's contact drives the engine through.
    pub bridge: EngineAudio,
    /// The engine's input-health reports ([`report_inputs`]).
    pub reports: Receiver<Report>,
    /// The contact timers the bridge was made with ([`timing`]).
    pub timing: Timing,
}

/// A set replica's contact timers: samplez-replica's defaults, with the
/// engine's block, margin, hold and contact window and the set's
/// `maxApplyLeadMs`.
pub fn timing(engine: &samplez::Config, set: &Set) -> Timing {
    Timing {
        margin: u64::from(engine.reader_margin_samples),
        hold: Duration::from_millis(u64::from(engine.hold_ms)),
        contact_window: Duration::from_millis(u64::from(engine.contact_window_ms)),
        max_apply_lead: set.max_apply_lead,
        ..Timing::defaults(u64::from(engine.block_size))
    }
}

pub struct Audio {
    engine: Arc<Engine>,
    /// The engine follows a set's replicated log: nothing is sent to it
    /// from here.
    replicated: bool,
    shape: Shape,
    /// Each channel's flows of input a and b.
    wiring: Vec<mix::Wiring>,
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
    /// [`Audio::resends`] as a count the `counters` message reads.
    resent: AtomicU64,
    resend_pending: Gauge,
    inputs: Mutex<InputHealth>,
}

/// What the engine was sent: every coefficient, and each input's flow.
struct Sent {
    matrix: Vec<Vec<f32>>,
    routed: Vec<String>,
    /// Each input's strip parameters ([`strip::params`]); `None` for one
    /// not sent, as all are at start: the engine's strip passes its input
    /// unchanged until set.
    strips: Vec<Vec<Option<StripParam>>>,
    /// The duck groups and masks the engine holds, as targets; `None`
    /// after a resend asked for, when every setting and mask goes again.
    ducks: Option<Targets>,
}

impl Sent {
    /// Forgets every coefficient sent, so all are sent again: NaN equals
    /// no gain. Takes each input's flow from the engine, which reports the
    /// flow it reads: a route to it again would cost a generation, and
    /// one built on a loaded host can be late again.
    fn clear(&mut self, engine: &Engine) {
        self.matrix.iter_mut().flatten().for_each(|g| *g = f32::NAN);
        self.strips.iter_mut().flatten().for_each(|p| *p = None);
        self.ducks = None;
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
        wiring: Vec<mix::Wiring>,
        state: &Value,
        ops: Ops,
    ) -> Result<Audio, String> {
        let started_on = mix::selected(shape, &wiring, state);
        for (input, flow) in config.inputs.iter_mut().zip(&started_on) {
            input.flow_id.clone_from(flow);
        }
        let block = u64::from(config.block_size);
        let ramp = u64::from(config.ramp_samples);
        let engine = Engine::start(config, process, None).map_err(|e| format!("engine: {e}"))?;
        let mut audio = Audio::new(Arc::new(engine), shape, wiring, state, ops);
        (audio.started_on, audio.block, audio.ramp) = (started_on, block, ramp);
        Ok(audio)
    }

    /// Starts the engine of `config` as a replica of `set`, following the
    /// replicated log, each channel reading the flow its selected input in
    /// `state` names.
    ///
    /// # Errors
    ///
    /// When the engine does not start.
    pub fn start_set(
        mut config: samplez::Config,
        process: ProcessStatus,
        shape: &Shape,
        wiring: Vec<mix::Wiring>,
        state: &Value,
        ops: Ops,
        set: &Set,
    ) -> Result<SetEngine, String> {
        let timing = timing(&config, set);
        for (input, flow) in config
            .inputs
            .iter_mut()
            .zip(mix::selected(shape, &wiring, state))
        {
            input.flow_id = flow;
        }
        let (reports, reports_rx) = std::sync::mpsc::sync_channel(64);
        let (statuses, statuses_rx) = std::sync::mpsc::sync_channel(1 << 12);
        let (hashes, hashes_rx) = std::sync::mpsc::sync_channel(1 << 10);
        let replication = Replication {
            voters: usize::from(set.voters.replicas),
            reports,
            statuses: Some(statuses),
            hashes: Some(hashes),
        };
        let engine = Engine::start_replicated(config, process, None, replication)
            .map_err(|e| format!("engine: {e}"))?;
        let engine = Arc::new(engine);
        let bridge = EngineAudio::new(engine.clone(), statuses_rx, hashes_rx, &timing);
        let mut audio = Audio::new(engine, shape, wiring, state, ops);
        audio.replicated = true;
        Ok(SetEngine {
            audio,
            bridge,
            reports: reports_rx,
            timing,
        })
    }

    fn new(
        engine: Arc<Engine>,
        shape: &Shape,
        wiring: Vec<mix::Wiring>,
        state: &Value,
        ops: Ops,
    ) -> Audio {
        let metrics = ops.metrics();
        Audio {
            engine,
            replicated: false,
            started_on: Vec::new(),
            block: 0,
            ramp: 0,
            shape: shape.clone(),
            wiring,
            resends_asked: Mutex::new(0),
            resend: Notify::new(),
            taps: Mutex::new(mix::meter_taps(shape, state)),
            resent: AtomicU64::new(0),
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
            inputs: Mutex::new(InputHealth::new(metrics, shape)),
            ops,
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Stops the engine and releases its writers, which deletes their
    /// flows. A set's engine is held by the replica's contact as well and
    /// stops once that lets it go, with the process.
    pub fn stop(self) {
        if let Some(engine) = Arc::into_inner(self.engine) {
            engine.stop();
        }
    }

    /// Sends every gain and route to the engine again, as absolute
    /// targets, at the earliest index it takes. Until that is done
    /// `/readyz` fails: the output may not follow the state.
    pub fn resend(&self) {
        self.resends.add(1);
        self.resent.fetch_add(1, Relaxed);
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
            // What an engine holds before any duck entry: every group at
            // its defaults and every mask 0.
            ducks: Some(Targets {
                ducks: vec![DuckTargets::default(); self.shape.ducks.len()],
                ..Targets::default()
            }),
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

    /// Keeps the meter taps on a set's replicated state. A set's engine
    /// follows the log through samplez-replica and [`Audio::follow`] does not
    /// run, so nothing else updates them: without this every post-fader meter
    /// stays at the fader, mute and VCA gains the replica started with. Wakes
    /// on each applied revision, and every second besides, so a revision a
    /// rebuild moved backwards cannot stall it. Never returns.
    pub async fn follow_taps(&self, replica: &Replica<Mixer>, mut revision: u64) {
        loop {
            tokio::select! {
                () = replica.applied(revision + 1) => {}
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
            let (taps, at) = replica.store().with_state(|sm| {
                (
                    mix::meter_taps(&self.shape, sm.product.state()),
                    sm.revision,
                )
            });
            revision = at;
            *self.taps.lock().expect("taps lock poisoned") = taps;
        }
    }

    /// Sends the engine every strip parameter, duck setting, duck mask and
    /// coefficient of `state`'s targets ([`mix::targets`]) that differs
    /// from what it was sent, and a route for every channel whose selected
    /// flow differs, all at `at` rounded up to a block. When the engine no
    /// longer takes that index, the rest of the diff goes at the earliest
    /// one it does; while it holds as many changes as it takes, the next
    /// waits. With `settle`, the gains wait for the strips' ramps.
    async fn apply(&self, state: &Value, sent: &mut Sent, at: u64, settle: bool) {
        let mut at = at.next_multiple_of(self.block);
        let targets = mix::targets(&self.shape, &self.wiring, state);
        for (slot, (sent, next)) in sent.strips.iter_mut().zip(targets.strips).enumerate() {
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
        let next = Targets {
            ducks: targets.ducks,
            masks: targets.masks,
            ..Targets::default()
        };
        let entries = match &sent.ducks {
            Some(held) => held.diff(&next),
            None => next.assert(&[], None),
        };
        let mut held = sent.ducks.take().unwrap_or_default();
        let mut refused = false;
        for entry in entries {
            let took = match &entry {
                Entry::Duck { group, setting } => {
                    let set = |at| self.engine.set_duck(*group, setting.clone(), at);
                    self.send(&mut at, set).await
                }
                &Entry::DuckMask {
                    input,
                    output,
                    mask,
                } => {
                    let set = |at| self.engine.set_duck_mask(input, output, mask, at);
                    self.send(&mut at, set).await
                }
                _ => continue,
            };
            if took {
                hold_entry(&mut held, entry);
            } else {
                refused = true;
            }
        }
        // After a refusal every setting and mask goes again on the next
        // write.
        sent.ducks = (!refused).then_some(held);
        if settle {
            at = (at + self.ramp).next_multiple_of(self.block);
        }
        *self.taps.lock().expect("taps lock poisoned") = mix::meter_taps(&self.shape, state);
        for (input, (row, new)) in sent.matrix.iter_mut().zip(targets.gains).enumerate() {
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
        for (slot, flow) in targets.flows.into_iter().enumerate() {
            // Every channel reads a flow: no target detaches one.
            let Some(flow) = flow else { continue };
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
            self.inputs
                .lock()
                .expect("input health lock poisoned")
                .update(&status.inputs);
            let late = dropped(counters);
            // A set's engine is resynced or rebuilt by the replica.
            let act = if self.replicated {
                Act::None
            } else {
                drops.tick(late, &status.diverged, tick.into_std())
            };
            match act {
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

    /// The model `Counters` of the `counters` message ([`counters`]) and
    /// the engine of `GET /status` ([`engine_status`]), from one reading of
    /// the engine's status.
    pub fn readings(&self) -> (Value, Value) {
        let status = self.engine.status();
        (self.counters(&status), engine_status(&status))
    }

    /// The model `Counters` of the `counters` message: the engine's counts
    /// of faults an operator acts on, the inputs' too-late blocks and the
    /// resends, totals since the engine started. None grows while the
    /// engine keeps up, so the UI's lamp lights on any growth; counts that
    /// grow every block, such as the blocks processed, are left out.
    fn counters(&self, status: &samplez::Status) -> Value {
        let mut values: Map<String, Value> = engine_counters(self.engine.counters())
            .into_iter()
            .map(|(name, c)| (format!("{name}_total"), json!(c.load(Relaxed))))
            .collect();
        let too_late: u64 = (status.inputs.iter()).map(|i| i.counters.too_late).sum();
        values.insert("input_too_late_blocks_total".into(), json!(too_late));
        values.insert("resends_total".into(), json!(self.resent.load(Relaxed)));
        Value::Object(values)
    }
}

/// The engine of `GET /status`: per channel its input's state in the
/// engine, whether the log has it up, the flow it reads and its blocks
/// copied, late and too late; per output whether it is enabled and
/// realised, and why not.
fn engine_status(status: &samplez::Status) -> Value {
    let inputs: Vec<Value> = (status.inputs.iter())
        .map(|i| {
            let state = match i.state {
                InputState::Attached => "attached",
                InputState::Pruned => "pruned",
                InputState::Detached => "detached",
            };
            json!({
                "channel": i.name,
                "state": state,
                "up": i.up,
                "flowId": i.flow_id,
                "copiedBlocks": i.counters.copied,
                "lateBlocks": i.counters.late,
                "tooLateBlocks": i.counters.too_late,
            })
        })
        .collect();
    let outputs: Vec<Value> = (status.outputs.iter())
        .map(|o| {
            json!({
                "output": o.name,
                "enabled": o.enabled,
                "realised": o.realised,
                "reason": o.reason,
            })
        })
        .collect();
    json!({ "inputs": inputs, "outputs": outputs })
}

/// Notes in `held` the duck setting or mask `entry` sent.
fn hold_entry(held: &mut Targets, entry: Entry) {
    match entry {
        Entry::Duck { group, setting } => {
            if held.ducks.len() <= group {
                held.ducks.resize(group + 1, DuckTargets::default());
            }
            let d = &mut held.ducks[group];
            match setting {
                DuckSetting::Detector(p) => d.params = *p,
                DuckSetting::Keys(k) => d.keys = k,
                DuckSetting::Mode(m) => d.mode = m,
                DuckSetting::Hold { held } => d.held = held,
                DuckSetting::Depth { slot, db } => {
                    if d.depths.len() <= slot {
                        d.depths.resize(slot + 1, 0.0);
                    }
                    d.depths[slot] = db;
                }
            }
        }
        Entry::DuckMask {
            input,
            output,
            mask,
        } => {
            if held.masks.len() <= input {
                held.masks.resize(input + 1, Vec::new());
            }
            let row = &mut held.masks[input];
            if row.len() <= output {
                row.resize(output + 1, 0);
            }
            row[output] = mask;
        }
        _ => {}
    }
}

/// The engine counters [`Audio::counters`] carries, by field name.
fn engine_counters(c: &Counters) -> [(&'static str, &AtomicU64); 19] {
    [
        ("deadline_miss", &c.deadline_miss),
        ("overrun_skipped_samples", &c.overrun_skipped_samples),
        ("clock_steps", &c.clock_steps),
        ("clock_behind_wakes", &c.clock_behind_wakes),
        ("input_wait_timeout", &c.input_wait_timeout),
        ("input_late_blocks", &c.input_late_blocks),
        ("input_silenced_blocks", &c.input_silenced_blocks),
        ("output_held_blocks", &c.output_held_blocks),
        ("output_failed_writes", &c.output_failed_writes),
        ("command_late", &c.command_late),
        ("generation_late", &c.generation_late),
        ("strip_refused", &c.strip_refused),
        ("late_routes_lost", &c.late_routes_lost),
        ("blocks_withheld", &c.blocks_withheld),
        ("hold_overflows", &c.hold_overflows),
        ("blocks_without_core", &c.blocks_without_core),
        ("meter_dropped", &c.meter_dropped),
        ("telemetry_dropped", &c.telemetry_dropped),
        ("status_changes_dropped", &c.status_changes_dropped),
    ]
}

/// Each channel's input as the engine reads it, on `/metrics`.
struct InputHealth {
    attached: Vec<Gauge>,
    margin: Vec<Gauge>,
    /// Copied, late and too-late block counts, and the counts last added.
    blocks: Vec<[(Counter, u64); 3]>,
}

impl InputHealth {
    fn new(metrics: &Metrics, shape: &Shape) -> InputHealth {
        let mut h = InputHealth {
            attached: Vec::new(),
            margin: Vec::new(),
            blocks: Vec::new(),
        };
        for c in &shape.channels {
            let l = [("channel", c.id.as_str())];
            h.attached.push(metrics.gauge(
                "faderz_input_attached",
                "1 while the engine reads the channel's input flow, 0 while it is \
                 pruned and polled until it is readable again.",
                &l,
            ));
            h.margin.push(metrics.gauge(
                "faderz_input_margin_needed_samples",
                "The readerMarginSamples the channel's input needs when the \
                 configured one is below it, from its late blocks or its \
                 producer's batch hint; 0 when it suffices.",
                &l,
            ));
            let count = |name, help| (metrics.counter(name, help, &l), 0);
            h.blocks.push([
                count(
                    "faderz_input_copied_blocks_total",
                    "Blocks of the channel's input read on time.",
                ),
                count(
                    "faderz_input_late_blocks_total",
                    "Blocks of the channel's input not read because its head was \
                     below them at their deadline: silenced.",
                ),
                count(
                    "faderz_input_too_late_blocks_total",
                    "Blocks of the channel's input not read because the flow no \
                     longer held them: silenced.",
                ),
            ]);
        }
        h
    }

    /// Follows the engine's input statuses, one per channel in shape
    /// order.
    fn update(&mut self, inputs: &[InputStatus]) {
        for (i, input) in inputs.iter().enumerate().take(self.attached.len()) {
            self.attached[i].set(f64::from(u8::from(input.state == InputState::Attached)));
            self.margin[i].set(margin_needed(input) as f64);
            let c = &input.counters;
            for ((counter, last), now) in self.blocks[i]
                .iter_mut()
                .zip([c.copied, c.late, c.too_late])
            {
                counter.add(now.saturating_sub(*last));
                *last = now;
            }
        }
    }
}

/// The `readerMarginSamples` an input needs when the configured one is
/// below it, the larger of what its late blocks and its producer's batch
/// hint ask for; 0 when it suffices.
fn margin_needed(input: &InputStatus) -> u64 {
    input.margin_warning.max(input.margin_short).unwrap_or(0)
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

/// Appends an `InputDown` for each input the replicated engine pruned and
/// an `InputUp` for each one readable again, which the leader stamps; so
/// every replica of the set silences and raises the input at the same
/// index. Only a leader's append is ordered: a report made while
/// following, or one lost to an election, is appended again while this
/// replica leads and the applied log does not hold it, first after
/// `write_timeout`, within which an append is ordered or given up, then
/// at twice the interval up to [`RETRY_CAP`] times it ([`Unapplied`]). Runs on a
/// thread of its own until the engine stops or the replica is gone.
pub fn report_inputs(
    reports: Receiver<Report>,
    replica: Weak<Replica<Mixer>>,
    write_timeout: Duration,
) {
    let forward = move || {
        let mut unapplied = Unapplied::new(write_timeout);
        let mut next: Option<std::time::Instant> = None;
        loop {
            let received = match next {
                Some(at) => {
                    reports.recv_timeout(at.saturating_duration_since(std::time::Instant::now()))
                }
                None => reports.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            let now = std::time::Instant::now();
            match received {
                Ok(report) => unapplied.report(report, now),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let Some(replica) = replica.upgrade() else {
                return;
            };
            let leads = replica.raft().metrics().borrow().state == ServerState::Leader;
            next = unapplied.retry(
                now,
                leads,
                |slot| replica.store().with_state(|sm| applied_up(sm, slot)),
                |body| {
                    if let Err(e) = replica.append(body) {
                        tracing::warn!("input report not appended: {e}");
                    }
                },
            );
        }
    };
    if let Err(e) = std::thread::Builder::new()
        .name("faderz-inputs".into())
        .spawn(forward)
    {
        tracing::error!("no thread reports the inputs' health to the set: {e}");
    }
}

/// How many times `write_timeout` an input report is retried at most.
pub const RETRY_CAP: u32 = 4;

/// The input health this replica's engine reported that the set may not
/// hold yet, per slot; a later report on a slot replaces an earlier one.
struct Unapplied {
    slots: BTreeMap<u32, Retry>,
    first: Duration,
}

struct Retry {
    up: bool,
    /// Reported while leading and not appended yet: appended whatever the
    /// applied log holds, as every report was before it was retried.
    fresh: bool,
    due: std::time::Instant,
    backoff: Duration,
}

impl Unapplied {
    fn new(first: Duration) -> Unapplied {
        Unapplied {
            slots: BTreeMap::new(),
            first,
        }
    }

    fn report(&mut self, report: Report, now: std::time::Instant) {
        let (slot, up) = match report {
            Report::InputPruned { slot } => (slot as u32, false),
            Report::InputReadable { slot } => (slot as u32, true),
        };
        let retry = Retry {
            up,
            fresh: true,
            due: now,
            backoff: self.first,
        };
        self.slots.insert(slot, retry);
    }

    /// Appends each report due, while `leads`, unless it was appended
    /// before and the applied log holds it (`applied` says whether it
    /// holds a slot up), which drops it. Returns when the next one is due.
    fn retry(
        &mut self,
        now: std::time::Instant,
        leads: bool,
        applied: impl Fn(u32) -> bool,
        mut append: impl FnMut(Body),
    ) -> Option<std::time::Instant> {
        let (first, cap) = (self.first, self.first * RETRY_CAP);
        self.slots.retain(|&slot, r| {
            if r.due > now {
                return true;
            }
            if !leads {
                // A follower's append is not ordered: the applied log
                // decides once it leads.
                r.fresh = false;
                r.due = now + first;
                return true;
            }
            if !r.fresh && applied(slot) == r.up {
                return false;
            }
            append(input_entry(slot, r.up));
            r.fresh = false;
            r.due = now + r.backoff;
            r.backoff = (r.backoff * 2).min(cap);
            true
        });
        self.slots.values().map(|r| r.due).min()
    }
}

/// Whether the applied log holds `slot` up: as its latest `InputDown` or
/// `InputUp` says, up where none is held
/// ([`StateMachine::product_state`]).
fn applied_up(sm: &StateMachine<Mixer>, slot: u32) -> bool {
    (sm.pending.values().rev())
        .find_map(|body| match body {
            Body::InputDown { slot: s, .. } if *s == slot => Some(false),
            Body::InputUp { slot: s, .. } if *s == slot => Some(true),
            _ => None,
        })
        .unwrap_or(true)
}

/// The log entry of slot `slot` up or down, at an index the leader
/// stamps.
fn input_entry(slot: u32, up: bool) -> Body {
    if up {
        Body::InputUp { slot, at_index: 0 }
    } else {
        Body::InputDown { slot, at_index: 0 }
    }
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

    /// The margin an input needs is the larger of the two the engine
    /// reports, either of which may be absent.
    #[test]
    fn the_margin_needed_is_the_larger_report() {
        let status = |warning, short| InputStatus {
            name: "in".into(),
            state: InputState::Attached,
            up: true,
            flow_id: String::new(),
            error: None,
            margin_short: short,
            margin_warning: warning,
            counters: Default::default(),
            meters: Default::default(),
        };
        assert_eq!(margin_needed(&status(None, None)), 0);
        assert_eq!(margin_needed(&status(Some(1303), None)), 1303);
        assert_eq!(margin_needed(&status(None, Some(1008))), 1008);
        assert_eq!(margin_needed(&status(Some(700), Some(1008))), 1008);
        assert_eq!(margin_needed(&status(Some(1399), Some(1008))), 1399);
    }

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

    /// A set's contact timers take the engine's block, margin, hold and
    /// contact window, and the cap `replica.maxApplyLeadMs` sets.
    #[test]
    fn a_set_times_contact_by_its_engine_and_its_cap() {
        let id = |n: u8| format!("00000000-0000-4000-8000-00000000000{n}");
        let config = json!({
            "replica": {
                "replicas": 3, "maxApplyLeadMs": 80,
                "peers": ["f-0.f:7443", "f-1.f:7443", "f-2.f:7443"],
                "peer": { "bind": "0.0.0.0", "port": 7443, "tls": {
                    "mode": "files", "certFile": "/t/tls.crt", "keyFile": "/t/tls.key",
                    "clientCaFile": "/t/ca.crt" } },
                "marker": "m",
            },
            "shape": crate::shape::tests::small(),
            "mxl": {
                "domains": [{ "name": "d", "path": "/dev/shm/d" }],
                "inputs": { "ch1": { "flow": id(1), "altFlow": id(5) }, "ch2": { "flow": id(2) },
                            "st1": { "flow": id(3) } },
                "outputs": { "out-pgm": { "flows": [id(4), id(6), id(7)] } },
            },
            "engine": { "blockSize": 96, "readerMarginSamples": 960 },
        });
        let (config, _, _) =
            crate::config::Config::parse(&config.to_string(), || "faderz-1".into()).unwrap();
        let engine = config.audio.as_ref().unwrap();
        let t = timing(engine, config.set.as_ref().unwrap());
        assert_eq!((t.block, t.margin), (96, 960));
        assert_eq!(t.max_apply_lead, Duration::from_millis(80));
        assert_eq!(t.hold, Duration::from_millis(u64::from(engine.hold_ms)));
        assert_eq!(
            t.contact_window,
            Duration::from_millis(u64::from(engine.contact_window_ms))
        );
    }

    fn down(slot: u32) -> Body {
        Body::InputDown { slot, at_index: 0 }
    }

    fn up(slot: u32) -> Body {
        Body::InputUp { slot, at_index: 0 }
    }

    /// A leader appends a pruned input's `InputDown` at once. An election
    /// takes the entry with it; the replica follows for a while, appends
    /// nothing then, leads again and appends it again, first after the
    /// write timeout, then at twice the interval, until the applied log
    /// holds the input down.
    #[test]
    fn an_input_report_lost_in_an_election_is_appended_again() {
        let first = Duration::from_secs(2);
        let t0 = std::time::Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut u = Unapplied::new(first);
        let held_up = std::cell::Cell::new(true);
        let mut appended = Vec::new();
        u.report(Report::InputPruned { slot: 3 }, t0);
        let next = u.retry(t0, true, |_| held_up.get(), |b| appended.push(b));
        assert_eq!(appended, [down(3)]);
        assert_eq!(next, Some(at(2_000)));
        // Not due yet.
        u.retry(at(1_000), true, |_| held_up.get(), |b| appended.push(b));
        assert_eq!(appended.len(), 1);
        // The election: following, it appends nothing.
        let next = u.retry(at(2_000), false, |_| held_up.get(), |b| appended.push(b));
        assert_eq!((appended.len(), next), (1, Some(at(4_000))));
        // Leading again, the set still holds the input up.
        let next = u.retry(at(4_000), true, |_| held_up.get(), |b| appended.push(b));
        assert_eq!(appended, [down(3), down(3)]);
        assert_eq!(next, Some(at(8_000)));
        let next = u.retry(at(8_000), true, |_| held_up.get(), |b| appended.push(b));
        assert_eq!((appended.len(), next), (3, Some(at(16_000))));
        // Bounded: never more than RETRY_CAP write timeouts apart.
        let next = u.retry(at(16_000), true, |_| held_up.get(), |b| appended.push(b));
        assert_eq!((appended.len(), next), (4, Some(at(24_000))));
        // Applied: dropped.
        held_up.set(false);
        let next = u.retry(at(24_000), true, |_| held_up.get(), |b| appended.push(b));
        assert_eq!((appended.len(), next), (4, None));
    }

    /// Reports on a slot not yet held replace each other: the latest is
    /// appended. A follower's report is appended once it leads only while
    /// the applied log does not hold it already.
    #[test]
    fn the_latest_input_report_wins() {
        let first = Duration::from_secs(2);
        let t0 = std::time::Instant::now();
        let mut u = Unapplied::new(first);
        let mut appended = Vec::new();
        u.report(Report::InputPruned { slot: 1 }, t0);
        u.report(Report::InputReadable { slot: 1 }, t0);
        u.retry(t0, true, |_| true, |b| appended.push(b));
        assert_eq!(appended, [up(1)]);

        let mut u = Unapplied::new(first);
        let mut appended = Vec::new();
        u.report(Report::InputPruned { slot: 0 }, t0);
        u.report(Report::InputPruned { slot: 2 }, t0);
        u.retry(t0, false, |_| true, |b| appended.push(b));
        assert!(appended.is_empty());
        // Leading: slot 0 the set already holds down, slot 2 it does not.
        let next = u.retry(t0 + first, true, |slot| slot != 0, |b| appended.push(b));
        assert_eq!(appended, [down(2)]);
        assert_eq!(next, Some(t0 + first * 2));
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
