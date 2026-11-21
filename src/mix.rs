//! The live state as the engine's gain matrix: one row per input channel
//! (the legs of each channel in shape order, then those of the line-up
//! tone's generators, [`tone_widths`]) and one column per output channel (the legs of
//! each shape output in shape order), then per leg of each element the
//! engine meters on columns of its own ([`Shape::meter_columns`]).
//!
//! What a matrix coefficient can carry is applied: fader, mute, VCA
//! offsets and mutes, AFV open or closed, pan of a mono leg and the
//! fold-down of a wider channel, bus routes, sends
//! and their levels, bus, aux and mix-minus faders, mutes and polarity,
//! the mix-minus exclusions and talkback interrupts, and the monitor's
//! source or the channel and VCA PFL and AFL listens, downmix, level,
//! dim, talkback dim, cut, mono and speaker mutes and solos. Everything
//! else in the
//! state is the input's strip ([`crate::strip`]), a duck group
//! ([`crate::duck`]) or is kept and not applied. [`targets`] adds the
//! strips, the duck groups and their masks, and each input's flow.
//!
//! Every value is computed with `libm`, not the host's math library, so
//! replicas on other hosts derive the same bits from the same state.

use std::collections::BTreeSet;

use samplez::Targets;
use samplez_dsp::ToneParams;
use serde::Serialize;
use serde_json::Value;

use crate::meters::Layout;
use crate::shape::Shape;
use crate::state::tokens;
use crate::{duck, strip};

/// The start of the names of the engine inputs that play the line-up
/// tone (`/tone`, [`tone_input`]), after the channels' inputs: no channel
/// takes one, an id having no `/`.
pub const TONE_INPUT: &str = "/tone";
/// The line-up tone's frequency, Hz.
const TONE_HZ: u32 = 1000;

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

/// The channel mix-minus `id` carries in place of its mix while its
/// talkback interrupt is active, per model `Interrupt`; an interrupt with
/// no source changes nothing.
pub(crate) fn talkback<'a>(state: &'a Value, id: &str) -> Option<&'a str> {
    let interrupt = &state["mixMinus"][id]["interrupt"];
    on(&interrupt["active"])
        .then(|| interrupt["source"].as_str())
        .flatten()
}

/// Whether a mix-minus of `shape` carries its talkback, which dims the
/// monitor when `talkbackDim` is on.
fn talking(shape: &Shape, state: &Value) -> bool {
    (shape.mix_minus.iter()).any(|m| talkback(state, &m.id).is_some())
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

/// Whether any channel or VCA solos, and whether any of them is in PFL:
/// per model `MonitorState` the monitor then plays the listen sum, and
/// per `MonitorMeter` PFL takes priority over AFL, so while any PFL is
/// on only the PFL listens are summed. A VCA solos even without members,
/// which makes the listen sum silent.
pub(crate) fn solos(state: &Value) -> (bool, bool) {
    let all = ["channels", "vcas"]
        .into_iter()
        .filter_map(|g| state[g].as_object())
        .flat_map(|m| m.values())
        .map(|e| e["solo"].as_str().unwrap_or("off"));
    all.fold((false, false), |(any, pfl), mode| {
        (any || mode != "off", pfl || mode == "pfl")
    })
}

/// Whether channel `id` listens in PFL and in AFL: per model `VcaState`
/// a VCA's solo listens to every member as if the member had it set, so
/// its own solo and that of each VCA it is a member of add up.
fn listens(state: &Value, id: &str) -> (bool, bool) {
    let own = std::iter::once(&state["channels"][id]["solo"]);
    let vcas = (state["vcas"].as_object().into_iter())
        .flat_map(|v| v.values())
        .filter(|v| {
            v["members"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m == id))
        })
        .map(|v| &v["solo"]);
    own.chain(vcas).fold((false, false), |(pfl, afl), mode| {
        (pfl || mode == "pfl", afl || mode == "afl")
    })
}

/// The gain from leg `i` of a `from`-leg signal to leg `j` of a `to`-leg
/// element (model `Layout`: 1 mono, 2 stereo L R, 6 5.1 L R C LFE Ls Rs,
/// 8 7.1 L R C LFE Ls Rs Lrs Rrs), per model `PanState`: a mono signal
/// pans onto the first two legs at constant power by `pan` (-1 to 1); a
/// stereo one onto an element of two legs or more balances: `pan` lowers
/// the opposite leg linearly, to silence at the edge, and leaves the
/// other at unity, so the centre passes both legs; any other signal
/// ignores `pan`. A signal maps leg to leg by label onto an element as
/// wide or wider, and folds down onto a narrower one.
pub fn pan(from: usize, to: usize, i: usize, j: usize, pan: f64) -> f64 {
    match (from, to) {
        (1, 1) => 1.0,
        (2, 2..) => {
            let p = pan.clamp(-1.0, 1.0);
            let balance = if i == 0 { 1.0 - p } else { 1.0 + p };
            fold(from, to, i, j) * balance.min(1.0)
        }
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

/// Where a channel sits in the field its panner places it in (model
/// `PanState`): `lr` left to right, `frontRear` front to rear (`fr`
/// here), both -1 to 1, the front `spread` (`divergence` here) and the
/// `lfeSend` (`lfe`) in dB. The model's `fr`, `divergence` and `lfe` are
/// deprecated and never read.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    lr: f64,
    fr: f64,
    divergence: f64,
    lfe: f64,
}

impl Place {
    /// Centred at the front with full divergence, no LFE: the L/R pair a
    /// mono signal was on a surround element before the field, where a
    /// send, a direct out and a PFL listen put it.
    const FRONT: Place = Place {
        lr: 0.0,
        fr: -1.0,
        divergence: 1.0,
        lfe: -90.0,
    };

    /// A position on the left-right axis alone, at the front.
    fn lr(lr: f64) -> Place {
        Place { lr, ..Place::FRONT }
    }

    fn of(pan: &Value) -> Place {
        Place {
            lr: num(&pan["lr"]),
            fr: pan["frontRear"].as_f64().unwrap_or(Place::FRONT.fr),
            divergence: pan["spread"].as_f64().unwrap_or(Place::FRONT.divergence),
            lfe: pan["lfeSend"].as_f64().unwrap_or(Place::FRONT.lfe),
        }
    }
}

/// The gain of leg `j` of a 5.1 (L R C LFE Ls Rs) or 7.1 (L R C LFE Ls
/// Rs Lrs Rrs) element for a mono signal at `place`. The field is the
/// ITU-R BS.775 layout, L -30, C 0, R +30, Ls -110 and Rs +110 degrees
/// (7.1: sides at +-90 and rear surrounds at +-135), and every law is
/// constant power:
///
/// - Front: `lr` pans pairwise between adjacent speakers, L and C for
///   the left half, C and R for the right, so the centre of the field
///   is the C leg alone. `divergence` blends that law with a plain
///   constant-power pan between L and R, the phantom image: none of it
///   at 0, all of it at 1, a power split between.
/// - Rear: `lr` pans between the surrounds Ls and Rs; on 7.1 the side
///   pair and the rear pair take half the power each.
/// - `fr` crossfades the front into the rear, -1 the front alone, 1
///   the rear alone, and the middle half of each.
/// - The LFE leg takes the `lfe` send as an independent gain, silent at
///   -90 dB; it is no part of the pan.
fn field_gain(to: usize, j: usize, place: &Place) -> f64 {
    use std::f64::consts::FRAC_PI_2;
    let lr = place.lr.clamp(-1.0, 1.0);
    if j == 3 {
        return gain(place.lfe);
    }
    // (cos, sin) of the position along a pair, 0 at its first speaker.
    let pair = |at: f64| {
        let a = at.clamp(0.0, 1.0) * FRAC_PI_2;
        (libm::cos(a), libm::sin(a))
    };
    let front = {
        let (left, centre, right) = if lr <= 0.0 {
            let (a, b) = pair(lr + 1.0);
            (a, b, 0.0)
        } else {
            let (a, b) = pair(lr);
            (0.0, a, b)
        };
        let (keep, spread) = pair(place.divergence);
        let (phantom_l, phantom_r) = pair((lr + 1.0) / 2.0);
        match j {
            0 => libm::sqrt(keep * keep * left * left + spread * spread * phantom_l * phantom_l),
            1 => libm::sqrt(keep * keep * right * right + spread * spread * phantom_r * phantom_r),
            2 => keep * centre,
            _ => 0.0,
        }
    };
    let (rl, rr) = pair((lr + 1.0) / 2.0);
    let rear = match (j, to) {
        (4, 6) | (4, 8) => rl,
        (5, 6) | (5, 8) => rr,
        _ => 0.0,
    };
    let rear = if to == 8 {
        // The side pair and the rear pair, half the power each.
        match j {
            4 | 5 => rear / std::f64::consts::SQRT_2,
            6 => rl / std::f64::consts::SQRT_2,
            7 => rr / std::f64::consts::SQRT_2,
            _ => 0.0,
        }
    } else {
        rear
    };
    let (wf, wr) = pair((place.fr.clamp(-1.0, 1.0) + 1.0) / 2.0);
    wf * front + wr * rear
}

/// How a stereo channel or monitor uses its legs (model `StereoImage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Image {
    Normal,
    LeftToBoth,
    RightToBoth,
    Swap,
}

impl Image {
    fn of(v: &Value) -> Image {
        match v.as_str() {
            Some("leftToBoth") => Image::LeftToBoth,
            Some("rightToBoth") => Image::RightToBoth,
            Some("swap") => Image::Swap,
            _ => Image::Normal,
        }
    }

    /// The share of leg `k` of the signal that leg `i` of the imaged
    /// stereo signal carries.
    fn share(self, i: usize, k: usize) -> f64 {
        let on = match self {
            Image::Normal => i == k,
            Image::LeftToBoth => k == 0,
            Image::RightToBoth => k == 1,
            Image::Swap => i != k,
        };
        f64::from(u8::from(on))
    }
}

/// [`pan`] of a signal at `place`, a mono one onto a surround element by
/// [`field_gain`].
fn pan_place(from: usize, to: usize, i: usize, j: usize, place: &Place) -> f64 {
    if from == 1 && to >= 6 {
        field_gain(to, j, place)
    } else {
        pan(from, to, i, j, place.lr)
    }
}

/// [`pan_place`] of a stereo signal whose legs `image` has put to use
/// first.
fn pan_image(from: usize, to: usize, i: usize, j: usize, place: &Place, image: Image) -> f64 {
    if from == 2 && image != Image::Normal {
        (0..2)
            .map(|a| image.share(a, i) * pan_place(from, to, a, j, place))
            .sum()
    } else {
        pan_place(from, to, i, j, place)
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
/// `[input][column]`, the line-up tone generators' legs last. Each
/// channel's gain at each tap and each element's fader, mute and polarity
/// are taken once, then multiplied into every coefficient in the same
/// order of operations as one coefficient at a time, so the bits are
/// those.
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
    let mut m = vec![vec![0.0; width]; mix.rows + tone_widths(shape).iter().sum::<usize>()];
    let rows: Vec<usize> = (0..mix.rows).collect();
    let mut g = Vec::with_capacity(mix.rows * 8);
    let mut first = 0;
    let mut placed = Vec::with_capacity(columns.len());
    for (source, width) in columns {
        let ew = mix.element(source, &mut g);
        put(&mut m, &rows, first, width, ew, &g);
        placed.push((source, width, first));
        first += width;
    }
    put_tone(&mut m, mix.rows, shape, state, placed);
    m
}

/// The legs of each of the line-up tone's generators, one engine input
/// each ([`tone_input`]): one per distinct width of a bus, aux or
/// mix-minus, the elements `/tone/targets` names, narrowest first. A
/// target plays the generator as wide as it is, so its ident cuts its
/// own legs in turn.
pub fn tone_widths(shape: &Shape) -> Vec<usize> {
    let widths: BTreeSet<usize> = (shape.buses.iter().map(|e| e.width))
        .chain(shape.auxes.iter().map(|e| e.width))
        .chain(shape.mix_minus.iter().map(|e| e.width))
        .collect();
    widths.into_iter().collect()
}

/// The engine input of the line-up tone's generator `width` legs wide.
pub fn tone_input(width: usize) -> String {
    format!("{TONE_INPUT}/{width}")
}

/// The first of the tone rows, those of the generators of [`tone_widths`]
/// `widths` in order, that the generator `width` legs wide takes.
fn generator_row(widths: &[usize], width: usize) -> usize {
    widths.iter().take_while(|&&w| w < width).sum()
}

/// The line-up tone's generator: 1 kHz at `/tone/level`, the legs
/// identified with `/tone/ident`.
fn line_up(state: &Value) -> ToneParams {
    let t = &state["tone"];
    ToneParams {
        frequency_hz: TONE_HZ,
        level_db: num(&t["level"]) as f32,
        ident: on(&t["ident"]),
    }
}

/// Whether the line-up tone replaces the mix of element `id`: `/tone` is
/// on and its targets name it.
fn toned(state: &Value, id: &str) -> bool {
    let t = &state["tone"];
    on(&t["on"]) && (t["targets"].as_array()).is_some_and(|ids| ids.iter().any(|t| t == id))
}

/// Writes the line-up tones' rows, from `tone_row` on, of each of
/// `columns`, an element, its width and its first column. Leg `l` of the
/// generator as wide as an element the tone replaces ([`toned`]) is at
/// unity in the column's leg `l`, so an output narrower than its element
/// takes the tone's first legs; [`Mix::element`] gives such an element
/// no channel. The element's fader is not in these gains, so the tone is
/// after it; the engine's limiter on the output is after the matrix. A
/// monitor playing such an element plays its tone ([`Mix::tone`]).
fn put_tone<'c>(
    m: &mut [Vec<f32>],
    tone_row: usize,
    shape: &Shape,
    state: &Value,
    columns: impl IntoIterator<Item = (&'c str, usize, usize)>,
) {
    let widths = tone_widths(shape);
    let mix = Mix::of(shape, state, &[], |_| unreachable!("no channel"));
    let rows: Vec<usize> = (tone_row..m.len()).collect();
    let mut g = Vec::new();
    for (id, width, first) in columns {
        let ew = mix.tone(id, &widths, &mut g);
        if !toned(state, id) {
            put(m, &rows, first, width, ew, &g);
            continue;
        }
        let generator = generator_row(&widths, ew);
        for (row, r) in m[tone_row..].iter_mut().enumerate() {
            for leg in 0..width {
                r[first + leg] = f32::from(u8::from(row == generator + leg));
            }
        }
    }
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
/// rows into rows `rows` of `m`. An output as wide as the element takes
/// its legs; a narrower one folds them down as [`pan`] does a channel,
/// the sum of each leg's gain times its fold coefficient; legs the
/// element lacks are 0.
fn put(m: &mut [Vec<f32>], rows: &[usize], first: usize, width: usize, ew: usize, g: &[f64]) {
    for (i, &row) in rows.iter().enumerate() {
        for leg in 0..width {
            let v = if width < ew {
                (0..ew)
                    .map(|k| g[i * ew + k] * pan(ew, width, k, leg, 0.0))
                    .sum()
            } else if leg < ew {
                g[i * ew + leg]
            } else {
                0.0
            };
            m[row][first + leg] = v as f32;
        }
    }
}

/// One channel's input as the MXL configuration wires it: the flow of
/// input a, the flow of input b when the channel has one, and the domain
/// both are in. A tone channel's names no flow and no domain, both empty:
/// it reads none ([`selected`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wiring {
    pub a: String,
    pub b: Option<String>,
    pub domain: String,
}

/// The flow each channel's selected input reads: input b's when the
/// state selects b and the channel has one, else input a's; none for a
/// tone channel, which reads none.
pub fn selected(shape: &Shape, wiring: &[Wiring], state: &Value) -> Vec<Option<String>> {
    (shape.channels.iter())
        .zip(wiring)
        .map(|(c, w)| (c.tone.is_none()).then(|| flow(state, &c.id, w).to_owned()))
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
/// ([`matrix`]), each channel's strip ([`strip::params`]) and input
/// delay ([`strip::delay`]), each tone channel's generator ([`strip::tone`]), each output's strip ([`strip::output_strips`]), delay ([`strip::output_delays`]) and limiter ([`strip::limiters`]), the duck groups ([`duck::targets`])
/// and masks ([`duck::masks`]) and the flow its selected input reads
/// ([`selected`]), one slot per channel; then a slot per line-up tone
/// generator ([`tone_widths`]), which reads no flow, has no strip and
/// plays [`line_up`]. Every output is enabled.
pub fn targets(shape: &Shape, wiring: &[Wiring], state: &Value) -> Targets {
    Derived::new(shape, wiring, state).targets
}

/// Sets each loudness measurement's run flag and reset epoch in `t` from
/// `/loudness/<id>/running` and `/loudness/<id>/resets` in `state`, by
/// the engine meter that measures it ([`Layout::loudness_meter`]): the
/// epoch is the reset count, wrapping, so a reset counted moves it
/// forward and the engine restarts the measurement where it applies.
/// Every meter up to the last one measuring a bus, aux or mix-minus
/// output is listed, one measuring none running at epoch 0.
pub fn loudness(shape: &Shape, state: &Value, t: &mut Targets) {
    let layout = Layout::of(shape);
    let ids = (shape.buses.iter().map(|b| &b.id))
        .chain(shape.auxes.iter().map(|a| &a.id))
        .chain(shape.mix_minus.iter().map(|m| &m.id));
    let (mut running, mut resets) = (Vec::new(), Vec::new());
    for id in ids {
        let Some(meter) = layout.loudness_meter(id) else {
            continue;
        };
        if running.len() <= meter {
            running.resize(meter + 1, true);
            resets.resize(meter + 1, 0);
        }
        let m = &state["loudness"][id];
        running[meter] = m["running"].as_bool().unwrap_or(true);
        resets[meter] = m["resets"].as_u64().unwrap_or(0) as u32;
    }
    (t.loudness_running, t.loudness_resets) = (running, resets);
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
    /// [`targets`] of `state` read through this view, with the loudness
    /// measurements' run flags and reset epochs ([`loudness`]).
    pub fn targets(&self, shape: &Shape, state: &Value) -> Targets {
        let mut t = targets(shape, &self.wiring, state);
        self.overlay(&mut t);
        loudness(shape, state, &mut t);
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
    /// The line-up tone generators' first row, after every channel's.
    tone_row: usize,
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
    /// The strips, delays, tone generators and flows of these slots.
    strips: BTreeSet<usize>,
    delays: BTreeSet<usize>,
    tones: BTreeSet<usize>,
    flows: BTreeSet<usize>,
    /// The line-up tone's generators ([`line_up`]).
    line_up: bool,
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
        let generators = tone_widths(shape);
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
                flows: (selected(shape, wiring, state).into_iter())
                    .chain(generators.iter().map(|_| None))
                    .collect(),
                strips: (shape.channels.iter())
                    .map(|c| strip::params(&state["channels"][&c.id], c.width))
                    .chain(generators.iter().map(|_| Vec::new()))
                    .collect(),
                delays: (shape.channels.iter())
                    .map(|c| strip::delay(&state["channels"][&c.id]))
                    .collect(),
                tones: (shape.channels.iter())
                    .map(|c| (c.tone.is_some()).then(|| strip::tone(&state["channels"][&c.id])))
                    .chain(generators.iter().map(|_| Some(line_up(state))))
                    .collect(),
                output_strips: strip::output_strips(shape, state),
                output_delays: strip::output_delays(shape, state),
                limiters: strip::limiters(shape, state),
                ducks: duck::targets(shape, state),
                masks: duck::masks(shape, state),
                ..Targets::default()
            },
            factors,
            first,
            columns,
            tone_row: row,
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
        // The rows and columns written above hold the mix in place of the
        // tone again.
        let placed = (self.columns.iter()).map(|(id, width, first)| (id.as_str(), *width, *first));
        put_tone(&mut self.targets.gains, self.tone_row, shape, state, placed);
        for slot in moved.strips {
            let c = &shape.channels[slot];
            self.targets.strips[slot] = strip::params(&state["channels"][&c.id], c.width);
        }
        for slot in moved.delays {
            self.targets.delays[slot] = strip::delay(&state["channels"][&shape.channels[slot].id]);
        }
        for slot in moved.tones {
            let c = &shape.channels[slot];
            self.targets.tones[slot] =
                (c.tone.is_some()).then(|| strip::tone(&state["channels"][&c.id]));
        }
        for slot in moved.flows {
            let c = &shape.channels[slot];
            self.targets.flows[slot] =
                (c.tone.is_none()).then(|| flow(state, &c.id, &wiring[slot]).to_owned());
        }
        if moved.line_up {
            for tone in &mut self.targets.tones[shape.channels.len()..] {
                *tone = Some(line_up(state));
            }
        }
        // A bus, aux or mix-minus write may move an output's strip, delay
        // or limiter: one per output, each read in full.
        if !moved.elements.is_empty() {
            self.targets.output_strips = strip::output_strips(shape, state);
            self.targets.output_delays = strip::output_delays(shape, state);
            self.targets.limiters = strip::limiters(shape, state);
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
                        Some(
                            "fader" | "mute" | "routes" | "sends" | "pan" | "image" | "directOut",
                        ) => {
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
                        Some("delay") => {
                            moved.delays.insert(s);
                        }
                        Some("tone") => {
                            moved.tones.insert(s);
                        }
                        // Whether anything solos, and in which mode, moves
                        // every row of the monitor ([`Mix::monitor`]) and
                        // its duck masks.
                        Some("solo") => {
                            moved.monitor = true;
                            moved.masks = true;
                        }
                        // A write of the whole channel may carry a solo.
                        None => {
                            moved.rows.insert(s);
                            moved.strips.insert(s);
                            moved.delays.insert(s);
                            moved.tones.insert(s);
                            moved.flows.insert(s);
                            moved.monitor = true;
                            moved.masks = true;
                        }
                        _ => {
                            moved.rows.insert(s);
                            moved.strips.insert(s);
                            moved.delays.insert(s);
                            moved.tones.insert(s);
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
                // A VCA moves its members, those it had and those it has,
                // and its solo every row of the monitor and its masks.
                (Some("vcas"), _) => {
                    let solo = matches!(key, None | Some("solo"));
                    moved.monitor |= solo;
                    moved.masks |= solo;
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
                    // An interrupt dims the monitor while talkbackDim is on.
                    let interrupt =
                        group == Some("mixMinus") && matches!(key, None | Some("interrupt"));
                    moved.monitor |= interrupt && on(&state["monitor"]["talkbackDim"]);
                    // And lifts the masks of the mix-minus while it lasts.
                    moved.masks |= interrupt;
                }
                (Some("monitor"), _) => {
                    moved.monitor = true;
                    moved.masks |= matches!(id, None | Some("source"));
                }
                // A group, or a group's triggers, targets or destinations.
                (Some("ducks"), _) => {
                    moved.masks |= t.len() < 4 || duck::routing_of(path).is_some();
                }
                // The line-up tone's level and ident move its generator
                // alone; whether it is on and what it replaces move
                // everything, as any other leaf does (below).
                (Some("tone"), Some("level" | "ident")) => moved.line_up = true,
                (
                    Some(
                        "tally" | "loudness" | "recallSafe" | "scenes" | "consoles" | "metering"
                        | "sceneRecalled",
                    ),
                    _,
                ) => {}
                _ => moved.all = true,
            }
        }
        moved
    }
}

/// A channel's gain after its strip, the same on every leg.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterTap {
    /// At its `meterTap`.
    pub meter: f64,
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
    /// Its pan.
    place: Place,
    /// Its stereo image.
    image: Image,
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
    /// Its bus, the channels it excludes and the channel its talkback
    /// interrupt carries in their place ([`talkback`]).
    MixMinus(&'a str, &'a Value, Option<&'a str>),
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
                    place: Place::of(&s["pan"]),
                    image: Image::of(&s["image"]),
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
            Kind::MixMinus(
                &m.bus,
                &self.state["mixMinus"][id]["exclude"],
                talkback(self.state, id),
            )
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
        // The line-up tone in place of its mix: no channel ([`put_tone`]).
        if toned(self.state, id) {
            return width;
        }
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
            // A direct out is the channel itself, whatever its image.
            let image = if matches!(kind, Kind::Channel) {
                Image::Normal
            } else {
                c.image
            };
            for i in 0..c.width {
                let row = &mut g[(c.row + i) * width..][..width];
                for (j, out) in row.iter_mut().enumerate() {
                    *out = at * pan_image(c.width, width, i, j, &p, image);
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

    /// The monitor output: while any channel or VCA solos the listen sum
    /// ([`solos`]), else its source; that signal's legs onto the
    /// monitor's, folded down by `downmix` when it is wider, then level,
    /// dim (also while a mix-minus carries its talkback and `talkbackDim`
    /// is on), cut, mono (the sum of the folded legs) and the speaker
    /// mutes and solos.
    ///
    /// The listen sum takes each listening channel's legs at their own
    /// width. PFL is the channel after its strip, before fader, mute and
    /// pan: [`Tap::PreFader`], centred, so a mono channel plays on every
    /// leg and a stereo one leg to leg, as its image sets them. AFL is [`Tap::PostFader`], after
    /// fader, mute and VCAs and before AFV, at the channel's pan.
    fn monitor(&self, width: usize, g: &mut [f64]) {
        let play = self.monitor_play(width);
        let (soloing, pfl) = solos(self.state);
        if !soloing {
            // A source the line-up tone replaces gives every row 0
            // ([`Mix::element`]); [`Mix::tone`] plays the tone.
            let source = self.state["monitor"]["source"].as_str().unwrap_or_default();
            let mut src = Vec::new();
            let from = self.element(source, &mut src);
            for row in 0..self.rows {
                play(
                    &src[row * from..][..from],
                    from,
                    &Place::FRONT,
                    Image::Normal,
                    &mut g[row * width..][..width],
                );
            }
            return;
        }
        let mut legs = Vec::new();
        for c in &self.chans {
            let (p, a) = listens(self.state, c.id);
            let heard = if pfl {
                p.then_some((Tap::PreFader, Place::FRONT))
            } else {
                a.then_some((Tap::PostFader, c.place))
            };
            let Some((tap, p)) = heard else {
                continue;
            };
            for i in 0..c.width {
                legs.clear();
                legs.resize(c.width, 0.0);
                legs[i] = c.at(tap);
                let row = c.row + i;
                play(&legs, c.width, &p, c.image, &mut g[row * width..][..width]);
            }
        }
    }

    /// The gains of the line-up tones' rows into element `id`, the
    /// generators of [`tone_widths`] `widths` in order, `[tone row * width
    /// + leg]` as [`Mix::element`] gives the channels'; returns its width.
    /// A monitor whose source the tone replaces ([`toned`]) plays that
    /// source's generator, as wide as the source, as it plays the source,
    /// while nothing solos; any other element has none. The columns of an
    /// element the tone replaces are [`put_tone`]'s.
    fn tone(&self, id: &str, widths: &[usize], g: &mut Vec<f64>) -> usize {
        let width = width_of(self.shape, id);
        g.clear();
        g.resize(widths.iter().sum::<usize>() * width, 0.0);
        let source = self.state["monitor"]["source"].as_str().unwrap_or_default();
        let monitor = self.shape.monitor_outputs.iter().any(|m| m.id == id);
        if !monitor || solos(self.state).0 || !toned(self.state, source) {
            return width;
        }
        let from = width_of(self.shape, source);
        let first = generator_row(widths, from);
        let play = self.monitor_play(width);
        let mut legs = vec![0.0; from];
        for leg in 0..from {
            legs.fill(0.0);
            legs[leg] = 1.0;
            let row = first + leg;
            play(
                &legs,
                from,
                &Place::FRONT,
                Image::Normal,
                &mut g[row * width..][..width],
            );
        }
        width
    }

    /// The monitor output `width` legs wide as it plays a signal: writes
    /// into `out` its legs for the `from` legs `legs` of a signal panned
    /// by `p` with stereo image `image` ([`Mix::monitor`]).
    fn monitor_play(&self, width: usize) -> impl Fn(&[f64], usize, &Place, Image, &mut [f64]) {
        let m = &self.state["monitor"];
        let mut level = gain(num(&m["level"]));
        if on(&m["dim"]) || on(&m["talkbackDim"]) && talking(self.shape, self.state) {
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
        // The `from` legs of a signal panned by `p` onto monitor leg `j`,
        // folded by the downmix: lr plays only its L and R legs.
        let folded = move |legs: &[f64], from: usize, p: &Place, image: Image, j: usize| -> f64 {
            if lr && from > width {
                legs.get(j).copied().filter(|_| j < 2).unwrap_or(0.0)
            } else {
                (0..from)
                    .map(|i| legs[i] * pan_image(from, width, i, j, p, image))
                    .sum()
            }
        };
        // The monitor's image takes its stereo legs after the fold.
        let monitor_image = if width == 2 {
            Image::of(&m["image"])
        } else {
            Image::Normal
        };
        let shaped = move |legs: &[f64], from: usize, p: &Place, image: Image, j: usize| -> f64 {
            if monitor_image == Image::Normal {
                return folded(legs, from, p, image, j);
            }
            (0..width)
                .map(|k| monitor_image.share(j, k) * folded(legs, from, p, image, k))
                .sum()
        };
        let speaker_solo = m["speakerSolo"]
            .as_array()
            .is_some_and(|s| s.iter().any(on));
        let leg_gain: Vec<f64> = (0..width)
            .map(|j| {
                let audible =
                    !on(&m["speakerMute"][j]) && (!speaker_solo || on(&m["speakerSolo"][j]));
                if audible { level } else { 0.0 }
            })
            .collect();
        move |legs: &[f64], from: usize, p: &Place, image: Image, out: &mut [f64]| {
            for (j, out) in out.iter_mut().enumerate() {
                let v: f64 = if mono {
                    (0..width).map(|k| shaped(legs, from, p, image, k)).sum()
                } else {
                    shaped(legs, from, p, image, j)
                };
                *out = v * leg_gain[j];
            }
        }
    }
}

/// How channel `c` feeds element `id` of `kind`: tap, level and pan, or
/// `None` when it does not.
fn feed(c: &Chan<'_>, kind: &Kind<'_>, id: &str) -> Option<(Tap, f64, Place)> {
    match kind {
        Kind::Channel if c.id == id => {
            let tap = match c.state["directOut"]["tap"].as_str() {
                Some("input") => Tap::Input,
                Some("postFader") => Tap::PostFader,
                _ => Tap::PreFader,
            };
            Some((tap, 1.0, Place::FRONT))
        }
        Kind::Bus => on(&c.routes[id]).then_some((Tap::Mix, 1.0, c.place)),
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
            Some((tap, gain(num(&send["level"])), Place::lr(num(&send["pan"]))))
        }
        // The talkback alone, after its strip and before its fader.
        Kind::MixMinus(_, _, Some(talk)) => {
            (c.id == *talk).then_some((Tap::PreFader, 1.0, c.place))
        }
        Kind::MixMinus(bus, exclude, None) => {
            let excluded = exclude
                .as_array()
                .is_some_and(|e| e.iter().any(|e| e == c.id));
            (on(&c.routes[*bus]) && !excluded).then_some((Tap::Mix, 1.0, c.place))
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
        use super::super::{
            Image, Place, Tap, channel, gain, listens, num, on, pan_image, solos, talkback,
            talking, width_of,
        };
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
                    let image = if c.id == id {
                        Image::Normal
                    } else {
                        Image::of(&state["channels"][&c.id]["image"])
                    };
                    for i in 0..c.width {
                        let at = channel(state, &c.id, tap) * level;
                        for (j, out) in g[row + i].iter_mut().enumerate() {
                            *out = at * pan_image(c.width, width, i, j, &p, image);
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
        fn feed(
            shape: &Shape,
            state: &Value,
            channel: &str,
            id: &str,
        ) -> Option<(Tap, f64, Place)> {
            let c = &state["channels"][channel];
            let pan = Place::of(&c["pan"]);
            if channel == id {
                let tap = match c["directOut"]["tap"].as_str() {
                    Some("input") => Tap::Input,
                    Some("postFader") => Tap::PostFader,
                    _ => Tap::PreFader,
                };
                return Some((tap, 1.0, Place::FRONT));
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
                return on(&s["on"]).then_some((
                    tap,
                    gain(num(&s["level"])),
                    Place::lr(num(&s["pan"])),
                ));
            }
            let m = shape.mix_minus.iter().find(|m| m.id == id)?;
            if let Some(talk) = talkback(state, id) {
                return (channel == talk).then_some((Tap::PreFader, 1.0, pan));
            }
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

        /// The monitor output: while anything solos the listen sum, else
        /// its source; that signal's legs onto the monitor's, folded down
        /// by `downmix` when it is wider, then level, dim, cut, mono (the
        /// sum of the folded legs) and the speaker mutes and solos.
        fn monitor(shape: &Shape, state: &Value, width: usize) -> Vec<Vec<f64>> {
            let m = &state["monitor"];
            let mut level = gain(num(&m["level"]));
            if on(&m["dim"]) || on(&m["talkbackDim"]) && talking(shape, state) {
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
            // The `from` legs of a signal panned by `p` onto monitor leg
            // `j`, folded by the downmix: lr plays only its L and R legs.
            let folded = |legs: &[f64], from: usize, p: &Place, image: Image, j: usize| -> f64 {
                if lr && from > width {
                    legs.get(j).copied().filter(|_| j < 2).unwrap_or(0.0)
                } else {
                    (0..from)
                        .map(|i| legs[i] * pan_image(from, width, i, j, p, image))
                        .sum()
                }
            };
            let monitor_image = if width == 2 {
                Image::of(&m["image"])
            } else {
                Image::Normal
            };
            let shaped = |legs: &[f64], from: usize, p: &Place, image: Image, j: usize| -> f64 {
                if monitor_image == Image::Normal {
                    return folded(legs, from, p, image, j);
                }
                (0..width)
                    .map(|k| monitor_image.share(j, k) * folded(legs, from, p, image, k))
                    .sum()
            };
            let leg_gain = |j: usize| {
                let solo = m["speakerSolo"]
                    .as_array()
                    .is_some_and(|s| s.iter().any(on));
                let audible = !on(&m["speakerMute"][j]) && (!solo || on(&m["speakerSolo"][j]));
                if audible { level } else { 0.0 }
            };
            let play = |legs: &[f64], from: usize, p: &Place, image: Image| -> Vec<f64> {
                (0..width)
                    .map(|j| {
                        let v: f64 = if mono {
                            (0..width).map(|k| shaped(legs, from, p, image, k)).sum()
                        } else {
                            shaped(legs, from, p, image, j)
                        };
                        v * leg_gain(j)
                    })
                    .collect()
            };
            let (soloing, pfl) = solos(state);
            if !soloing {
                let source = m["source"].as_str().unwrap_or_default();
                let src = element(shape, state, source);
                let from = width_of(shape, source);
                return src
                    .iter()
                    .map(|legs| play(legs, from, &Place::FRONT, Image::Normal))
                    .collect();
            }
            let mut g = Vec::new();
            for c in &shape.channels {
                let (p, a) = listens(state, &c.id);
                let heard = if pfl {
                    p.then_some((Tap::PreFader, Place::FRONT))
                } else {
                    a.then(|| (Tap::PostFader, Place::of(&state["channels"][&c.id]["pan"])))
                };
                for i in 0..c.width {
                    let row = match heard {
                        Some((tap, p)) => {
                            let mut legs = vec![0.0; c.width];
                            legs[i] = channel(state, &c.id, tap);
                            play(
                                &legs,
                                c.width,
                                &p,
                                Image::of(&state["channels"][&c.id]["image"]),
                            )
                        }
                        None => vec![0.0; width],
                    };
                    g.push(row);
                }
            }
            g
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

    // Rows: ch1, ch2, st1 L, st1 R, then the line-up tone's mono and
    // stereo generators' legs. Columns: o-pgm L R, o-aux, o-mm L R,
    // o-mon L R, o-ch1, then cf L R.

    /// A fresh state is silent everywhere but the direct out, which taps
    /// before the closed faders.
    #[test]
    fn the_initial_state_passes_only_the_direct_out() {
        let shape = shape();
        let m = matrix(&shape, &shape.initial_state());
        assert_eq!(m.len(), 7);
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
        s["channels"]["st1"]["fader"] = json!(0);
        s["channels"]["st1"]["mute"] = json!(true);
        let t = meter_taps(&shape, &s);
        let tap = |meter| MeterTap { meter };
        assert_eq!(t, [tap(1.0), tap(gain(-6.0)), tap(0.0)]);
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

    /// Model `Interrupt`: while active, the mix-minus output carries its
    /// source channel alone, after its strip and before its fader, panned
    /// as the channel is, at the mix-minus fader and mute.
    #[test]
    fn an_interrupt_replaces_the_mix_minus_with_its_source_before_the_fader() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch2"]["fader"] = json!(0);
        s["channels"]["st1"]["fader"] = json!(0);
        s["mixMinus"]["mm1"]["interrupt"] = json!({ "source": "ch1", "active": true });
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let m = matrix(&shape, &s);
        assert!(close(m[0][3], h) && close(m[0][4], h), "{m:?}");
        assert!(
            (1..4).all(|row| close(m[row][3], 0.0) && close(m[row][4], 0.0)),
            "{m:?}"
        );
        // The programme and the monitor still carry the mix.
        assert!(close(m[1][0], h) && close(m[0][0], 0.0), "{m:?}");
        s["channels"]["ch1"]["pan"]["lr"] = json!(-1);
        s["mixMinus"]["mm1"]["fader"] = json!(-6);
        let m = matrix(&shape, &s);
        assert!(close(m[0][3], gain(-6.0)) && close(m[0][4], 0.0), "{m:?}");
        s["mixMinus"]["mm1"]["mute"] = json!(true);
        assert!(
            matrix(&shape, &s)
                .iter()
                .all(|r| close(r[3], 0.0) && close(r[4], 0.0))
        );
    }

    /// A wider source, or a narrower mix-minus, folds as the mix path
    /// folds it: with the source alone open at 0 dB, the interrupted
    /// column is the one the mix carries.
    #[test]
    fn an_interrupt_folds_its_source_as_the_mix_does() {
        for (source, width, layout) in
            [("st1", 2, "stereo"), ("st1", 1, "mono"), ("ch1", 1, "mono")]
        {
            let mut s = small();
            s["mixMinus"][0]["width"] = json!(width);
            s["mixMinus"][0]["layout"] = json!(layout);
            s["outputs"] = json!([
                { "id": "o-mm", "name": "N-1", "source": "mm1", "width": width, "layout": layout },
            ]);
            let shape = Shape::parse(&s).unwrap();
            let mut state = shape.initial_state();
            state["channels"][source]["fader"] = json!(0);
            let mix = matrix(&shape, &state);
            state["mixMinus"]["mm1"]["interrupt"] = json!({ "source": source, "active": true });
            let talk = matrix(&shape, &state);
            // The output's own columns; the monitor, dimmed while
            // talking, follows later.
            let own = |m: &[Vec<f32>]| -> Vec<Vec<f32>> {
                m.iter().map(|r| r[..width].to_vec()).collect()
            };
            assert_eq!(own(&talk), own(&mix), "{source} onto {layout}");
            assert!(
                own(&talk).iter().flatten().any(|&g| g > 0.1),
                "{source}: {talk:?}"
            );
        }
        // Stereo onto stereo: each leg onto its own.
        let shape = shape();
        let mut s = shape.initial_state();
        s["mixMinus"]["mm1"]["interrupt"] = json!({ "source": "st1", "active": true });
        let m = matrix(&shape, &s);
        assert!(close(m[2][3], 1.0) && close(m[3][4], 1.0), "{m:?}");
        assert!(close(m[2][4], 0.0) && close(m[3][3], 0.0), "{m:?}");
        assert!(close(m[0][3], 0.0) && close(m[1][3], 0.0), "{m:?}");
    }

    /// A write to an interrupt, or to the whole mix-minus, moves the
    /// monitor while talkbackDim is on, and not with it off; the leaf
    /// writes the contract takes and the objects above them alike.
    #[test]
    fn an_interrupt_write_moves_the_monitor_only_with_talkback_dim() {
        let shape = shape();
        let mut s = shape.initial_state();
        for path in [
            "/mixMinus/mm1",
            "/mixMinus/mm1/interrupt",
            "/mixMinus/mm1/interrupt/source",
            "/mixMinus/mm1/interrupt/active",
        ] {
            let of = |s: &Value| Moved::of(&shape, s, &Value::Null, [path]);
            assert!(of(&s).monitor, "{path}");
            assert!(of(&s).elements.contains("mm1"), "{path}");
            s["monitor"]["talkbackDim"] = json!(false);
            assert!(!of(&s).monitor, "{path} with talkbackDim off");
            s["monitor"]["talkbackDim"] = json!(true);
        }
        assert!(!Moved::of(&shape, &s, &Value::Null, ["/mixMinus/mm1/fader"]).monitor);
        // The masks follow an interrupt, whatever talkbackDim says.
        for path in ["/mixMinus/mm1", "/mixMinus/mm1/interrupt/active"] {
            assert!(Moved::of(&shape, &s, &Value::Null, [path]).masks, "{path}");
        }
        assert!(!Moved::of(&shape, &s, &Value::Null, ["/mixMinus/mm1/fader"]).masks);
    }

    /// A closed or muted fader does not silence the talkback: the
    /// interrupt taps before the fader.
    #[test]
    fn an_interrupt_carries_its_source_with_the_fader_closed() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["mute"] = json!(true);
        s["mixMinus"]["mm1"]["interrupt"] = json!({ "source": "ch1", "active": true });
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let m = matrix(&shape, &s);
        assert!(close(m[0][3], h) && close(m[0][4], h), "{m:?}");
    }

    /// Released, or active with no source, the mix-minus carries its mix
    /// again, bit for bit.
    #[test]
    fn a_released_interrupt_restores_the_mix() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(-6);
        s["channels"]["ch2"]["fader"] = json!(0);
        s["mixMinus"]["mm1"]["exclude"] = json!(["ch2"]);
        let mix = matrix(&shape, &s);
        assert!(close(
            mix[0][3],
            gain(-6.0) * std::f64::consts::FRAC_1_SQRT_2
        ));
        s["mixMinus"]["mm1"]["interrupt"] = json!({ "source": "ch2", "active": true });
        let talk = matrix(&shape, &s);
        assert!(close(talk[0][3], 0.0) && talk[1][3] > 0.5, "{talk:?}");
        s["mixMinus"]["mm1"]["interrupt"]["active"] = json!(false);
        assert_eq!(matrix(&shape, &s), mix);
        s["mixMinus"]["mm1"]["interrupt"] = json!({ "source": null, "active": true });
        assert_eq!(matrix(&shape, &s), mix);
    }

    /// Model `MonitorState.talkbackDim`: the monitor dims by its dim depth
    /// while a mix-minus carries its talkback, once with dim on as well,
    /// and not with talkbackDim off.
    #[test]
    fn talkback_dims_the_monitor_while_an_interrupt_is_active() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        s["monitor"]["level"] = json!(0);
        assert_eq!(s["monitor"]["talkbackDim"], json!(true));
        assert!(close(matrix(&shape, &s)[2][5], 1.0));
        s["mixMinus"]["mm1"]["interrupt"]["active"] = json!(true);
        assert!(
            close(matrix(&shape, &s)[2][5], 1.0),
            "no source, no talkback"
        );
        s["mixMinus"]["mm1"]["interrupt"]["source"] = json!("ch1");
        assert!(close(matrix(&shape, &s)[2][5], gain(-20.0)));
        s["monitor"]["dim"] = json!(true);
        assert!(close(matrix(&shape, &s)[2][5], gain(-20.0)));
        s["monitor"]["dim"] = json!(false);
        s["monitor"]["talkbackDim"] = json!(false);
        assert!(close(matrix(&shape, &s)[2][5], 1.0));
        s["monitor"]["talkbackDim"] = json!(true);
        s["mixMinus"]["mm1"]["interrupt"]["active"] = json!(false);
        assert!(close(matrix(&shape, &s)[2][5], 1.0));
    }

    /// A listen and a talkback together: PFL on the monitor is dimmed by
    /// the talkback dim once, also with dim on, and the monitor on an
    /// interrupted mix-minus plays the talkback, dimmed.
    #[test]
    fn a_listen_and_a_talkback_share_one_dim_on_the_monitor() {
        let shape = shape();
        let mut s = shape.initial_state();
        let h = std::f64::consts::FRAC_1_SQRT_2;
        s["monitor"]["level"] = json!(0);
        s["channels"]["ch1"]["solo"] = json!("pfl");
        s["mixMinus"]["mm1"]["interrupt"] = json!({ "source": "ch2", "active": true });
        // PFL on ch1 while ch2 talks: dimmed, ch2 not heard.
        let m = matrix(&shape, &s);
        assert!(close(m[0][5], h * gain(-20.0)), "{m:?}");
        assert!(close(m[1][5], 0.0), "{m:?}");
        s["monitor"]["dim"] = json!(true);
        assert!(
            close(matrix(&shape, &s)[0][5], h * gain(-20.0)),
            "dimmed once"
        );
        s["monitor"]["dim"] = json!(false);
        // Released: the PFL plays at full level again.
        s["mixMinus"]["mm1"]["interrupt"]["active"] = json!(false);
        assert!(close(matrix(&shape, &s)[0][5], h));
        // No solo, the monitor on the interrupted mix-minus: the talkback.
        s["channels"]["ch1"]["solo"] = json!("off");
        s["monitor"]["source"] = json!("mm1");
        s["mixMinus"]["mm1"]["interrupt"]["active"] = json!(true);
        let m = matrix(&shape, &s);
        assert!(close(m[1][5], h * gain(-20.0)), "{m:?}");
        assert!(close(m[0][5], 0.0), "{m:?}");
    }

    /// A stereo channel's pan balances it: the opposite leg lowers and
    /// the near one stays, on a stereo bus; a mono channel's pan still
    /// places it at constant power.
    #[test]
    fn a_stereo_channels_pan_balances_its_legs() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        s["channels"]["st1"]["pan"]["lr"] = json!(-0.5);
        // Rows: ch1, ch2, st1 L, st1 R; columns o-pgm L R.
        let m = matrix(&shape, &s);
        assert!(close(m[2][0], 1.0) && close(m[2][1], 0.0), "{m:?}");
        assert!(close(m[3][1], 0.5) && close(m[3][0], 0.0), "{m:?}");
        s["channels"]["st1"]["pan"]["lr"] = json!(0.5);
        let m = matrix(&shape, &s);
        assert!(close(m[2][0], 0.5) && close(m[3][1], 1.0), "{m:?}");
    }

    /// A stereo channel's image feeds one leg to both, or swaps them,
    /// before the balance, on every bus, aux and mix-minus it feeds, and
    /// not on its direct out.
    #[test]
    fn a_stereo_images_use_of_its_legs_reaches_what_it_feeds() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        let at = |s: &Value| matrix(&shape, s);
        // (image, [L->L, L->R, R->L, R->R]) onto the programme.
        for (image, want) in [
            ("normal", [1.0, 0.0, 0.0, 1.0]),
            ("leftToBoth", [1.0, 1.0, 0.0, 0.0]),
            ("rightToBoth", [0.0, 0.0, 1.0, 1.0]),
            ("swap", [0.0, 1.0, 1.0, 0.0]),
        ] {
            s["channels"]["st1"]["image"] = json!(image);
            let m = at(&s);
            let got = [m[2][0], m[2][1], m[3][0], m[3][1]];
            assert!(
                got.iter().zip(want).all(|(g, w)| close(*g, w)),
                "{image}: {m:?}"
            );
        }
        // The image comes before the balance: the left leg to both, the
        // right one lowered by a pan of -1 on the way.
        s["channels"]["st1"]["image"] = json!("leftToBoth");
        s["channels"]["st1"]["pan"]["lr"] = json!(-1);
        let m = at(&s);
        assert!(close(m[2][0], 1.0) && close(m[2][1], 0.0), "{m:?}");
        // The mix-minus follows the bus, imaged alike.
        s["channels"]["st1"]["pan"]["lr"] = json!(0);
        s["channels"]["st1"]["image"] = json!("swap");
        let m = at(&s);
        assert!(close(m[2][3], 0.0) && close(m[2][4], 1.0), "{m:?}");
    }

    /// The image reaches an aux send and the AFL listen as it reaches a
    /// bus, with the send's and the channel's pan, and a stereo direct
    /// out stays as the channel is.
    #[test]
    fn the_image_reaches_sends_and_listens_but_not_the_direct_out() {
        let mut v = small();
        v["outputs"] = json!([
            { "id": "o-aux", "name": "IFB", "source": "aux1", "width": 1, "layout": "mono" },
            { "id": "o-mon", "name": "Mon", "source": "mon", "width": 2, "layout": "stereo" },
            { "id": "o-st1", "name": "DO", "source": "st1", "width": 2, "layout": "stereo" },
        ]);
        let shape = Shape::parse(&v).unwrap();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        s["channels"]["st1"]["image"] = json!("leftToBoth");
        s["channels"]["st1"]["sends"]["aux1"] =
            json!({ "on": true, "level": 0, "pan": 0, "pre": false });
        s["monitor"]["level"] = json!(0);
        // Rows: ch1, ch2, st1 L, st1 R. Columns: o-aux, o-mon L R, o-st1 L R.
        let m = matrix(&shape, &s);
        // Both legs carry the left one onto the mono aux, -6 dB each.
        assert!(close(m[2][0], 1.0) && close(m[3][0], 0.0), "{m:?}");
        // The direct out is the channel: leg to leg, image or not.
        assert!(close(m[2][3], 1.0) && close(m[3][4], 1.0), "{m:?}");
        assert!(close(m[2][4], 0.0) && close(m[3][3], 0.0), "{m:?}");
        // AFL at the channel's pan and image: the left leg to both, the
        // right one of them lowered by a pan of -1.
        s["channels"]["st1"]["solo"] = json!("afl");
        s["channels"]["st1"]["pan"]["lr"] = json!(-1);
        let m = matrix(&shape, &s);
        assert!(close(m[2][1], 1.0) && close(m[2][2], 0.0), "{m:?}");
        assert!(close(m[3][1], 0.0) && close(m[3][2], 0.0), "{m:?}");
    }

    /// The monitor's image takes its stereo legs after the fold, then
    /// the speaker mutes: left to both, right to both, or swapped.
    #[test]
    fn the_monitors_image_uses_the_legs_it_plays() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        s["channels"]["st1"]["pan"]["lr"] = json!(0);
        s["monitor"]["level"] = json!(0);
        for (image, want) in [
            ("normal", [1.0, 0.0, 0.0, 1.0]),
            ("leftToBoth", [1.0, 1.0, 0.0, 0.0]),
            ("rightToBoth", [0.0, 0.0, 1.0, 1.0]),
            ("swap", [0.0, 1.0, 1.0, 0.0]),
        ] {
            s["monitor"]["image"] = json!(image);
            let m = matrix(&shape, &s);
            let got = [m[2][5], m[2][6], m[3][5], m[3][6]];
            assert!(
                got.iter().zip(want).all(|(g, w)| close(*g, w)),
                "{image}: {m:?}"
            );
        }
    }

    fn field(lr: f64, fr: f64, divergence: f64) -> Place {
        Place {
            lr,
            fr,
            divergence,
            lfe: -90.0,
        }
    }

    fn legs_of(to: usize, p: &Place) -> Vec<f64> {
        (0..to).map(|j| field_gain(to, j, p)).collect()
    }

    /// The surround panner's law points on 5.1: the front of the field
    /// pans pairwise at constant power, the centre is the C leg alone,
    /// divergence moves its power to L and R, fr crossfades front and
    /// rear and the rear pans between the surrounds.
    #[test]
    fn the_surround_panner_places_a_mono_source_in_the_field() {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let is = |got: Vec<f64>, want: [f64; 6]| {
            assert!(
                got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-12),
                "{got:?} vs {want:?}"
            );
        };
        // L R C LFE Ls Rs.
        is(
            legs_of(6, &field(0.0, -1.0, 0.0)),
            [0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        );
        is(
            legs_of(6, &field(-1.0, -1.0, 0.0)),
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        is(
            legs_of(6, &field(1.0, -1.0, 0.0)),
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        );
        is(
            legs_of(6, &field(-0.5, -1.0, 0.0)),
            [h, 0.0, h, 0.0, 0.0, 0.0],
        );
        is(
            legs_of(6, &field(0.5, -1.0, 0.0)),
            [0.0, h, h, 0.0, 0.0, 0.0],
        );
        // Full divergence is a plain constant-power pan between L and R,
        // as a mono channel was panned onto a surround bus before the
        // field.
        let quarter = std::f64::consts::FRAC_PI_4;
        for lr in [-1.0, -0.5, 0.0, 0.3, 1.0] {
            let theta = (lr + 1.0) * quarter;
            is(
                legs_of(6, &field(lr, -1.0, 1.0)),
                [libm::cos(theta), libm::sin(theta), 0.0, 0.0, 0.0, 0.0],
            );
        }
        // Between, the centre's power goes to L and R.
        is(
            legs_of(6, &field(0.0, -1.0, 1.0)),
            [h, h, 0.0, 0.0, 0.0, 0.0],
        );
        let (c, l) = (libm::cos(0.25 * std::f64::consts::FRAC_PI_2), 0.0);
        let half = libm::sin(0.25 * std::f64::consts::FRAC_PI_2) * h;
        is(
            legs_of(6, &field(0.0, -1.0, 0.25)),
            [half, half, c, l, 0.0, 0.0],
        );
        // The rear: the surrounds alone, panned between them.
        is(
            legs_of(6, &field(0.0, 1.0, 0.0)),
            [0.0, 0.0, 0.0, 0.0, h, h],
        );
        is(
            legs_of(6, &field(-1.0, 1.0, 0.0)),
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        );
        is(
            legs_of(6, &field(1.0, 1.0, 0.0)),
            [0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        );
        // fr crossfades at constant power: half the power each.
        is(
            legs_of(6, &field(0.0, 0.0, 0.0)),
            [0.0, 0.0, h, 0.0, 0.5, 0.5],
        );
    }

    /// The LFE leg takes its send as a gain of its own, whatever the pan
    /// and divergence, and is silent at -90 dB; 7.1 shares the rear
    /// between the side and the rear surrounds.
    #[test]
    fn the_lfe_send_is_a_gain_of_its_own_and_7_1_splits_its_rear() {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        for (lr, fr, d) in [(0.0, -1.0, 0.0), (-1.0, 1.0, 1.0), (0.3, 0.2, 0.6)] {
            let p = Place {
                lfe: -6.0,
                ..field(lr, fr, d)
            };
            assert!((field_gain(6, 3, &p) - gain(-6.0)).abs() < 1e-12);
            assert!((field_gain(8, 3, &p) - gain(-6.0)).abs() < 1e-12);
            assert_eq!(field_gain(6, 3, &field(lr, fr, d)), 0.0, "-90 is off");
        }
        // L R C LFE Ls Rs Lrs Rrs.
        let got = legs_of(8, &field(-1.0, 1.0, 0.0));
        let want = [0.0, 0.0, 0.0, 0.0, h, 0.0, h, 0.0];
        assert!(
            got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-12),
            "{got:?}"
        );
        let got = legs_of(8, &field(0.0, 1.0, 0.0));
        assert!(
            (got[4] - 0.5).abs() < 1e-12 && (got[7] - 0.5).abs() < 1e-12,
            "{got:?}"
        );
    }

    /// Every position keeps the power of the source: the legs but the
    /// LFE sum to unity over the field, on 5.1 and on 7.1.
    #[test]
    fn the_surround_panner_keeps_constant_power_over_the_field() {
        for to in [6, 8] {
            for lr in (-10..=10).map(|n| f64::from(n) / 10.0) {
                for fr in (-10..=10).map(|n| f64::from(n) / 10.0) {
                    for d in (0..=10).map(|n| f64::from(n) / 10.0) {
                        let power: f64 = (0..to)
                            .filter(|&j| j != 3)
                            .map(|j| field_gain(to, j, &field(lr, fr, d)).powi(2))
                            .sum();
                        assert!(
                            (power - 1.0).abs() < 1e-9,
                            "{to} legs at {lr} {fr} {d}: {power}"
                        );
                    }
                }
            }
        }
    }

    /// The matrix reads a mono channel's whole pan onto a 5.1 bus, and
    /// its fold-down onto the stereo output follows from the legs.
    #[test]
    fn a_mono_channels_pan_reaches_a_surround_bus() {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let shape = Shape::parse(&crate::shape::tests::surround()).unwrap();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(0);
        s["channels"]["ch1"]["routes"]["pgm"] = json!(true);
        // Rows ch1, ch2, st1 L R, sur x6; columns out-51 x6, out-lr x2.
        // A new mono channel is the L/R pair of the pan before the field.
        let m = matrix(&shape, &s);
        let row: Vec<f64> = m[0][..6].iter().map(|g| f64::from(*g)).collect();
        assert!(
            (row[0] - h).abs() < 1e-6 && (row[1] - h).abs() < 1e-6 && row[2].abs() < 1e-6,
            "{row:?}"
        );
        // No divergence: the centre leg alone, the fold onto Lo and Ro
        // taking it at -3 dB.
        s["channels"]["ch1"]["pan"]["spread"] = json!(0);
        let m = matrix(&shape, &s);
        let row: Vec<f64> = m[0][..6].iter().map(|g| f64::from(*g)).collect();
        assert!(
            (row[2] - 1.0).abs() < 1e-6 && row[0].abs() < 1e-6 && row[1].abs() < 1e-6,
            "{row:?}"
        );
        assert!(close(m[0][6], h) && close(m[0][7], h), "{m:?}");
        s["channels"]["ch1"]["pan"] =
            json!({ "lr": 0, "frontRear": 1, "spread": 0, "lfeSend": -6 });
        let m = matrix(&shape, &s);
        let row: Vec<f64> = m[0][..6].iter().map(|g| f64::from(*g)).collect();
        assert!(
            row[2].abs() < 1e-6 && (row[4] - h).abs() < 1e-6 && (row[5] - h).abs() < 1e-6,
            "{row:?}"
        );
        assert!((row[3] - gain(-6.0)).abs() < 1e-6, "the LFE send: {row:?}");
    }

    /// A mono channel's send, its PFL and its AFL on a surround element
    /// keep the L/R pair they were before the field: a send and a PFL
    /// by the position they have, the AFL by the channel's own place.
    #[test]
    fn sends_and_listens_of_a_mono_channel_stay_the_lr_pair_on_a_surround_element() {
        let mut v = crate::shape::tests::surround();
        v["auxes"] = json!([{ "id": "aux1", "name": "IFB", "width": 6, "layout": "5.1" }]);
        v["monitorOutputs"] = json!([{ "id": "mon", "name": "Mon", "width": 6, "layout": "5.1" }]);
        v["outputs"] = json!([
            { "id": "o-aux", "name": "IFB", "source": "aux1", "width": 6, "layout": "5.1" },
            { "id": "o-mon", "name": "Mon", "source": "mon", "width": 6, "layout": "5.1" },
        ]);
        let shape = Shape::parse(&v).unwrap();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(0);
        s["channels"]["ch1"]["sends"]["aux1"] =
            json!({ "on": true, "level": 0, "pan": 0.3, "pre": false });
        s["monitor"]["level"] = json!(0);
        let pair = |lr: f64| {
            let theta = (lr + 1.0) * std::f64::consts::FRAC_PI_4;
            [libm::cos(theta), libm::sin(theta), 0.0, 0.0, 0.0, 0.0]
        };
        let legs = |m: &[Vec<f32>], first: usize| -> Vec<f64> {
            m[0][first..first + 6]
                .iter()
                .map(|g| f64::from(*g))
                .collect()
        };
        let is = |got: Vec<f64>, want: [f64; 6]| {
            assert!(
                got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-6),
                "{got:?} vs {want:?}"
            );
        };
        // Columns: o-aux x6, o-mon x6.
        is(legs(&matrix(&shape, &s), 0), pair(0.3));
        // PFL: centred whatever the channel's pan.
        s["channels"]["ch1"]["pan"]["lr"] = json!(-0.5);
        s["channels"]["ch1"]["solo"] = json!("pfl");
        is(legs(&matrix(&shape, &s), 6), pair(0.0));
        // AFL: at the channel's place; with its divergence lowered the
        // centre leg takes it.
        s["channels"]["ch1"]["solo"] = json!("afl");
        is(legs(&matrix(&shape, &s), 6), pair(-0.5));
        s["channels"]["ch1"]["pan"]["spread"] = json!(0);
        s["channels"]["ch1"]["pan"]["lr"] = json!(0);
        let got = legs(&matrix(&shape, &s), 6);
        is(got, [0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
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

    /// PFL listens after the strip and before the fader and pan: a
    /// channel with its fader closed is heard on the monitor, and the
    /// source is not. A hard-panned mono channel is heard centred.
    #[test]
    fn a_pfl_is_heard_on_the_monitor_with_its_fader_closed() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        s["channels"]["ch1"]["pan"]["lr"] = json!(-1);
        s["monitor"]["level"] = json!(-6);
        s["channels"]["ch1"]["solo"] = json!("pfl");
        let m = matrix(&shape, &s);
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!(close(m[0][5], h * gain(-6.0)) && close(m[0][6], h * gain(-6.0)));
        assert!(m[1..].iter().all(|r| r[5] == 0.0 && r[6] == 0.0));
        // Hard right gives the same.
        s["channels"]["ch1"]["pan"]["lr"] = json!(1);
        let m = matrix(&shape, &s);
        assert!(close(m[0][5], h * gain(-6.0)) && close(m[0][6], h * gain(-6.0)));
        // The monitor's own controls follow.
        s["monitor"]["speakerMute"] = json!([false, true]);
        let m = matrix(&shape, &s);
        assert!(close(m[0][5], h * gain(-6.0)) && m[0][6] == 0.0);
    }

    /// AFL is the channel after fader, mute and VCAs and before AFV: a
    /// channel whose AFV stage is closed plays at its fader gain, not at
    /// the off level it adds to a bus.
    #[test]
    fn an_afl_is_heard_before_the_afv_stage() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(-6);
        s["channels"]["ch1"]["pan"]["lr"] = json!(-1);
        s["afv"]["ch1"]["enabled"] = json!(true);
        s["afv"]["ch1"]["open"] = json!(false);
        s["afv"]["ch1"]["offLevel"] = json!(-40);
        s["monitor"]["level"] = json!(0);
        s["channels"]["ch1"]["solo"] = json!("afl");
        let m = matrix(&shape, &s);
        assert!(close(m[0][5], gain(-6.0)) && m[0][6] == 0.0);
        // The bus still takes it at the off level.
        assert!(close(matrix(&shape, &s)[0][0], gain(-6.0 - 40.0)));
    }

    /// AFL listens to what the channel adds to the mix: nothing while its
    /// fader is closed, and at its pan while it is open.
    #[test]
    fn an_afl_of_a_closed_fader_is_silent() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        s["monitor"]["level"] = json!(0);
        s["channels"]["ch1"]["solo"] = json!("afl");
        let m = matrix(&shape, &s);
        assert!(m.iter().all(|r| r[5] == 0.0 && r[6] == 0.0));
        s["channels"]["ch1"]["fader"] = json!(-6);
        s["channels"]["ch1"]["pan"]["lr"] = json!(-1);
        let m = matrix(&shape, &s);
        assert!(close(m[0][5], gain(-6.0)) && close(m[0][6], 0.0));
        assert!(m[1..].iter().all(|r| r[5] == 0.0 && r[6] == 0.0));
    }

    /// VcaState: a VCA's solo listens to each member as if the member had
    /// it set.
    #[test]
    fn a_vca_solo_solos_its_members() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(0);
        s["monitor"]["level"] = json!(0);
        s["vcas"]["vca1"]["members"] = json!(["st1"]);
        s["vcas"]["vca1"]["solo"] = json!("pfl");
        let m = matrix(&shape, &s);
        assert!(close(m[2][5], 1.0) && close(m[3][6], 1.0));
        assert!(m[2][6] == 0.0 && m[3][5] == 0.0);
        assert!(m[..2].iter().all(|r| r[5] == 0.0 && r[6] == 0.0));
        // AFL through the VCA: the member's fader and the VCA's offset.
        s["vcas"]["vca1"]["solo"] = json!("afl");
        s["vcas"]["vca1"]["offset"] = json!(-2);
        s["channels"]["st1"]["fader"] = json!(-4);
        let m = matrix(&shape, &s);
        assert!(close(m[2][5], gain(-6.0)) && close(m[3][6], gain(-6.0)));
        assert!(m[..2].iter().all(|r| r[5] == 0.0 && r[6] == 0.0));
    }

    /// With every solo off again the monitor plays its source, bit for bit
    /// as before any solo.
    #[test]
    fn all_solos_off_restores_the_monitor_source() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        s["monitor"]["level"] = json!(0);
        let source = matrix(&shape, &s);
        assert!(close(source[2][5], 1.0));
        s["channels"]["ch2"]["solo"] = json!("pfl");
        s["vcas"]["vca1"]["solo"] = json!("afl");
        assert!(matrix(&shape, &s)[2][5] == 0.0);
        s["channels"]["ch2"]["solo"] = json!("off");
        s["vcas"]["vca1"]["solo"] = json!("off");
        assert_eq!(matrix(&shape, &s), source);
    }

    /// MonitorMeter: PFL takes priority over AFL, so while any PFL is on
    /// only the PFL listens are heard; a channel listens in PFL when its
    /// own solo or a VCA's it is a member of is PFL.
    #[test]
    fn pfl_takes_priority_over_afl() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["ch1"]["fader"] = json!(0);
        s["channels"]["ch1"]["pan"]["lr"] = json!(-1);
        s["monitor"]["level"] = json!(0);
        s["channels"]["ch1"]["solo"] = json!("afl");
        s["channels"]["ch2"]["solo"] = json!("pfl");
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let m = matrix(&shape, &s);
        assert!(m[0][5] == 0.0 && m[0][6] == 0.0, "the AFL waits");
        assert!(close(m[1][5], h) && close(m[1][6], h));
        s["channels"]["ch2"]["solo"] = json!("off");
        let m = matrix(&shape, &s);
        assert!(close(m[0][5], 1.0) && m[0][6] == 0.0, "the AFL again");
        // A VCA in PFL over a member's own AFL: the member listens in PFL.
        s["vcas"]["vca1"]["members"] = json!(["ch1"]);
        s["vcas"]["vca1"]["solo"] = json!("pfl");
        s["channels"]["ch1"]["fader"] = json!(-90);
        let m = matrix(&shape, &s);
        assert!(close(m[0][5], h) && close(m[0][6], h), "PFL centred");
    }

    /// PanState: a mono channel pans at constant power, a stereo one on a
    /// stereo bus balances its legs without crossing them.
    #[test]
    fn a_stereo_channel_balances_and_a_mono_one_pans_hard_right() {
        let shape = shape();
        let mut s = shape.initial_state();
        s["channels"]["st1"]["fader"] = json!(0);
        s["channels"]["ch1"]["fader"] = json!(0);
        for p in [0.5, 1.0, -1.0, 0.0] {
            s["channels"]["st1"]["pan"]["lr"] = json!(p);
            let m = matrix(&shape, &s);
            let (l, r) = ((1.0f64 - p).min(1.0), (1.0f64 + p).min(1.0));
            assert!(close(m[2][0], l) && close(m[2][1], 0.0), "pan {p}");
            assert!(close(m[3][0], 0.0) && close(m[3][1], r), "pan {p}");
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
        // Stereo balances onto two legs or more: unity at the centre, the
        // opposite leg lowered linearly, the same leg to leg onto 5.1.
        assert_eq!(pan(2, 6, 1, 1, 0.0), 1.0);
        assert_eq!(pan(2, 6, 1, 1, -1.0), 0.0);
        assert_eq!(pan(2, 6, 0, 0, -1.0), 1.0);
        assert_eq!(pan(2, 6, 1, 1, -0.25), 0.75);
        assert_eq!(pan(2, 2, 0, 0, 0.5), 0.5);
        assert_eq!(pan(2, 2, 1, 1, 0.5), 1.0);
        assert_eq!(pan(2, 2, 0, 1, 0.5), 0.0, "legs stay on their own");
        assert_eq!(pan(2, 6, 1, 2, -1.0), 0.0);
        // Stereo onto mono at -6 dB per leg; 5.1 onto mono through Lo/Ro.
        assert_eq!(pan(2, 1, 0, 0, 0.0), 0.5);
        assert_eq!(pan(6, 1, 2, 0, 0.0), h);
        assert_eq!(pan(6, 1, 3, 0, 0.0), 0.0);
    }

    /// A 5.1 channel on a 5.1 bus carried by a 6-leg output and by a
    /// stereo output: the stereo one is the bus folded by BS.775 Lo/Ro,
    /// after the bus fader, and the bus keeps no meter column of its own.
    fn surround() -> (Shape, Value) {
        let v = crate::shape::tests::surround();
        let shape = Shape::parse(&v).unwrap();
        let mut s = shape.initial_state();
        s["channels"]["sur"]["fader"] = json!(0);
        s["channels"]["sur"]["routes"]["pgm"] = json!(true);
        s["buses"]["pgm"]["fader"] = json!(-6);
        (shape, s)
    }

    #[test]
    fn an_output_narrower_than_its_source_folds_it_down() {
        let (shape, s) = surround();
        let m = matrix(&shape, &s);
        let (bus, h) = (gain(-6.0), std::f64::consts::FRAC_1_SQRT_2);
        // Rows: ch1, ch2, st1 L, st1 R, then sur L R C LFE Ls Rs.
        // Columns: out-51 0..6, out-lr 6..8, then cf, aux1, mon.
        assert_eq!(m[0].len(), 6 + 2 + 2 + 1 + 2);
        let want_lr = [
            [1.0, 0.0],
            [0.0, 1.0],
            [h, h],
            [0.0, 0.0],
            [h, 0.0],
            [0.0, h],
        ];
        for (leg, lr) in want_lr.iter().enumerate() {
            let row = &m[4 + leg];
            for (j, g) in row[..6].iter().enumerate() {
                assert!(
                    close(*g, if j == leg { bus } else { 0.0 }),
                    "5.1 [{leg}][{j}]"
                );
            }
            assert!(close(row[6], bus * lr[0]), "Lo of leg {leg}: {}", row[6]);
            assert!(close(row[7], bus * lr[1]), "Ro of leg {leg}: {}", row[7]);
        }
    }

    /// Only a full-width output is the element's meter; a bus a narrower
    /// output alone carries meters on columns of its own.
    #[test]
    fn a_fold_down_output_leaves_its_source_a_meter_unless_a_full_one_exists() {
        let (shape, _) = surround();
        assert!(shape.meter_columns().iter().all(|m| m.0 != "pgm"));
        let mut v = serde_json::to_value(&shape).unwrap();
        v["outputs"].as_array_mut().unwrap().remove(0);
        let shape = Shape::parse(&v).unwrap();
        let pgm = shape.meter_columns().into_iter().find(|m| m.0 == "pgm");
        assert_eq!(pgm, Some(("pgm", 6, "5.1")));
        let m = matrix(&shape, &surround().1);
        // out-lr, then the meters in shape order: pgm (6 legs) first.
        assert_eq!(m[0].len(), 2 + 6 + 2 + 1 + 2);
        assert!(close(m[4][2], gain(-6.0)) && close(m[4][0], gain(-6.0)));
    }

    /// An output wider than its source is refused; the engine would have
    /// to invent legs.
    #[test]
    fn an_output_wider_than_its_source_is_refused() {
        let mut v = crate::shape::tests::small();
        v["outputs"] = json!([
            { "id": "o", "name": "x", "source": "aux1", "width": 2, "layout": "stereo" },
        ]);
        let e = Shape::parse(&v).unwrap_err();
        assert!(e.contains("wider than its source"), "{e}");
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
                let (path, value) = match next(20) {
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
                    15 => (
                        format!(
                            "/{}/solo",
                            ["channels/ch1", "channels/st1", "vcas/vca1"][next(3) as usize]
                        ),
                        json!(["off", "off", "off", "pfl", "afl"][next(5) as usize]),
                    ),
                    16 => (
                        "/mixMinus/mm1/interrupt/source".into(),
                        if next(2) == 0 { json!(null) } else { json!(ch) },
                    ),
                    17 => ("/mixMinus/mm1/interrupt/active".into(), flag),
                    18 => (
                        if next(2) == 0 {
                            format!("/channels/{ch}/image")
                        } else {
                            "/monitor/image".into()
                        },
                        json!(["normal", "leftToBoth", "rightToBoth", "swap"][next(4) as usize]),
                    ),
                    _ => match next(9) {
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
                        7 => ("/monitor/talkbackDim".into(), flag),
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
            // The line-up tone is off: its rows, after the channels', are 0.
            let mut m = matrix(&shape, &s);
            let tone = m.split_off(shape.channels.iter().map(|c| c.width).sum());
            assert!(tone.iter().flatten().all(|g| g.to_bits() == 0), "{s}");
            assert_eq!(bits(m), bits(before::matrix(&shape, &s)), "{s}");
        }
    }

    /// The state of [`shape`] with ch1 and st1 open on every bus and
    /// sent to aux1, and every bus, aux and mix-minus at 0 dB: every
    /// column of an element carries a mix.
    fn mixing(shape: &Shape) -> Value {
        let mut s = shape.initial_state();
        for ch in ["ch1", "st1"] {
            s["channels"][ch]["fader"] = json!(0);
            s["channels"][ch]["sends"]["aux1"]["on"] = json!(true);
            s["channels"][ch]["sends"]["aux1"]["level"] = json!(0);
        }
        for path in ["/buses/pgm", "/buses/cf", "/auxes/aux1", "/mixMinus/mm1"] {
            crate::state::set_at(&mut s, &format!("{path}/fader"), json!(0));
        }
        s
    }

    /// The line-up tone generators' rows, after the channels', and the
    /// channels'.
    fn tone_rows(m: &[Vec<f32>], shape: &Shape) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let mut m = m.to_vec();
        let tone = m.split_off(shape.channels.iter().map(|c| c.width).sum());
        (tone, m)
    }

    /// While `/tone` is on, each column of an element in its targets
    /// carries the leg of the same index of the generator as wide as the
    /// element at unity, and no channel: the programme's output, the mono
    /// aux's output, which takes the mono generator, a mix-minus output
    /// and the meter columns of a bus no output carries. Every other
    /// column keeps its mix. Off, or with no targets, the tone's rows are
    /// 0 and the mix is as without it. The monitor's columns, which play
    /// the targeted programme, are
    /// [`a_monitor_on_a_line_up_target_plays_its_tone`]'s.
    #[test]
    fn the_line_up_tone_replaces_the_mix_of_its_targets() {
        let shape = shape();
        assert_eq!(tone_widths(&shape), [1, 2], "aux1, then pgm, cf and mm1");
        let mut s = mixing(&shape);
        let mixed = matrix(&shape, &s);
        let (tone, channels) = tone_rows(&mixed, &shape);
        assert!(tone.iter().flatten().all(|g| *g == 0.0), "off: no tone");
        for (cols, what) in [(0..2, "pgm"), (2..3, "aux1"), (3..5, "mm1"), (8..10, "cf")] {
            assert!(
                cols.clone().all(|c| channels.iter().any(|r| r[c] > 0.1)),
                "{what} carries a mix"
            );
        }
        // The columns of the elements replaced: first column, width and
        // the first tone row of their generator.
        let replaced = |elements: &[(usize, usize, usize)], s: &Value| {
            let m = matrix(&shape, s);
            let (tone, channels) = tone_rows(&m, &shape);
            for c in (0..mixed[0].len()).filter(|c| !(5..7).contains(c)) {
                let leg = (elements.iter())
                    .find(|(first, width, _)| (*first..first + width).contains(&c))
                    .map(|(first, _, generator)| generator + c - first);
                for (l, r) in tone.iter().enumerate() {
                    let want = f32::from(u8::from(leg == Some(l)));
                    assert_eq!(r[c].to_bits(), want.to_bits(), "tone leg {l} column {c}");
                }
                for (row, r) in channels.iter().enumerate() {
                    let want = if leg.is_some() { 0.0 } else { mixed[row][c] };
                    assert_eq!(r[c].to_bits(), want.to_bits(), "row {row} column {c}");
                }
            }
        };
        s["tone"]["targets"] = json!(["pgm", "aux1", "mm1", "cf"]);
        replaced(&[], &s);
        s["tone"]["on"] = json!(true);
        replaced(&[(0, 2, 1), (2, 1, 0), (3, 2, 1), (8, 2, 1)], &s);
        s["tone"]["targets"] = json!(["mm1"]);
        replaced(&[(3, 2, 1)], &s);
        s["tone"]["targets"] = json!([]);
        replaced(&[], &s);
    }

    /// The tone replaces a targeted element after its fader: the element's
    /// fader, mute and polarity and the channels' faders leave its columns
    /// as they are, and only its generator follows `/tone/level` and
    /// `/tone/ident`, at 1 kHz.
    #[test]
    fn the_line_up_tone_is_after_the_elements_fader() {
        let shape = shape();
        let mut s = mixing(&shape);
        s["tone"] = json!({ "on": true, "level": -18, "ident": false, "targets": ["pgm", "mm1"] });
        let wiring = vec![
            Wiring {
                a: "a".into(),
                b: None,
                domain: "d".into(),
            };
            shape.channels.len()
        ];
        let toned = targets(&shape, &wiring, &s);
        for (path, value) in [
            ("/buses/pgm/fader", json!(-20)),
            ("/buses/pgm/mute", json!(true)),
            ("/buses/pgm/polarity", json!([true, false])),
            ("/mixMinus/mm1/fader", json!(-90)),
            ("/channels/ch1/fader", json!(-6)),
        ] {
            crate::state::set_at(&mut s, path, value);
        }
        let moved = targets(&shape, &wiring, &s);
        for c in [0, 1, 3, 4] {
            for (row, (a, b)) in toned.gains.iter().zip(&moved.gains).enumerate() {
                assert_eq!(a[c].to_bits(), b[c].to_bits(), "row {row} column {c}");
            }
        }
        assert_ne!(toned.gains, moved.gains, "the faders moved the rest");
        let slots = shape.channels.len()..shape.channels.len() + 2;
        let line_up = |level_db, ident| ToneParams {
            frequency_hz: 1000,
            level_db,
            ident,
        };
        assert_eq!(moved.tones[slots.clone()], [Some(line_up(-18.0, false)); 2]);
        crate::state::set_at(&mut s, "/tone/level", json!(-20));
        crate::state::set_at(&mut s, "/tone/ident", json!(true));
        let t = targets(&shape, &wiring, &s);
        assert_eq!(t.tones[slots.clone()], [Some(line_up(-20.0, true)); 2]);
        assert_eq!(t.gains, moved.gains, "level and ident move no gain");
        assert_eq!((t.flows.len(), t.strips.len(), t.tones.len()), (5, 5, 5));
        for slot in slots {
            assert_eq!((&t.flows[slot], t.strips[slot].len()), (&None, 0));
        }
        assert_eq!(
            (duck::masks(&shape, &s).len(), t.gains.len()),
            (7, 7),
            "the masks have the tone's rows"
        );
    }

    /// A write of `/tone/level` or `/tone/ident` moves the tone's
    /// generator alone; one of whether it is on or of its targets moves
    /// everything.
    #[test]
    fn a_tone_write_moves_what_it_reads() {
        let shape = shape();
        let s = shape.initial_state();
        for path in ["/tone/level", "/tone/ident"] {
            let moved = Moved::of(&shape, &s, &Value::Null, [path]);
            assert!(moved.line_up && !moved.all, "{path}");
        }
        for path in ["/tone/on", "/tone/targets", "/tone"] {
            assert!(Moved::of(&shape, &s, &Value::Null, [path]).all, "{path}");
        }
    }

    /// The line-up tone has a generator per distinct width of a bus, aux
    /// or mix-minus, narrowest first, and every column of a target takes
    /// the legs of the one as wide as its element, leg to leg: the 5.1
    /// programme's output its six legs and its stereo fold-down output
    /// the first two, the stereo clean feed and mix-minus the stereo
    /// generator, the mono aux the mono one. So the ident of each cuts
    /// the target's own legs in turn.
    #[test]
    fn each_line_up_target_plays_the_generator_of_its_width() {
        let mut v = crate::shape::tests::surround();
        v["mixMinus"] =
            json!([{ "id": "mm1", "name": "N-1", "bus": "cf", "width": 2, "layout": "stereo" }]);
        let shape = Shape::parse(&v).unwrap();
        assert_eq!(tone_widths(&shape), [1, 2, 6], "aux1, cf and mm1, pgm");
        assert_eq!(tone_input(6), "/tone/6");
        let mut s = shape.initial_state();
        s["tone"]["on"] = json!(true);
        s["tone"]["targets"] = json!(["pgm", "cf", "aux1", "mm1"]);
        let m = matrix(&shape, &s);
        let (tone, _) = tone_rows(&m, &shape);
        assert_eq!(tone.len(), 1 + 2 + 6);
        // The first tone row of the generator `width` legs wide.
        let generator = |width: usize| match width {
            1 => 0,
            2 => 1,
            6 => 3,
            _ => unreachable!("{width}"),
        };
        let mut first = 0;
        let mut checked = Vec::new();
        for (id, width) in columns(&shape) {
            if shape.is_output_bus(id) {
                let row = generator(width_of(&shape, id));
                for leg in 0..width {
                    let got: Vec<f32> = tone.iter().map(|r| r[first + leg]).collect();
                    let want: Vec<f32> = (0..tone.len())
                        .map(|r| f32::from(u8::from(r == row + leg)))
                        .collect();
                    assert_eq!(got, want, "{id} leg {leg}");
                }
                checked.push((id, width));
            }
            first += width;
        }
        assert_eq!(
            checked,
            [("pgm", 6), ("pgm", 2), ("cf", 2), ("aux1", 1), ("mm1", 2)]
        );
    }

    /// A monitor playing an element the line-up tone replaces plays what
    /// the element's output carries: in place of its mix the generator
    /// as wide as it, as the monitor plays a source, so its level, speaker
    /// mutes and cut apply and the mono aux's generator is panned centre
    /// onto both legs. A source the tone does not replace plays its mix,
    /// and a solo the listen sum and no tone. A write of the source, a
    /// solo or a monitor control derives the same entries as the whole
    /// state.
    #[test]
    fn a_monitor_on_a_line_up_target_plays_its_tone() {
        let shape = shape();
        let mut s = mixing(&shape);
        s["monitor"]["level"] = json!(0);
        s["monitor"]["source"] = json!("pgm");
        s["tone"]["on"] = json!(true);
        s["tone"]["targets"] = json!(["pgm", "aux1"]);
        // The columns of o-mon, L and R: the tone's rows (mono, stereo L,
        // stereo R) and the channels'.
        let mon = |s: &Value| {
            let m = matrix(&shape, s);
            let (tone, channels) = tone_rows(&m, &shape);
            let lr = |r: &Vec<f32>| [r[5], r[6]];
            let tone: Vec<[f32; 2]> = tone.iter().map(lr).collect();
            (tone, channels.iter().map(lr).collect::<Vec<_>>())
        };
        let untoned = |s: &Value| {
            let mut s = s.clone();
            s["tone"]["on"] = json!(false);
            mon(&s).1
        };
        let (tone, channels) = mon(&s);
        assert!(channels.iter().flatten().all(|g| *g == 0.0), "{channels:?}");
        assert_eq!(tone, [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        assert!(untoned(&s).iter().flatten().any(|g| *g > 0.1), "a mix");
        s["monitor"]["level"] = json!(-6);
        s["monitor"]["speakerMute"] = json!([false, true]);
        let g = gain(-6.0) as f32;
        assert_eq!(mon(&s).0, [[0.0, 0.0], [g, 0.0], [0.0, 0.0]]);
        s["monitor"]["speakerMute"] = json!([false, false]);
        s["monitor"]["cut"] = json!(true);
        assert!(mon(&s).0.iter().flatten().all(|g| *g == 0.0), "cut");
        s["monitor"]["cut"] = json!(false);
        s["monitor"]["level"] = json!(0);
        s["monitor"]["source"] = json!("aux1");
        let (tone, channels) = mon(&s);
        assert!(channels.iter().flatten().all(|g| *g == 0.0), "{channels:?}");
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!(close(tone[0][0], h) && close(tone[0][1], h), "{tone:?}");
        assert!(tone[1..].iter().flatten().all(|g| *g == 0.0), "{tone:?}");
        s["monitor"]["source"] = json!("mm1");
        let (tone, channels) = mon(&s);
        assert!(tone.iter().flatten().all(|g| *g == 0.0), "{tone:?}");
        assert_eq!(channels, untoned(&s), "mm1's mix");
        s["monitor"]["source"] = json!("pgm");
        s["channels"]["ch1"]["solo"] = json!("pfl");
        let (tone, channels) = mon(&s);
        assert!(tone.iter().flatten().all(|g| *g == 0.0), "{tone:?}");
        assert_eq!(channels, untoned(&s), "the listen sum");
        assert!(channels.iter().flatten().any(|g| *g > 0.1), "ch1's PFL");

        let wiring = vec![
            Wiring {
                a: "a".into(),
                b: None,
                domain: "d".into(),
            };
            shape.channels.len()
        ];
        let mut d = Derived::new(&shape, &wiring, &s);
        for (path, value) in [
            ("/channels/ch1/solo", json!("off")),
            ("/monitor/source", json!("mm1")),
            ("/monitor/source", json!("pgm")),
            ("/monitor/level", json!(-12)),
            ("/monitor/source", json!("aux1")),
            ("/channels/st1/solo", json!("afl")),
        ] {
            let vcas = s["vcas"].clone();
            crate::state::set_at(&mut s, path, value);
            d.update(&shape, &wiring, &s, &vcas, [path]);
            assert_eq!(
                d.targets().gains,
                targets(&shape, &wiring, &s).gains,
                "{path}"
            );
        }
    }
}
