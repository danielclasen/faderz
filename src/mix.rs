//! The live state as the engine's gain matrix: one row per input channel
//! (the legs of each channel in shape order) and one column per output
//! channel (the legs of each shape output in shape order), then per leg
//! of each element the engine meters on columns of its own
//! ([`Shape::meter_columns`]).
//!
//! What a matrix coefficient can carry is applied: fader, mute, VCA
//! offsets and mutes, AFV open or closed, pan of a mono leg and the
//! fold-down of a wider channel, bus routes, sends
//! and their levels, bus, aux and mix-minus faders, mutes and polarity,
//! the mix-minus exclusions, and the monitor's source, downmix, level,
//! dim, cut, mono and speaker mutes and solos. Everything else in the
//! state is the input's strip ([`crate::strip`]), a duck group
//! ([`crate::duck`]) or is kept and not applied. [`targets`] adds the
//! strips, the duck groups and their masks, and each input's flow.
//!
//! Every value is computed with `libm`, not the host's math library, so
//! replicas on other hosts derive the same bits from the same state.

use std::collections::BTreeSet;

use samplez::Targets;
use serde::Serialize;
use serde_json::Value;

use crate::shape::Shape;
use crate::state::tokens;
use crate::{duck, strip};

/// A fader at this level or below is closed.
const CLOSED_DB: f64 = -90.0;
/// The most a channel's fader and VCA offsets add up to.
const MAX_CHANNEL_DB: f64 = 20.0;

/// The gain of channel `id` after its fader, mute and VCAs, before AFV,
/// the same on every leg.
pub fn post_fader(state: &Value, id: &str) -> f64 {
    channel(state, id, Tap::PostFader)
}

/// Linear gain of `db`, 0 at [`CLOSED_DB`] and below.
pub fn gain(db: f64) -> f64 {
    if db <= CLOSED_DB {
        0.0
    } else {
        libm::pow(10.0, db / 20.0)
    }
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn on(v: &Value) -> bool {
    v.as_bool().unwrap_or(false)
}

/// Where a channel's signal is taken.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tap {
    /// The selected input; the engine has no tap before the strip, so
    /// it is the pre-fader signal.
    Input,
    /// After the strip: trim, leg trim, polarity and processing.
    PreFader,
    /// After fader, mute and VCAs, before AFV.
    PostFader,
    /// After AFV: what buses and post-fader sends get.
    Mix,
}

/// The gain of channel `id` at `tap`, the same on every leg.
fn channel(state: &Value, id: &str, tap: Tap) -> f64 {
    // Trim, leg trim, polarity and the processing are the input's strip,
    // which runs before the matrix: the input tap is after it too.
    if matches!(tap, Tap::Input | Tap::PreFader) {
        return 1.0;
    }
    let c = &state["channels"][id];
    let mut g = 1.0;
    let mut db = num(&c["fader"]);
    let mut muted = on(&c["mute"]);
    if let Some(vcas) = state["vcas"].as_object() {
        for v in vcas.values() {
            let member = v["members"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m == id));
            if member {
                db += num(&v["offset"]);
                muted |= on(&v["mute"]);
            }
        }
    }
    if muted || num(&c["fader"]) <= CLOSED_DB {
        return 0.0;
    }
    g *= gain(db.min(MAX_CHANNEL_DB));
    if tap == Tap::PostFader {
        return g;
    }
    let afv = &state["afv"][id];
    if on(&afv["enabled"]) && !on(&afv["open"]) {
        g *= gain(num(&afv["offLevel"]));
    }
    g
}

/// The gain from leg `i` of a `from`-leg signal to leg `j` of a `to`-leg
/// element (model `Layout`: 1 mono, 2 stereo L R, 6 5.1 L R C LFE Ls Rs,
/// 8 7.1 L R C LFE Ls Rs Lrs Rrs), per model `PanState`: a mono signal
/// pans onto the first two legs at constant power by `pan` (-1 to 1);
/// any other signal ignores `pan`, maps leg to leg by label onto an
/// element as wide or wider, and folds down onto a narrower one.
pub fn pan(from: usize, to: usize, i: usize, j: usize, pan: f64) -> f64 {
    match (from, to) {
        (1, 1) => 1.0,
        (1, _) => {
            let theta = (pan.clamp(-1.0, 1.0) + 1.0) * std::f64::consts::FRAC_PI_4;
            match j {
                0 => libm::cos(theta),
                1 => libm::sin(theta),
                _ => 0.0,
            }
        }
        // Onto mono: the stereo fold summed at -6 dB, so a correlated
        // pair keeps its level. The contract does not define it.
        (_, 1) => (fold(from, 2, i, 0) + fold(from, 2, i, 1)) / 2.0,
        _ => fold(from, to, i, j),
    }
}

/// Leg to leg by label when `to` is at least as wide as `from`; else 5.1
/// and 7.1 onto stereo by the ITU-R BS.775 Lo/Ro equations (centre and
/// surrounds at -3 dB, LFE dropped) and 7.1 onto 5.1 by summing each side
/// and back surround pair at -3 dB (ETSI TS 103 190-1 Table 219).
fn fold(from: usize, to: usize, i: usize, j: usize) -> f64 {
    const H: f64 = std::f64::consts::FRAC_1_SQRT_2;
    match (from, to) {
        (6 | 8, 2) => match (i, j) {
            (0, 0) | (1, 1) => 1.0,
            (2, _) | (4 | 6, 0) | (5 | 7, 1) => H,
            _ => 0.0,
        },
        (8, 6) => match (i, j) {
            (4, 4) | (5, 5) | (6, 4) | (7, 5) => H,
            _ => f64::from(u8::from(i == j)),
        },
        _ => f64::from(u8::from(i == j)),
    }
}

/// The output and meter channels' gains for each input channel:
/// `[input][column]`. Each channel's gain at each tap and each element's
/// fader, mute and polarity are taken once, then multiplied into every
/// coefficient in the same order of operations as one coefficient at a
/// time, so the bits are those.
pub fn matrix(shape: &Shape, state: &Value) -> Vec<Vec<f32>> {
    let factors: Vec<Factors> = (shape.channels.iter())
        .map(|c| Factors::of(state, &c.id))
        .collect();
    gains(shape, state, &factors)
}

/// [`matrix`] with each channel's `factors` given.
fn gains(shape: &Shape, state: &Value, factors: &[Factors]) -> Vec<Vec<f32>> {
    let all: Vec<usize> = (0..shape.channels.len()).collect();
    let mix = Mix::of(shape, state, &all, |slot| factors[slot]);
    let columns = columns(shape);
    let width = columns.iter().map(|c| c.1).sum();
    let mut m = vec![vec![0.0; width]; mix.rows];
    let rows: Vec<usize> = (0..mix.rows).collect();
    let mut g = Vec::with_capacity(mix.rows * 8);
    let mut first = 0;
    for (source, width) in columns {
        let ew = mix.element(source, &mut g);
        put(&mut m, &rows, first, width, ew, &g);
        first += width;
    }
    m
}

/// The element each of the matrix's columns carries and its width: the
/// outputs, then the meter columns ([`Shape::meter_columns`]).
pub(crate) fn columns(shape: &Shape) -> Vec<(&str, usize)> {
    let outputs = shape.outputs.iter().map(|o| (o.source.as_str(), o.width));
    let meters = shape.meter_columns().into_iter().map(|(id, w, _)| (id, w));
    outputs.chain(meters).collect()
}

/// Writes the gains `g` of an element `ew` legs wide ([`Mix::element`])
/// into the columns from `first` on of an output `width` legs wide, its
/// rows into rows `rows` of `m`; legs the element lacks are 0.
fn put(m: &mut [Vec<f32>], rows: &[usize], first: usize, width: usize, ew: usize, g: &[f64]) {
    for (i, &row) in rows.iter().enumerate() {
        for leg in 0..width {
            let v = if leg < ew { g[i * ew + leg] } else { 0.0 };
            m[row][first + leg] = v as f32;
        }
    }
}

/// One channel's input as the MXL configuration wires it: the flow of
/// input a, the flow of input b when the channel has one, and the domain
/// both are in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wiring {
    pub a: String,
    pub b: Option<String>,
    pub domain: String,
}

/// The flow each channel's selected input reads: input b's when the
/// state selects b and the channel has one, else input a's.
pub fn selected(shape: &Shape, wiring: &[Wiring], state: &Value) -> Vec<String> {
    (shape.channels.iter())
        .zip(wiring)
        .map(|(c, w)| flow(state, &c.id, w).to_owned())
        .collect()
}

/// The flow channel `id`, wired as `w`, reads.
pub(crate) fn flow<'w>(state: &Value, id: &str, w: &'w Wiring) -> &'w str {
    match (state["channels"][id]["input"].as_str(), &w.b) {
        (Some("b"), Some(b)) => b,
        _ => &w.a,
    }
}

/// Everything the engine of `shape` plays from `state`: the gain matrix
/// ([`matrix`]), each channel's strip ([`strip::params`]), the duck
/// groups ([`duck::targets`]) and masks ([`duck::masks`]) and the flow
/// its selected input reads ([`selected`]), one slot per channel. Every
/// output is enabled.
pub fn targets(shape: &Shape, wiring: &[Wiring], state: &Value) -> Targets {
    Derived::new(shape, wiring, state).targets
}

/// What IS-05 activations set beside the state ([`crate::state::Mixer::engine_view`]):
/// each channel's flows with the leg flows activations named, the
/// channels whose receiver is disabled, and each output's enable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EngineView {
    pub wiring: Vec<Wiring>,
    pub detached: Vec<bool>,
    pub outputs: Vec<bool>,
}

impl EngineView {
    /// [`targets`] of `state` read through this view.
    pub fn targets(&self, shape: &Shape, state: &Value) -> Targets {
        let mut t = targets(shape, &self.wiring, state);
        self.overlay(&mut t);
        t
    }

    /// Detaches the disabled channels and sets each output's enable.
    pub fn overlay(&self, t: &mut Targets) {
        for (flow, detached) in t.flows.iter_mut().zip(&self.detached) {
            if *detached {
                *flow = None;
            }
        }
        t.outputs.clone_from(&self.outputs);
    }
}

/// [`targets`] kept from one write to the next: a write derives again
/// only what the leaves it changed move, with the same operations as
/// [`targets`], so the bits are those ([`Derived::update`]).
pub struct Derived {
    targets: Targets,
    /// Each channel's [`Factors`].
    factors: Vec<Factors>,
    /// Each channel's first row.
    first: Vec<usize>,
    /// The element each column carries ([`columns`]), its width and its
    /// first column.
    columns: Vec<(String, usize, usize)>,
}

/// What a write moves in [`targets`].
#[derive(Default)]
struct Moved {
    /// Every target.
    all: bool,
    /// The rows of these channel slots.
    rows: BTreeSet<usize>,
    /// The columns of these buses, auxes and mix-minus outputs.
    elements: BTreeSet<String>,
    /// The columns of the monitor outputs.
    monitor: bool,
    /// The strips and flows of these slots.
    strips: BTreeSet<usize>,
    flows: BTreeSet<usize>,
    /// The duck masks: a group's targets or destinations, or the
    /// monitor's source.
    masks: bool,
}

impl Derived {
    pub fn new(shape: &Shape, wiring: &[Wiring], state: &Value) -> Derived {
        let factors: Vec<Factors> = (shape.channels.iter())
            .map(|c| Factors::of(state, &c.id))
            .collect();
        let mut row = 0;
        let first = (shape.channels.iter())
            .map(|c| {
                row += c.width;
                row - c.width
            })
            .collect();
        let mut col = 0;
        let columns = (columns(shape).into_iter())
            .map(|(id, width)| {
                col += width;
                (id.to_owned(), width, col - width)
            })
            .collect();
        Derived {
            targets: Targets {
                gains: gains(shape, state, &factors),
                flows: selected(shape, wiring, state)
                    .into_iter()
                    .map(Some)
                    .collect(),
                strips: (shape.channels.iter())
                    .map(|c| strip::params(&state["channels"][&c.id], c.width))
                    .collect(),
                ducks: duck::targets(shape, state),
                masks: duck::masks(shape, state),
                ..Targets::default()
            },
            factors,
            first,
            columns,
        }
    }

    /// [`targets`] of the state last derived.
    pub fn targets(&self) -> &Targets {
        &self.targets
    }

    /// Derives again what a write of the leaves `paths` moved in
    /// `state`, the state after it; `vcas` is `/vcas` before it, whose
    /// former members a membership change moves.
    pub fn update<'p>(
        &mut self,
        shape: &Shape,
        wiring: &[Wiring],
        state: &Value,
        vcas: &Value,
        paths: impl IntoIterator<Item = &'p str>,
    ) {
        let moved = Moved::of(shape, state, vcas, paths);
        // Past half the channels, recomputing their rows and then the
        // columns besides costs more than the whole.
        if moved.all || moved.rows.len() * 2 > shape.channels.len() {
            *self = Derived::new(shape, wiring, state);
            return;
        }
        let mut g = Vec::new();
        if !moved.rows.is_empty() {
            let slots: Vec<usize> = moved.rows.into_iter().collect();
            for &slot in &slots {
                self.factors[slot] = Factors::of(state, &shape.channels[slot].id);
            }
            let mix = Mix::of(shape, state, &slots, |slot| self.factors[slot]);
            let rows: Vec<usize> = (slots.iter())
                .flat_map(|&slot| self.first[slot]..self.first[slot] + shape.channels[slot].width)
                .collect();
            for (id, width, first) in &self.columns {
                let ew = mix.element(id, &mut g);
                put(&mut self.targets.gains, &rows, *first, *width, ew, &g);
            }
        }
        let source = state["monitor"]["source"].as_str().unwrap_or_default();
        let monitor = moved.monitor || moved.elements.contains(source);
        let moved_columns: Vec<&(String, usize, usize)> = (self.columns.iter())
            .filter(|(id, ..)| {
                moved.elements.contains(id)
                    || monitor && shape.monitor_outputs.iter().any(|m| m.id == *id)
            })
            .collect();
        if !moved_columns.is_empty() {
            let all: Vec<usize> = (0..shape.channels.len()).collect();
            let mix = Mix::of(shape, state, &all, |slot| self.factors[slot]);
            let rows: Vec<usize> = (0..mix.rows).collect();
            for (id, width, first) in moved_columns {
                let ew = mix.element(id, &mut g);
                put(&mut self.targets.gains, &rows, *first, *width, ew, &g);
            }
        }
        for slot in moved.strips {
            let c = &shape.channels[slot];
            self.targets.strips[slot] = strip::params(&state["channels"][&c.id], c.width);
        }
        for slot in moved.flows {
            self.targets.flows[slot] =
                Some(flow(state, &shape.channels[slot].id, &wiring[slot]).to_owned());
        }
        // A handful of groups, each read in full: a key gain follows its
        // channel's fader, mute and VCAs, which any write can move.
        if !shape.ducks.is_empty() {
            self.targets.ducks = duck::targets(shape, state);
        }
        if moved.masks {
            self.targets.masks = duck::masks(shape, state);
        }
    }
}

impl Moved {
    /// What a write of the leaves `paths` moves, `vcas` being `/vcas`
    /// before it. [`targets`] reads the channels, AFV stages, VCAs,
    /// buses, auxes, mix-minus outputs and the monitor; a leaf of
    /// another group it is known not to read moves nothing, any other
    /// moves everything.
    fn of<'p>(
        shape: &Shape,
        state: &Value,
        vcas: &Value,
        paths: impl IntoIterator<Item = &'p str>,
    ) -> Moved {
        let slot = |id: &str| shape.channels.iter().position(|c| c.id == id);
        let mut moved = Moved::default();
        for path in paths {
            let t = tokens(path);
            let group = t.first().map(String::as_str);
            let id = t.get(1).map(String::as_str);
            let key = t.get(2).map(String::as_str);
            match (group, id) {
                (Some("channels"), Some(id)) => {
                    let Some(s) = slot(id) else {
                        moved.all = true;
                        continue;
                    };
                    match key {
                        Some("fader" | "mute" | "routes" | "sends" | "pan" | "directOut") => {
                            moved.rows.insert(s);
                        }
                        Some(
                            "trim" | "legTrim" | "polarity" | "hpf" | "eq" | "gate" | "comp"
                            | "dynamicsBeforeEq",
                        ) => {
                            moved.strips.insert(s);
                        }
                        Some("input") => {
                            moved.flows.insert(s);
                        }
                        _ => {
                            moved.rows.insert(s);
                            moved.strips.insert(s);
                            moved.flows.insert(s);
                        }
                    }
                }
                (Some("afv"), Some(id)) => match slot(id) {
                    Some(s) => {
                        moved.rows.insert(s);
                    }
                    None => moved.all = true,
                },
                // A VCA moves its members, those it had and those it has.
                (Some("vcas"), _) => {
                    for vcas in [vcas, &state["vcas"]] {
                        let touched: Vec<&Value> = match id {
                            Some(v) => vec![&vcas[v]],
                            None => vcas
                                .as_object()
                                .into_iter()
                                .flat_map(|v| v.values())
                                .collect(),
                        };
                        let members = (touched.into_iter())
                            .filter_map(|v| v["members"].as_array())
                            .flatten();
                        moved.rows.extend(members.filter_map(|m| slot(m.as_str()?)));
                    }
                }
                (Some("buses" | "auxes" | "mixMinus"), Some(id)) => {
                    moved.elements.insert(id.to_owned());
                }
                (Some("monitor"), _) => {
                    moved.monitor = true;
                    moved.masks |= matches!(id, None | Some("source"));
                }
                // A group, or a group's triggers, targets or destinations.
                (Some("ducks"), _) => {
                    moved.masks |= t.len() < 4 || duck::routing_of(path).is_some();
                }
                (
                    Some(
                        "tally" | "loudness" | "recallSafe" | "scenes" | "consoles" | "metering"
                        | "tone" | "sceneRecalled",
                    ),
                    _,
                ) => {}
                _ => moved.all = true,
            }
        }
        moved
    }
}

/// A channel's gains after its strip, the same on every leg.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterTap {
    /// At its `meterTap`.
    pub meter: f64,
    /// At its direct out's tap.
    pub direct: f64,
}

/// Each channel's [`MeterTap`]: the engine meters the channel after its
/// strip, which is the input and pre-fader tap; a post-fader tap adds
/// the fader, mute and VCAs.
pub fn meter_taps(shape: &Shape, state: &Value) -> Vec<MeterTap> {
    let tap = |v: &Value| match v.as_str() {
        Some("postFader") => Tap::PostFader,
        _ => Tap::PreFader,
    };
    shape
        .channels
        .iter()
        .map(|c| {
            let s = &state["channels"][&c.id];
            MeterTap {
                meter: channel(state, &c.id, tap(&s["meterTap"])),
                direct: channel(state, &c.id, tap(&s["directOut"]["tap"])),
            }
        })
        .collect()
}

/// A channel's gain at [`Tap::PostFader`] and at [`Tap::Mix`]
/// ([`channel`]), the same on every leg.
#[derive(Clone, Copy)]
struct Factors {
    post: f64,
    mix: f64,
}

impl Factors {
    fn of(state: &Value, id: &str) -> Factors {
        Factors {
            post: channel(state, id, Tap::PostFader),
            mix: channel(state, id, Tap::Mix),
        }
    }
}

/// One channel as the matrix reads it.
struct Chan<'a> {
    id: &'a str,
    state: &'a Value,
    routes: &'a Value,
    sends: &'a Value,
    /// Its pan, `pan.lr`.
    pan: f64,
    /// The row of its first leg.
    row: usize,
    width: usize,
    factors: Factors,
}

impl Chan<'_> {
    fn at(&self, tap: Tap) -> f64 {
        match tap {
            Tap::Input | Tap::PreFader => 1.0,
            Tap::PostFader => self.factors.post,
            Tap::Mix => self.factors.mix,
        }
    }
}

/// What an element is, which says how a channel feeds it.
enum Kind<'a> {
    Channel,
    Bus,
    Aux,
    /// Its bus and the channels it excludes.
    MixMinus(&'a str, &'a Value),
    Monitor,
    Other,
}

/// The state's matrix over some of its channels, each channel's tap
/// gains taken once.
struct Mix<'a> {
    shape: &'a Shape,
    state: &'a Value,
    rows: usize,
    chans: Vec<Chan<'a>>,
}

impl<'a> Mix<'a> {
    /// The channels at `slots` with the gains `factors` gives each slot,
    /// their rows counted from 0 in that order. A coefficient takes only
    /// its own channel's state and its element's, so a channel's rows
    /// are the same bits whichever others are with it.
    fn of(
        shape: &'a Shape,
        state: &'a Value,
        slots: &[usize],
        factors: impl Fn(usize) -> Factors,
    ) -> Mix<'a> {
        let mut row = 0;
        let chans = (slots.iter())
            .map(|&slot| {
                let c = &shape.channels[slot];
                let s = &state["channels"][&c.id];
                let chan = Chan {
                    id: &c.id,
                    state: s,
                    routes: &s["routes"],
                    sends: &s["sends"],
                    pan: num(&s["pan"]["lr"]),
                    row,
                    width: c.width,
                    factors: factors(slot),
                };
                row += c.width;
                chan
            })
            .collect();
        Mix {
            shape,
            state,
            rows: row,
            chans,
        }
    }

    fn kind(&self, id: &str) -> Kind<'a> {
        let shape = self.shape;
        if shape.channels.iter().any(|c| c.id == id) {
            Kind::Channel
        } else if shape.buses.iter().any(|b| b.id == id) {
            Kind::Bus
        } else if shape.auxes.iter().any(|a| a.id == id) {
            Kind::Aux
        } else if let Some(m) = shape.mix_minus.iter().find(|m| m.id == id) {
            Kind::MixMinus(&m.bus, &self.state["mixMinus"][id]["exclude"])
        } else if shape.monitor_outputs.iter().any(|m| m.id == id) {
            Kind::Monitor
        } else {
            Kind::Other
        }
    }

    /// The gains of element `id` from every input channel into `g`, row
    /// by row, `[input * width + leg]`; returns its width.
    fn element(&self, id: &str, g: &mut Vec<f64>) -> usize {
        let width = width_of(self.shape, id);
        g.clear();
        g.resize(self.rows * width, 0.0);
        let kind = self.kind(id);
        if let Kind::Monitor = kind {
            self.monitor(width, g);
            return width;
        }
        for c in &self.chans {
            let Some((tap, level, p)) = feed(c, &kind, id) else {
                continue;
            };
            let at = c.at(tap) * level;
            for i in 0..c.width {
                let row = &mut g[(c.row + i) * width..][..width];
                for (j, out) in row.iter_mut().enumerate() {
                    *out = at * pan(c.width, width, i, j, p);
                }
            }
        }
        match kind {
            Kind::Bus => scale(g, width, &self.state["buses"][id], true),
            Kind::Aux => scale(g, width, &self.state["auxes"][id], true),
            Kind::MixMinus(..) => scale(g, width, &self.state["mixMinus"][id], false),
            _ => {}
        }
        width
    }

    /// The monitor output: its source's legs onto the monitor's, folded
    /// down by `downmix` when the source is wider, then level, dim, cut,
    /// mono (the sum of the folded legs) and the speaker mutes and solos.
    fn monitor(&self, width: usize, g: &mut [f64]) {
        let m = &self.state["monitor"];
        let source = m["source"].as_str().unwrap_or_default();
        let mut src = Vec::new();
        let from = self.element(source, &mut src);
        let mut level = gain(num(&m["level"]));
        if on(&m["dim"]) {
            level *= gain(-num(&m["dimDepth"]));
        }
        if on(&m["cut"]) {
            level = 0.0;
        }
        let lr = m["downmix"] == "lr";
        let mono = on(&m["mono"]);
        if mono {
            level *= gain(-num(&m["monoCompensation"]));
        }
        // The source's legs onto monitor leg `j`, folded by the downmix:
        // lr plays only the source's L and R legs.
        let folded = |legs: &[f64], j: usize| -> f64 {
            if lr && from > width {
                legs.get(j).copied().filter(|_| j < 2).unwrap_or(0.0)
            } else {
                (0..from)
                    .map(|i| legs[i] * pan(from, width, i, j, 0.0))
                    .sum()
            }
        };
        let solo = m["speakerSolo"]
            .as_array()
            .is_some_and(|s| s.iter().any(on));
        let leg_gain: Vec<f64> = (0..width)
            .map(|j| {
                let audible = !on(&m["speakerMute"][j]) && (!solo || on(&m["speakerSolo"][j]));
                if audible { level } else { 0.0 }
            })
            .collect();
        for row in 0..self.rows {
            let legs = &src[row * from..][..from];
            for (j, out) in g[row * width..][..width].iter_mut().enumerate() {
                let v: f64 = if mono {
                    (0..width).map(|k| folded(legs, k)).sum()
                } else {
                    folded(legs, j)
                };
                *out = v * leg_gain[j];
            }
        }
    }
}

/// How channel `c` feeds element `id` of `kind`: tap, level and pan, or
/// `None` when it does not.
fn feed(c: &Chan<'_>, kind: &Kind<'_>, id: &str) -> Option<(Tap, f64, f64)> {
    match kind {
        Kind::Channel if c.id == id => {
            let tap = match c.state["directOut"]["tap"].as_str() {
                Some("input") => Tap::Input,
                Some("postFader") => Tap::PostFader,
                _ => Tap::PreFader,
            };
            Some((tap, 1.0, 0.0))
        }
        Kind::Bus => on(&c.routes[id]).then_some((Tap::Mix, 1.0, c.pan)),
        Kind::Aux => {
            let send = &c.sends[id];
            if !on(&send["on"]) {
                return None;
            }
            let tap = if on(&send["pre"]) {
                Tap::PreFader
            } else {
                Tap::Mix
            };
            Some((tap, gain(num(&send["level"])), num(&send["pan"])))
        }
        Kind::MixMinus(bus, exclude) => {
            let excluded = exclude
                .as_array()
                .is_some_and(|e| e.iter().any(|e| e == c.id));
            (on(&c.routes[*bus]) && !excluded).then_some((Tap::Mix, 1.0, c.pan))
        }
        _ => None,
    }
}

/// Applies an element's fader, mute and, when it has one, polarity to
/// `g`, rows of `width` legs.
fn scale(g: &mut [f64], width: usize, state: &Value, polarity: bool) {
    let level = if on(&state["mute"]) {
        0.0
    } else {
        gain(num(&state["fader"]))
    };
    let legs: Vec<f64> = (0..width)
        .map(|leg| {
            let sign = if polarity && on(&state["polarity"][leg]) {
                -1.0
            } else {
                1.0
            };
            sign * level
        })
        .collect();
    for row in g.chunks_mut(width.max(1)) {
        for (v, l) in row.iter_mut().zip(&legs) {
            *v *= l;
        }
    }
}

fn width_of(shape: &Shape, id: &str) -> usize {
    shape
        .channels
        .iter()
        .map(|e| (&e.id, e.width))
        .chain(shape.buses.iter().map(|e| (&e.id, e.width)))
        .chain(shape.auxes.iter().map(|e| (&e.id, e.width)))
        .chain(shape.mix_minus.iter().map(|e| (&e.id, e.width)))
        .chain(shape.monitor_outputs.iter().map(|e| (&e.id, e.width)))
        .find(|(e, _)| *e == id)
        .map_or(0, |(_, w)| w)
}

#[cfg(test)]
mod tests {

    /// The matrix as it was computed one coefficient at a time, before
    /// the per-channel and per-element factors were taken once: the
    /// reference the single-replica engine path was sent.
    mod before {
        use super::super::{Tap, channel, gain, num, on, pan, width_of};
        use crate::shape::Shape;
        use serde_json::Value;

        /// The output and meter channels' gains for each input channel:
        /// `[input][column]`.
        pub fn matrix(shape: &Shape, state: &Value) -> Vec<Vec<f32>> {
            let rows: usize = shape.channels.iter().map(|c| c.width).sum();
            let mut m = vec![Vec::new(); rows];
            let outputs = shape.outputs.iter().map(|o| (o.source.as_str(), o.width));
            let meters = shape.meter_columns().into_iter().map(|(id, w, _)| (id, w));
            for (source, width) in outputs.chain(meters) {
                let element = element(shape, state, source);
                for leg in 0..width {
                    for (row, r) in m.iter_mut().enumerate() {
                        let g = element.get(row).and_then(|g| g.get(leg)).copied();
                        r.push(g.unwrap_or(0.0) as f32);
                    }
                }
            }
            m
        }

        /// The gains of element `id` from every input channel: `[input][leg]`.
        fn element(shape: &Shape, state: &Value, id: &str) -> Vec<Vec<f64>> {
            let rows: usize = shape.channels.iter().map(|c| c.width).sum();
            let width = width_of(shape, id);
            let mut g = vec![vec![0.0; width]; rows];
            // The row of each channel's first leg.
            let mut row = 0;
            for c in &shape.channels {
                let feed = feed(shape, state, &c.id, id);
                if let Some((tap, level, p)) = feed {
                    for i in 0..c.width {
                        let at = channel(state, &c.id, tap) * level;
                        for (j, out) in g[row + i].iter_mut().enumerate() {
                            *out = at * pan(c.width, width, i, j, p);
                        }
                    }
                }
                row += c.width;
            }
            if let Some(b) = shape.buses.iter().find(|b| b.id == id) {
                scale(&mut g, &state["buses"][&b.id], true);
            } else if let Some(a) = shape.auxes.iter().find(|a| a.id == id) {
                scale(&mut g, &state["auxes"][&a.id], true);
            } else if shape.mix_minus.iter().any(|m| m.id == id) {
                scale(&mut g, &state["mixMinus"][id], false);
            } else if shape.monitor_outputs.iter().any(|m| m.id == id) {
                return monitor(shape, state, width);
            }
            g
        }

        /// How channel `channel` feeds element `id`: tap, level and pan, or
        /// `None` when it does not.
        fn feed(shape: &Shape, state: &Value, channel: &str, id: &str) -> Option<(Tap, f64, f64)> {
            let c = &state["channels"][channel];
            let pan = num(&c["pan"]["lr"]);
            if channel == id {
                let tap = match c["directOut"]["tap"].as_str() {
                    Some("input") => Tap::Input,
                    Some("postFader") => Tap::PostFader,
                    _ => Tap::PreFader,
                };
                return Some((tap, 1.0, 0.0));
            }
            if shape.buses.iter().any(|b| b.id == id) {
                return on(&c["routes"][id]).then_some((Tap::Mix, 1.0, pan));
            }
            if shape.auxes.iter().any(|a| a.id == id) {
                let s = &c["sends"][id];
                let tap = if on(&s["pre"]) {
                    Tap::PreFader
                } else {
                    Tap::Mix
                };
                return on(&s["on"]).then_some((tap, gain(num(&s["level"])), num(&s["pan"])));
            }
            let m = shape.mix_minus.iter().find(|m| m.id == id)?;
            let excluded = state["mixMinus"][id]["exclude"]
                .as_array()
                .is_some_and(|e| e.iter().any(|e| e == channel));
            (on(&c["routes"][&m.bus]) && !excluded).then_some((Tap::Mix, 1.0, pan))
        }

        /// Applies an element's fader, mute and, when it has one, polarity.
        fn scale(g: &mut [Vec<f64>], state: &Value, polarity: bool) {
            let level = if on(&state["mute"]) {
                0.0
            } else {
                gain(num(&state["fader"]))
            };
            for row in g {
                for (leg, v) in row.iter_mut().enumerate() {
                    let sign = if polarity && on(&state["polarity"][leg]) {
                        -1.0
                    } else {
                        1.0
                    };
                    *v *= sign * level;
                }
            }
        }

        /// The monitor output: its source's legs onto the monitor's, folded down
        /// by `downmix` when the source is wider, then level, dim, cut, mono (the
        /// sum of the folded legs) and the speaker mutes and solos.
        fn monitor(shape: &Shape, state: &Value, width: usize) -> Vec<Vec<f64>> {
            let m = &state["monitor"];
            let source = m["source"].as_str().unwrap_or_default();
            let src = element(shape, state, source);
            let from = width_of(shape, source);
            let mut level = gain(num(&m["level"]));
            if on(&m["dim"]) {
                level *= gain(-num(&m["dimDepth"]));
            }
            if on(&m["cut"]) {
                level = 0.0;
            }
            let lr = m["downmix"] == "lr";
            let mono = on(&m["mono"]);
            if mono {
                level *= gain(-num(&m["monoCompensation"]));
            }
            // The source's legs onto monitor leg `j`, folded by the downmix: lr
            // plays only the source's L and R legs.
            let folded = |legs: &[f64], j: usize| -> f64 {
                if lr && from > width {
                    legs.get(j).copied().filter(|_| j < 2).unwrap_or(0.0)
                } else {
                    (0..from)
                        .map(|i| legs[i] * pan(from, width, i, j, 0.0))
                        .sum()
                }
            };
            let leg_gain = |j: usize| {
                let solo = m["speakerSolo"]
                    .as_array()
                    .is_some_and(|s| s.iter().any(on));
                let audible = !on(&m["speakerMute"][j]) && (!solo || on(&m["speakerSolo"][j]));
                if audible { level } else { 0.0 }
            };
            src.iter()
                .map(|legs| {
                    (0..width)
                        .map(|j| {
                            let v: f64 = if mono {
                                (0..width).map(|k| folded(legs, k)).sum()
                            } else {
                                folded(legs, j)
                            };
                            v * leg_gain(j)
                        })
                        .collect()
                })
                .collect()
        }
    }
    use super::*;
    use crate::shape::tests::small;
    use serde_json::json;

    fn shape() -> Shape {
        let mut s = small();
        s["outputs"] = json!([
            { "id": "o-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
            { "id": "o-aux", "name": "IFB", "source": "aux1", "width": 1, "layout": "mono" },
            { "id": "o-mm", "name": "N-1", "source": "mm1", "width": 2, "layout": "stereo" },
            { "id": "o-mon", "name": "Mon", "source": "mon", "width": 2, "layout": "stereo" },
            { "id": "o-ch1", "name": "DO 1", "source": "ch1", "width": 1, "layout": "mono" },
        ]);
        Shape::parse(&s).unwrap()
    }

    fn close(a: f32, b: f64) -> bool {
        (f64::from(a) - b).abs() < 1e-6
    }

    // Rows: ch1, ch2, st1 L, st1 R. Columns: o-pgm L R, o-aux, o-mm L R,
    // o-mon L R, o-ch1.

    /// A fresh state is silent everywhere but the direct out, which taps
    /// before the closed faders.
    #[test]
    fn the_initial_state_passes_only_the_direct_out() {
        let shape = shape();
        let m = matrix(&shape, &shape.initial_state());
        assert_eq!(m.len(), 4);
        // The outputs' 8 columns, then cf's 2, which no output carries.
        assert!(m.iter().all(|r| r.len() == 10));
        for (row, r) in m.iter().enumerate() {
            for (col, g) in r.iter().enumerate() {
                let want = if row == 0 && col == 7 { 1.0 } else { 0.0 };
                assert!(close(*g, want), "[{row}][{col}] = {g}");
            }
        }
    }

    /// A bus no output carries gets the columns an output of it would:
    /// cf, after the outputs, carries what an output of cf would.
    #[test]
    fn a_bus_no_output_carries_has_its_own_columns() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(0);
        s["channels"]["ch1"]["routes"]["cf"] = json!(true);
        s["buses"]["cf"]["fader"] = json!(-6);
        let m = matrix(&shape, &s);
        let mut with_output = crate::shape::tests::small();
        with_output["outputs"] = json!([
            { "id": "o-cf", "name": "CF", "source": "cf", "width": 2, "layout": "stereo" },
        ]);
        let cf = matrix(&Shape::parse(&with_output).unwrap(), &s);
        for (row, r) in m.iter().enumerate() {
            assert_eq!(r[8..], cf[row][..2], "row {row}");
        }
        assert!(m[0][8] > 0.3);
    }

    /// The engine meters a channel after its strip: a pre-fader meter tap
    /// reads it as it is, a post-fader one adds fader, mute and VCAs.
    #[test]
    fn a_post_fader_meter_tap_follows_the_fader() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(-6);
        s["channels"]["ch1"]["meterTap"] = json!("preFader");
        s["channels"]["ch2"]["fader"] = json!(-6);
        s["channels"]["ch2"]["meterTap"] = json!("postFader");
        s["channels"]["st1"]["meterTap"] = json!("postFader");
        s["channels"]["st1"]["directOut"]["tap"] = json!("postFader");
        s["channels"]["st1"]["fader"] = json!(0);
        s["channels"]["st1"]["mute"] = json!(true);
        let t = meter_taps(&shape, &s);
        let tap = |meter, direct| MeterTap { meter, direct };
        assert_eq!(t, [tap(1.0, 1.0), tap(gain(-6.0), 1.0), tap(0.0, 0.0)]);
    }

    #[test]
    fn a_mono_channel_pans_at_constant_power_onto_a_stereo_bus() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(0);
        let m = matrix(&shape, &s);
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!(close(m[0][0], h) && close(m[0][1], h));
        s["channels"]["ch1"]["pan"]["lr"] = json!(-1);
        let m = matrix(&shape, &s);
        assert!(close(m[0][0], 1.0) && close(m[0][1], 0.0));
    }

    /// Fader, mute, VCA and bus fader multiply into one coefficient;
    /// trim, leg trim and polarity are the strip's, so not in it.
    #[test]
    fn the_gain_stages_multiply() {
        let shape = shape();
        let mut s = shape.initial_state();
        let c = &mut s["channels"]["st1"];
        c["fader"] = json!(-6);
        c["trim"] = json!(3);
        c["legTrim"] = json!([0, -3]);
        c["polarity"] = json!([false, true]);
        s["buses"]["pgm"]["fader"] = json!(-10);
        s["vcas"]["vca1"]["members"] = json!(["st1"]);
        s["vcas"]["vca1"]["offset"] = json!(1);
        let m = matrix(&shape, &s);
        assert!(close(m[2][0], gain(-6.0 + 1.0 - 10.0)));
        assert!(close(m[2][1], 0.0));
        assert!(close(m[3][1], gain(-6.0 + 1.0 - 10.0)));
        s["vcas"]["vca1"]["mute"] = json!(true);
        assert!(matrix(&shape, &s)[2].iter().take(2).all(|g| *g == 0.0));
        s["vcas"]["vca1"]["mute"] = json!(false);
        s["channels"]["st1"]["mute"] = json!(true);
        assert!(matrix(&shape, &s)[2].iter().take(2).all(|g| *g == 0.0));
    }

    #[test]
    fn a_route_off_or_a_closed_afv_takes_the_channel_off_the_bus() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch2"]["fader"] = json!(0);
        assert!(matrix(&shape, &s)[1][0] > 0.0);
        s["afv"]["ch2"]["enabled"] = json!(true);
        s["afv"]["ch2"]["open"] = json!(false);
        s["afv"]["ch2"]["offLevel"] = json!(-20);
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!(close(matrix(&shape, &s)[1][0], h * gain(-20.0)));
        s["channels"]["ch2"]["routes"]["pgm"] = json!(false);
        assert_eq!(matrix(&shape, &s)[1][0], 0.0);
    }

    /// A send taps pre or post fader at its level; the mix-minus is the
    /// bus without its excluded channels and without the bus fader.
    #[test]
    fn sends_and_mix_minus() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["sends"]["aux1"] =
            json!({ "on": true, "level": -6, "pan": 0, "pre": true });
        assert!(close(matrix(&shape, &s)[0][2], gain(-6.0)));
        s["channels"]["ch1"]["sends"]["aux1"]["pre"] = json!(false);
        assert_eq!(matrix(&shape, &s)[0][2], 0.0);
        s["channels"]["ch1"]["fader"] = json!(0);
        s["channels"]["ch2"]["fader"] = json!(0);
        s["buses"]["pgm"]["fader"] = json!(-20);
        s["mixMinus"]["mm1"]["exclude"] = json!(["ch1"]);
        let m = matrix(&shape, &s);
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(m[0][3], 0.0);
        assert!(close(m[1][3], h));
    }

    #[test]
    fn the_monitor_follows_its_source_and_its_own_controls() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        s["monitor"]["level"] = json!(0);
        let m = matrix(&shape, &s);
        assert!(close(m[2][5], 1.0) && close(m[3][6], 1.0) && m[2][6] == 0.0);
        s["monitor"]["dim"] = json!(true);
        assert!(close(matrix(&shape, &s)[2][5], gain(-20.0)));
        s["monitor"]["dim"] = json!(false);
        s["monitor"]["mono"] = json!(true);
        let m = matrix(&shape, &s);
        assert!(close(m[3][5], gain(-3.0)) && close(m[2][6], gain(-3.0)));
        s["monitor"]["speakerMute"] = json!([true, false]);
        assert_eq!(matrix(&shape, &s)[2][5], 0.0);
        s["monitor"]["cut"] = json!(true);
        assert!(
            matrix(&shape, &s)
                .iter()
                .all(|r| r[5] == 0.0 && r[6] == 0.0)
        );
    }

    /// PanState: a channel as wide as the bus maps leg to leg whatever its
    /// pan; only a mono channel pans.
    #[test]
    fn a_stereo_channel_ignores_pan_and_a_mono_one_pans_hard_right() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        s["channels"]["ch1"]["fader"] = json!(0);
        for p in [0.5, 1.0, -1.0] {
            s["channels"]["st1"]["pan"]["lr"] = json!(p);
            let m = matrix(&shape, &s);
            assert!(close(m[2][0], 1.0) && close(m[2][1], 0.0), "pan {p}");
            assert!(close(m[3][0], 0.0) && close(m[3][1], 1.0), "pan {p}");
        }
        s["channels"]["ch1"]["pan"]["lr"] = json!(1);
        let m = matrix(&shape, &s);
        assert!(close(m[0][0], 0.0) && close(m[0][1], 1.0));
    }

    /// 5.1 and 7.1 onto stereo by ITU-R BS.775 Lo/Ro, 7.1 onto 5.1 by the
    /// surround pairs, both at -3 dB, LFE dropped; narrower onto wider
    /// leg to leg.
    #[test]
    fn wider_signals_fold_down_per_the_contract() {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        // L R C LFE Ls Rs onto Lo.
        let lo: Vec<f64> = (0..6).map(|i| pan(6, 2, i, 0, 0.7)).collect();
        assert_eq!(lo, [1.0, 0.0, h, 0.0, h, 0.0]);
        let ro: Vec<f64> = (0..8).map(|i| pan(8, 2, i, 1, 0.0)).collect();
        assert_eq!(ro, [0.0, 1.0, h, 0.0, 0.0, h, 0.0, h]);
        // Lrs into Ls, Rrs into Rs, the rest leg to leg.
        assert_eq!(pan(8, 6, 4, 4, 0.0), h);
        assert_eq!(pan(8, 6, 5, 5, 0.0), h);
        assert_eq!(pan(8, 6, 6, 4, 0.0), h);
        assert_eq!(pan(8, 6, 7, 5, 0.0), h);
        assert_eq!(pan(8, 6, 2, 2, 0.0), 1.0);
        assert_eq!(pan(8, 6, 6, 0, 0.0), 0.0);
        assert_eq!(pan(2, 6, 1, 1, -1.0), 1.0);
        assert_eq!(pan(2, 6, 1, 2, -1.0), 0.0);
        // Stereo onto mono at -6 dB per leg; 5.1 onto mono through Lo/Ro.
        assert_eq!(pan(2, 1, 0, 0, 0.0), 0.5);
        assert_eq!(pan(6, 1, 2, 0, 0.0), h);
        assert_eq!(pan(6, 1, 3, 0, 0.0), 0.0);
    }

    #[test]
    fn a_post_fader_send_follows_the_open_fader() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["sends"]["aux1"] =
            json!({ "on": true, "level": -6, "pan": 0, "pre": false });
        s["channels"]["ch1"]["fader"] = json!(-4);
        assert!(close(matrix(&shape, &s)[0][2], gain(-10.0)));
    }

    #[test]
    fn a_muted_bus_is_silent_and_its_mix_minus_is_not() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(0);
        s["buses"]["pgm"]["mute"] = json!(true);
        let m = matrix(&shape, &s);
        assert_eq!((m[0][0], m[0][1]), (0.0, 0.0));
        assert!(
            m[0][3] > 0.0,
            "the mix-minus is taken before bus processing"
        );
    }

    /// VcaState: fader and offsets add up to +20 dB at most.
    #[test]
    fn vca_offsets_clamp_at_20_db() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(10);
        s["channels"]["ch1"]["pan"]["lr"] = json!(-1);
        s["vcas"]["vca1"]["members"] = json!(["ch1"]);
        s["vcas"]["vca1"]["offset"] = json!(10);
        assert!(close(matrix(&shape, &s)[0][0], gain(20.0)));
        s["vcas"]["vca1"]["offset"] = json!(5);
        assert!(close(matrix(&shape, &s)[0][0], gain(15.0)));
    }

    /// MonitorState downmix: lr plays only the L and R legs of a wider
    /// source, loRo folds it.
    #[test]
    fn the_monitor_folds_a_wider_source_by_its_downmix() {
        let mut v = small();
        v["channels"] =
            json!([{ "id": "c51", "name": "5.1", "width": 6, "layout": "5.1", "altInput": false }]);
        v["buses"] = json!([{ "id": "sur", "name": "Surround", "kind": "program", "width": 6, "layout": "5.1" }]);
        v["auxes"] = json!([]);
        v["mixMinus"] = json!([]);
        v["vcas"] = json!([]);
        v["outputs"] = json!([{ "id": "o-mon", "name": "Mon", "source": "mon", "width": 2, "layout": "stereo" }]);
        let shape = Shape::parse(&v).unwrap();
        let mut s = shape.initial_state();
        s["channels"]["c51"]["fader"] = json!(0);
        s["monitor"]["source"] = json!("sur");
        s["monitor"]["level"] = json!(0);
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let m = matrix(&shape, &s);
        assert!(close(m[2][0], h) && close(m[4][0], h) && m[3][0] == 0.0);
        s["monitor"]["downmix"] = json!("lr");
        let m = matrix(&shape, &s);
        assert!(close(m[0][0], 1.0) && close(m[1][1], 1.0));
        assert!(m[2..].iter().all(|r| r[0] == 0.0 && r[1] == 0.0));
        // Mono sums the folded legs: LFE stays out, C is in both.
        s["monitor"]["downmix"] = json!("loRo");
        s["monitor"]["mono"] = json!(true);
        s["monitor"]["monoCompensation"] = json!(0);
        let m = matrix(&shape, &s);
        assert_eq!(m[3][0], 0.0, "LFE");
        assert!(close(m[2][0], 2.0 * h) && close(m[0][1], 1.0));
    }

    /// The matrix taken from per-channel and per-element factors is the
    /// one computed a coefficient at a time, bit for bit, over random
    /// states touching every leaf it reads: what the engine was sent
    /// before.
    #[test]
    fn the_factored_matrix_is_the_one_before_bit_for_bit() {
        let shape = shape();
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let channels = ["ch1", "ch2", "st1"];
        for _ in 0..400 {
            let mut s = shape.initial_state();
            for _ in 0..40 {
                let ch = channels[next(3) as usize];
                let db = json!(next(1200) as f64 / 10.0 - 100.0);
                let flag = json!(next(2) == 0);
                let (path, value) = match next(16) {
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
                    4 => (format!("/channels/{ch}/sends/aux1/on"), flag),
                    5 => (format!("/channels/{ch}/sends/aux1/level"), db),
                    6 => (format!("/channels/{ch}/sends/aux1/pre"), flag),
                    7 => (
                        format!("/channels/{ch}/directOut/tap"),
                        json!(["input", "preFader", "postFader"][next(3) as usize]),
                    ),
                    8 => (
                        "/vcas/vca1/members".into(),
                        json!(channels[..next(4) as usize]),
                    ),
                    9 => (
                        "/vcas/vca1/offset".into(),
                        json!(next(400) as f64 / 10.0 - 30.0),
                    ),
                    10 => ("/vcas/vca1/mute".into(), flag),
                    11 => (
                        format!(
                            "/{}/fader",
                            ["buses/pgm", "auxes/aux1", "mixMinus/mm1"][next(3) as usize]
                        ),
                        db,
                    ),
                    12 => (format!("/buses/pgm/polarity/{}", next(2)), flag),
                    13 => (
                        "/mixMinus/mm1/exclude".into(),
                        json!(channels[..next(4) as usize]),
                    ),
                    14 => (
                        format!("/afv/{ch}/{}", ["enabled", "open"][next(2) as usize]),
                        flag,
                    ),
                    _ => match next(8) {
                        0 => (
                            "/monitor/source".into(),
                            json!(["pgm", "cf", "aux1", "mm1"][next(4) as usize]),
                        ),
                        1 => ("/monitor/level".into(), db),
                        2 => ("/monitor/dim".into(), flag),
                        3 => ("/monitor/cut".into(), json!(next(8) == 0)),
                        4 => ("/monitor/mono".into(), flag),
                        5 => (
                            "/monitor/downmix".into(),
                            json!(["lr", "itu"][next(2) as usize]),
                        ),
                        6 => (format!("/monitor/speakerMute/{}", next(2)), flag),
                        _ => (format!("/monitor/speakerSolo/{}", next(2)), flag),
                    },
                };
                crate::state::set_at(&mut s, &path, value);
            }
            let bits = |m: Vec<Vec<f32>>| -> Vec<Vec<u32>> {
                m.into_iter()
                    .map(|r| r.into_iter().map(f32::to_bits).collect())
                    .collect()
            };
            assert_eq!(
                bits(matrix(&shape, &s)),
                bits(before::matrix(&shape, &s)),
                "{s}"
            );
        }
    }
}
