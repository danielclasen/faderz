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

use samplez::rt::ProcessStatus;
use samplez::{
    Counters, DuckSetting, DuckTargets, Engine, Entry, EntryError, Fade, GainError, InputState,
    InputStatus, Replication, Report, Targets,
};
use samplez_dsp::{StripParam, ToneParams, WINDOW};
use samplez_replica::Replica;
use samplez_replica::bridge::EngineAudio;
use samplez_replica::contact::Timing;
use samplez_replica::entry::Body;
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
/// engine's block, margin, hold, contact window and `pruneAfterBlocks`,
/// which input liveness holds a decided change for, and the set's
/// `maxApplyLeadMs`.
pub fn timing(engine: &samplez::Config, set: &Set) -> Timing {
    Timing {
        margin: u64::from(engine.reader_margin_samples),
        hold: Duration::from_millis(u64::from(engine.hold_ms)),
        contact_window: Duration::from_millis(u64::from(engine.contact_window_ms)),
        max_apply_lead: set.max_apply_lead,
        prune_after_blocks: engine.prune_after_blocks,
        ..Timing::defaults(u64::from(engine.block_size))
    }
}

/// The flow each channel reads at start, as the state sets it: none for a
/// channel IS-05 detached or a tone channel, so the engine opens no
/// reader for it.
fn start_flows(shape: &Shape, view: &mix::EngineView, state: &Value) -> Vec<Option<String>> {
    (mix::selected(shape, &view.wiring, state).into_iter())
        .zip(&view.detached)
        .map(|(flow, detached)| flow.filter(|_| !detached))
        .collect()
}

pub struct Audio {
    engine: Arc<Engine>,
    /// The engine follows a set's replicated log: nothing is sent to it
    /// from here.
    replicated: bool,
    shape: Shape,
    /// The flow each channel's input read when the engine started.
    started_on: Vec<Option<String>>,
    /// Each output's enable when the engine started.
    started_enabled: Vec<bool>,
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
    /// Per loudness meter ([`Layout::loudness_meter`]), the resets the
    /// state counts and where the last one this engine applied starts the
    /// measurement.
    resets: Mutex<BTreeMap<usize, Resets>>,
}

/// What the engine was sent: every coefficient, each input's flow, none
/// for a detached one, and each output's enable.
struct Sent {
    matrix: Vec<Vec<f32>>,
    routed: Vec<Option<String>>,
    outputs: Vec<bool>,
    /// Each input's strip parameters ([`strip::params`]); `None` for one
    /// not sent, as all are at start: the engine's strip passes its input
    /// unchanged until set.
    strips: Vec<Vec<Option<StripParam>>>,
    /// The limiters, input delays, tone generators, duck groups and masks
    /// the engine holds, as targets; `None` after a resend asked for, when
    /// every limiter, delay, tone, setting and mask goes again.
    ducks: Option<Targets>,
    /// The loudness run flags and reset epochs the engine was sent, as
    /// targets ([`mix::loudness`]).
    loudness: Targets,
    /// Whether the engine holds the run flags of `loudness`: not after a
    /// resend asked for, when every one goes again.
    loudness_held: bool,
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
        self.loudness_held = false;
        let status = engine.status();
        for (routed, input) in self.routed.iter_mut().zip(status.inputs) {
            let reads = !matches!(input.state, InputState::Detached | InputState::Tone);
            *routed = reads.then_some(input.flow_id);
        }
        for (enabled, output) in self.outputs.iter_mut().zip(status.outputs) {
            *enabled = output.enabled;
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
        view: mix::EngineView,
        state: &Value,
        ops: Ops,
    ) -> Result<Audio, String> {
        let started_on = start_flows(shape, &view, state);
        for (input, flow) in config.inputs.iter_mut().zip(&started_on) {
            input.flow_id.clone_from(flow);
        }
        for (output, enabled) in config.outputs.iter_mut().zip(&view.outputs) {
            output.enabled = *enabled;
        }
        let block = u64::from(config.block_size);
        let ramp = u64::from(config.ramp_samples);
        let engine = Engine::start(config, process, None).map_err(|e| format!("engine: {e}"))?;
        let mut audio = Audio::new(Arc::new(engine), shape, state, ops);
        (audio.started_on, audio.block, audio.ramp) = (started_on, block, ramp);
        audio.started_enabled = view.outputs;
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
        view: mix::EngineView,
        state: &Value,
        ops: Ops,
        set: &Set,
    ) -> Result<SetEngine, String> {
        let timing = timing(&config, set);
        for (input, flow) in config
            .inputs
            .iter_mut()
            .zip(start_flows(shape, &view, state))
        {
            input.flow_id = flow;
        }
        for (output, enabled) in config.outputs.iter_mut().zip(&view.outputs) {
            output.enabled = *enabled;
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
        let mut audio = Audio::new(engine, shape, state, ops);
        audio.replicated = true;
        Ok(SetEngine {
            audio,
            bridge,
            reports: reports_rx,
            timing,
        })
    }

    fn new(engine: Arc<Engine>, shape: &Shape, state: &Value, ops: Ops) -> Audio {
        let metrics = ops.metrics();
        Audio {
            engine,
            replicated: false,
            started_on: Vec::new(),
            started_enabled: Vec::new(),
            block: 0,
            ramp: 0,
            shape: shape.clone(),
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
            resets: Mutex::default(),
            ops,
        }
    }

    /// The engine, for a thread that must not keep it running.
    pub fn engine_weak(&self) -> Weak<Engine> {
        Arc::downgrade(&self.engine)
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
        // Every measurement runs from the engine's start, and the resets
        // the state counted before it are history.
        let mut loudness = Targets::default();
        mix::loudness(&self.shape, &state, &mut loudness);
        loudness.loudness_running.clear();
        let mut sent = Sent {
            matrix: mix::matrix(&self.shape, &state)
                .into_iter()
                .map(|row| vec![0.0; row.len()])
                .collect(),
            routed: self.started_on.clone(),
            outputs: self.started_enabled.clone(),
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
            loudness,
            loudness_held: true,
        };
        let (view, fades) = crate::app::with_state(replica, |sm| {
            (sm.product.engine_view(), sm.product.gain_fades())
        });
        self.apply(&state, &view, &fades, &mut sent, 0, true).await;
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
                            (state, revision) = owned_state_of(replica);
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
                None => (state, revision) = owned_state_of(replica),
            }
            let (view, fades) = crate::app::with_state(replica, |sm| {
                (sm.product.engine_view(), sm.product.gain_fades())
            });
            self.apply(&state, &view, &fades, &mut sent, at, false)
                .await;
            if let Some(n) = resent {
                let asked = self.resends_asked.lock().expect("resend lock poisoned");
                if *asked == n {
                    self.resend_pending.set(0.0);
                    self.ops.set_engine_started(true);
                }
            }
        }
    }

    /// Keeps the meter taps and the loudness resets on a set's replicated
    /// state. A set's engine follows the log through samplez-replica and
    /// [`Audio::follow`] does not run, so nothing else updates them:
    /// without this every post-fader meter stays at the fader, mute and
    /// VCA gains the replica started with. Notes where each reset the
    /// state machine derived starts its measurement: the last record that
    /// resets a meter, a restamp's after the entry it moved. Wakes on each
    /// applied revision, and every second besides, so a revision a rebuild
    /// moved backwards cannot stall it. Never returns.
    pub async fn follow_taps(&self, replica: &Replica<Mixer>, mut revision: u64) {
        loop {
            tokio::select! {
                () = replica.applied(revision + 1) => {}
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
            let (taps, at, counted, placed) = crate::app::with_state(replica, |sm| {
                let placed: Vec<(usize, u64)> = (sm.records.values())
                    .flat_map(|r| {
                        (r.entries.iter()).filter_map(|e| match *e {
                            Entry::ResetLoudness { meter } => Some((meter, r.at_index)),
                            _ => None,
                        })
                    })
                    .collect();
                (
                    mix::meter_taps(&self.shape, sm.product.state()),
                    sm.revision,
                    resets_of(sm.product.state()),
                    placed,
                )
            });
            revision = at;
            *self.taps.lock().expect("taps lock poisoned") = taps;
            self.counted(&counted);
            for (meter, at) in placed {
                self.placed(meter, at);
            }
        }
    }

    /// Notes the resets the state counts, `counted` by bus.
    fn counted(&self, counted: &BTreeMap<String, u64>) {
        let layout = Layout::of(&self.shape);
        let mut resets = self.resets.lock().expect("resets lock poisoned");
        for (id, &n) in counted {
            if let Some(meter) = layout.loudness_meter(id) {
                resets.entry(meter).or_default().counted = n;
            }
        }
    }

    /// Notes a reset of `meter` the engine takes at `at`: it starts the
    /// measurement at the first 100 ms window at or after it.
    fn placed(&self, meter: usize, at: u64) {
        let mut resets = self.resets.lock().expect("resets lock poisoned");
        let start = &mut resets.entry(meter).or_default().at;
        *start = (*start).max(Some(at.next_multiple_of(WINDOW)));
    }

    /// Sends the engine every strip parameter, tone, duck setting, duck mask and
    /// coefficient of `state`'s targets read through `view`
    /// ([`mix::EngineView::targets`]) that differs from what it was sent, a
    /// route or detach for every channel whose flow differs and an enable
    /// for every output whose enable differs, all at `at` rounded up to a
    /// block; the gains no earlier than the detaches. When the engine no
    /// longer takes that index, the rest of the diff goes at the earliest
    /// one it does; while it holds as many changes as it takes, the next
    /// waits. With `settle`, the gains wait for the strips' ramps. A gain
    /// goes with its fade in `fades` ([`crate::state::Mixer::gain_fades`])
    /// when the fade is towards that gain, and the engine takes it at the
    /// index the gain goes at; the state the deltas built may lag the
    /// fades read, and the gain then goes without, over the ramp length.
    async fn apply(
        &self,
        state: &Value,
        view: &mix::EngineView,
        fades: &[Vec<Option<(f32, Fade)>>],
        sent: &mut Sent,
        at: u64,
        settle: bool,
    ) {
        let mut at = at.next_multiple_of(self.block);
        let targets = view.targets(&self.shape, state);
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
            limiters: targets.limiters,
            delays: targets.delays,
            tones: targets.tones,
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
                &Entry::Limiter { output, params } => {
                    let set = |at| self.engine.set_limiter(output, params, at);
                    self.send(&mut at, set).await
                }
                &Entry::Delay { slot, samples } => {
                    let set = |at| self.engine.set_delay(slot, samples, at);
                    self.send(&mut at, set).await
                }
                &Entry::Tone {
                    slot,
                    frequency_hz,
                    level_db,
                    ident,
                } => {
                    let params = ToneParams {
                        frequency_hz,
                        level_db,
                        ident,
                    };
                    let set = |at| self.engine.set_tone(slot, params, at);
                    self.send(&mut at, set).await
                }
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
        // After a refusal every limiter, delay, tone, setting and mask goes
        // again on the next write.
        sent.ducks = (!refused).then_some(held);
        // The line-up tone's slot, past the channels', reads no flow.
        let flows = targets.flows.into_iter().zip(&mut sent.routed);
        for (slot, (flow, routed)) in flows.enumerate() {
            while flow != *routed {
                let sent_now = match &flow {
                    Some(f) => self.engine.route(slot, f, at),
                    None => self.engine.detach(slot, at),
                };
                match sent_now {
                    Ok(k) => {
                        // A detached channel is silent before any gain
                        // sent with it applies, so one detached at start
                        // is never heard.
                        if flow.is_none() {
                            at = at.max(k.next_multiple_of(self.block));
                        }
                        routed.clone_from(&flow);
                    }
                    Err(EntryError::Full) => tokio::time::sleep(FULL_RETRY).await,
                    Err(e) => {
                        tracing::warn!(slot, ?flow, "input route refused: {e}");
                        break;
                    }
                }
            }
        }
        for (slot, enabled) in targets.outputs.into_iter().enumerate() {
            while enabled != sent.outputs[slot] {
                match self.engine.set_output_enabled(slot, enabled, at) {
                    Ok(_) => sent.outputs[slot] = enabled,
                    Err(EntryError::Full) => tokio::time::sleep(FULL_RETRY).await,
                    Err(e) => {
                        tracing::warn!(slot, enabled, "output enable refused: {e}");
                        break;
                    }
                }
            }
        }
        // A pause, a run again and a reset counted, as the engine's own
        // log entries; after a resend asked for, every run flag again.
        let next = Targets {
            loudness_running: targets.loudness_running,
            loudness_resets: targets.loudness_resets,
            ..Targets::default()
        };
        let mut entries = sent.loudness.diff(&next);
        if !sent.loudness_held {
            entries.retain(|e| matches!(e, Entry::ResetLoudness { .. }));
            entries.extend(next.assert(&[], None));
        }
        for entry in entries {
            match entry {
                Entry::LoudnessRun { meter, running } => {
                    let set = |at| self.engine.set_loudness_running(meter, running, at);
                    self.send(&mut at, set).await;
                }
                Entry::ResetLoudness { meter } => {
                    let reset = |at| self.engine.reset_loudness(meter, at);
                    if self.send(&mut at, reset).await {
                        self.placed(meter, at);
                    }
                }
                _ => {}
            }
        }
        (sent.loudness, sent.loudness_held) = (next, true);
        self.counted(&resets_of(state));
        if settle {
            at = (at + self.ramp).next_multiple_of(self.block);
        }
        *self.taps.lock().expect("taps lock poisoned") = mix::meter_taps(&self.shape, state);
        for (input, (row, new)) in sent.matrix.iter_mut().zip(targets.gains).enumerate() {
            for (output, (g, n)) in row.iter_mut().zip(new).enumerate() {
                let fade = fades.get(input).and_then(|r| r.get(output)).copied();
                let fade = (fade.flatten())
                    .filter(|(to, _)| to.to_bits() == n.to_bits())
                    .map(|(_, f)| f);
                let set = |at| {
                    let fade = fade.filter(|f| f.within(at));
                    self.engine.set_gain_fade(input, output, n, fade, at)
                };
                if *g != n && self.send(&mut at, set).await {
                    *g = n;
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

    /// The `values` and `inputs` of the `counters` message ([`counters`])
    /// and the engine of `GET /status` ([`engine_status`]), from one
    /// reading of the engine's status.
    pub fn readings(&self) -> (Value, Value) {
        let status = self.engine.status();
        let inputs: Vec<Value> = channel_inputs(&status)
            .map(|i| {
                json!({
                    "channel": i.name,
                    "state": input_state(i.state),
                    "up": i.up,
                    "lateBlocks": i.counters.late,
                    "tooLateBlocks": i.counters.too_late,
                })
            })
            .collect();
        let counters = json!({
            "values": self.counters(&status),
            "inputs": inputs,
            "loudness": self.loudness(&status),
        });
        (counters, engine_status(&status))
    }

    /// Model `LoudnessHealth` of each bus, aux and mix-minus output in
    /// shape order: the gaps and lost slices of its measurement, when it
    /// started in UTC, and whether it restarted ([`restarted`]).
    fn loudness(&self, status: &samplez::Status) -> Vec<Value> {
        let layout = Layout::of(&self.shape);
        let now = crate::replica::tai_samples();
        let resets = self.resets.lock().expect("resets lock poisoned");
        let ids = (self.shape.buses.iter().map(|b| &b.id))
            .chain(self.shape.auxes.iter().map(|a| &a.id))
            .chain(self.shape.mix_minus.iter().map(|m| &m.id));
        ids.filter_map(|id| {
            let meter = layout.loudness_meter(id)?;
            let meters = match meter.checked_sub(status.outputs.len()) {
                None => &status.outputs.get(meter)?.meters,
                Some(m) => &status.meters.get(m)?.meters,
            };
            Some(json!({
                "id": id,
                "gaps": meters.gaps,
                "lost": meters.lost,
                "since": meters.since.map(|i| utc_of(i, now)),
                "restarted": restarted(
                    meters.since,
                    resets.get(&meter).copied().unwrap_or_default(),
                    now,
                ),
            }))
        })
        .collect()
    }

    /// The model `Counters` of the `counters` message: the engine's counts
    /// of faults an operator acts on, the inputs' too-late blocks and the
    /// resends, totals since the engine started; counts that grow every
    /// block, such as the blocks processed, are left out.
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

/// An input's state as the contract names it.
fn input_state(state: InputState) -> &'static str {
    match state {
        InputState::Attached => "attached",
        InputState::Pruned => "pruned",
        InputState::Detached => "detached",
        InputState::Tone => "tone",
    }
}

/// The engine's inputs that are channels': all but the line-up tone's
/// ([`mix::tone_input`]).
fn channel_inputs(status: &samplez::Status) -> impl Iterator<Item = &InputStatus> {
    (status.inputs.iter()).filter(|i| !i.name.starts_with(mix::TONE_INPUT))
}

/// The engine of `GET /status`: per channel its input's state in the
/// engine, whether the log has it up, the flow it reads and its blocks
/// copied, late and too late; per output whether it is enabled and
/// realised, and why not.
fn engine_status(status: &samplez::Status) -> Value {
    let inputs: Vec<Value> = channel_inputs(status)
        .map(|i| {
            json!({
                "channel": i.name,
                "state": input_state(i.state),
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

/// Notes in `held` the limiter, delay, tone, duck setting or mask `entry`
/// sent.
fn hold_entry(held: &mut Targets, entry: Entry) {
    match entry {
        Entry::Tone {
            slot,
            frequency_hz,
            level_db,
            ident,
        } => {
            if held.tones.len() <= slot {
                held.tones.resize(slot + 1, None);
            }
            held.tones[slot] = Some(ToneParams {
                frequency_hz,
                level_db,
                ident,
            });
        }
        Entry::Delay { slot, samples } => {
            if held.delays.len() <= slot {
                held.delays.resize(slot + 1, 0);
            }
            held.delays[slot] = samples;
        }
        Entry::Limiter { output, params } => {
            if held.limiters.len() <= output {
                held.limiters.resize(output + 1, None);
            }
            held.limiters[output] = Some(params);
        }
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

/// A bus's resets as this replica knows them.
#[derive(Debug, Default, Clone, Copy)]
struct Resets {
    /// The resets the state counts, `/loudness/<id>/resets`.
    counted: u64,
    /// Where the last reset this engine applied starts the measurement:
    /// the first 100 ms window at or after the index the engine took it
    /// at; none before the first.
    at: Option<u64>,
}

/// How long after the index a reset starts at a measurement that has not
/// moved to it yet counts as pending, not restarted: the counters read
/// the engine's status once a second.
const RESET_PENDING: u64 = 2 * crate::replica::RATE;

/// Whether a measurement that started at `since` restarted on its own:
/// the state counts a reset of it, and it did not start where the last
/// reset this replica applied starts it, or this replica applied none,
/// as one does that started after that reset. Within [`RESET_PENDING`]
/// of that start at `now` the reset is pending, not missed. Without a
/// reset in the state, every replica measures from its own start and
/// none is flagged; a measurement not started yet has not restarted.
fn restarted(since: Option<u64>, resets: Resets, now: u64) -> bool {
    match (since, resets.at) {
        (None, _) => false,
        _ if resets.counted == 0 => false,
        (Some(_), None) => true,
        (Some(since), Some(at)) => since != at && now >= at.saturating_add(RESET_PENDING),
    }
}

/// Sample index `index` of libmxl's clock, CLOCK_TAI in samples, in RFC
/// 3339 UTC: placed back from the system clock by how far it lies behind
/// `now`, the same clock's index now, so the TAI-UTC offset falls out.
fn utc_of(index: u64, now: u64) -> String {
    let behind = Duration::from_nanos(
        (now.saturating_sub(index) as u128 * 1_000_000_000 / u128::from(crate::replica::RATE))
            as u64,
    );
    let at = std::time::SystemTime::now()
        .checked_sub(behind)
        .unwrap_or(std::time::UNIX_EPOCH);
    crate::app::rfc3339(at)
}

/// The engine counters [`Audio::counters`] carries, by field name.
fn engine_counters(c: &Counters) -> [(&'static str, &AtomicU64); 20] {
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
        ("generation_past_margin", &c.generation_past_margin),
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

/// Whether a channel's input is live: an attached flow, or a tone
/// generator, which reads no flow and is never pruned.
fn attached(state: InputState) -> bool {
    matches!(state, InputState::Attached | InputState::Tone)
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
                "1 while the engine reads the channel's input flow, or plays its \
                 line-up tone for a tone channel; 0 while it is pruned and polled \
                 until it is readable again, or detached.",
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
            self.attached[i].set(f64::from(u8::from(attached(input.state))));
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

/// Appends the input liveness a majority of the set's audio voters
/// decided ([`Replica::input_liveness`]) while this replica leads, as an
/// `InputDown` or `InputUp` the leader stamps, so every replica of the
/// set silences and raises the input at the same index; an input late on
/// one node alone goes down nowhere. Drains the engine's own `reports`,
/// which decide nothing in a set. Runs on a thread of its own until the
/// engine stops or the replica is gone.
pub fn report_inputs(reports: Receiver<Report>, replica: Weak<Replica<Mixer>>) {
    let forward = move || loop {
        match reports.recv_timeout(LIVENESS_POLL) {
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let Some(replica) = replica.upgrade() else {
            return;
        };
        for report in replica.input_liveness() {
            let body = match report {
                Report::InputPruned { slot } => input_entry(slot as u32, false),
                Report::InputReadable { slot } => input_entry(slot as u32, true),
            };
            if let Err(e) = replica.append(body) {
                tracing::warn!("input liveness not appended: {e}");
            }
        }
    };
    if let Err(e) = std::thread::Builder::new()
        .name("faderz-inputs".into())
        .spawn(forward)
    {
        tracing::error!("no thread reports the inputs' health to the set: {e}");
    }
}

/// How often the leader asks for the input liveness the set decided.
const LIVENESS_POLL: Duration = Duration::from_millis(10);

/// The log entry of slot `slot` up or down, at an index the leader
/// stamps.
fn input_entry(slot: u32, up: bool) -> Body {
    if up {
        Body::InputUp { slot, at_index: 0 }
    } else {
        Body::InputDown { slot, at_index: 0 }
    }
}

/// Each measurement's reset count in `state`.
fn resets_of(state: &Value) -> BTreeMap<String, u64> {
    (state["loudness"].as_object().into_iter().flatten())
        .map(|(id, m)| (id.clone(), m["resets"].as_u64().unwrap_or(0)))
        .collect()
}

/// The state and revision `replica` holds now: shared, so the store lock
/// is held for a pointer copy, not a copy of the state.
pub fn state_of(replica: &Replica<Mixer>) -> (Arc<Value>, u64) {
    crate::app::with_state(replica, |sm| (sm.product.shared_state(), sm.revision))
}

/// [`state_of`], owned: copied after the store lock is released.
pub fn owned_state_of(replica: &Replica<Mixer>) -> (Value, u64) {
    let (state, revision) = state_of(replica);
    (Arc::unwrap_or_clone(state), revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A measurement restarted when the state counts a reset of it and
    /// this replica applied none, or it started elsewhere than the last
    /// one applied starts it, once that start is past by
    /// [`RESET_PENDING`]. Without a reset counted nothing restarted; one
    /// not started has not.
    #[test]
    fn a_measurement_restarted_unless_it_started_at_the_last_reset() {
        let at = 100 * WINDOW;
        let later = at + RESET_PENDING;
        let none = Resets::default();
        let applied = Resets {
            counted: 1,
            at: Some(at),
        };
        let missed = Resets {
            counted: 1,
            at: None,
        };
        assert!(!restarted(None, missed, later));
        // No reset ever: every replica measures from its own start.
        assert!(!restarted(Some(7 * WINDOW), none, later));
        // Counted and never applied: started after the reset.
        assert!(restarted(Some(7 * WINDOW), missed, later));
        assert!(!restarted(Some(at), applied, later));
        // Started elsewhere than the reset applied starts it.
        assert!(restarted(Some(at + WINDOW), applied, later));
        assert!(restarted(Some(at - WINDOW), applied, later));
        // Applied, and not reached yet: pending.
        assert!(!restarted(Some(at - WINDOW), applied, later - 1));
    }

    /// A start on libmxl's clock reads as the UTC time that far back from
    /// now, within the time the two clock reads take.
    #[test]
    fn a_start_reads_as_utc_that_far_back() {
        let now = crate::replica::tai_samples();
        let since = utc_of(now - 60 * crate::replica::RATE, now);
        let want = std::time::SystemTime::now() - Duration::from_secs(60);
        let want = want
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        // Seconds into the day, from "YYYY-MM-DDTHH:MM:SS.mmmZ".
        let ms = |t: &str| {
            let n = |r: std::ops::Range<usize>| t[r].parse::<i64>().unwrap();
            ((n(11..13) * 60 + n(14..16)) * 60 + n(17..19)) * 1000 + n(20..23)
        };
        let day = 86_400_000;
        let diff = (ms(&since) - want.rem_euclid(day)).rem_euclid(day);
        assert!(diff.min(day - diff) <= 1000, "{since}");
        assert!(since.ends_with('Z'), "{since}");
    }

    #[test]
    fn the_resets_counted_name_their_bus() {
        let state = json!({ "loudness": { "a": { "running": true }, "b": { "resets": 3 } } });
        assert_eq!(
            resets_of(&state),
            BTreeMap::from([("a".to_owned(), 0), ("b".to_owned(), 3)])
        );
    }

    /// A tone channel's input is live for good, as an attached flow is;
    /// a pruned or detached one is not.
    #[test]
    fn a_tone_input_counts_as_attached() {
        assert!(attached(InputState::Attached));
        assert!(attached(InputState::Tone));
        assert!(!attached(InputState::Pruned));
        assert!(!attached(InputState::Detached));
    }

    /// The margin an input needs is the larger of the two the engine
    /// reports, either of which may be absent.
    #[test]
    fn the_margin_needed_is_the_larger_report() {
        let status = |warning, short| InputStatus {
            name: "in".into(),
            state: InputState::Attached,
            up: true,
            readable: Some(true),
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

    /// counters.schema.json names exactly the counters the `counters`
    /// message carries, each with a title and a description, so the
    /// console never shows a counter it cannot explain.
    #[test]
    fn the_counters_schema_names_every_counter_sent() {
        let schema: Value =
            serde_json::from_str(include_str!("../api/ws/counters.schema.json")).unwrap();
        let named: std::collections::BTreeSet<&str> = (schema["properties"].as_object())
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let c = Counters::default();
        let sent: std::collections::BTreeSet<String> = (engine_counters(&c).iter())
            .map(|(name, _)| format!("{name}_total"))
            .chain(["input_too_late_blocks_total".into(), "resends_total".into()])
            // App::counters_message adds the tallies taken late.
            .chain(["afv_late_total".into()])
            .collect();
        assert_eq!(
            named,
            sent.iter().map(String::as_str).collect(),
            "counters.schema.json and the counters message"
        );
        for name in named {
            for key in ["title", "description"] {
                let text = schema["properties"][name][key].as_str().unwrap_or("");
                assert!(!text.is_empty(), "{name} has no {key}");
            }
        }
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
