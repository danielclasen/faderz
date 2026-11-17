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
//! state is the input's strip ([`crate::strip`]) or is kept and not
//! applied.

use serde_json::Value;

use crate::shape::Shape;

/// A fader at this level or below is closed.
const CLOSED_DB: f64 = -90.0;
/// The most a channel's fader and VCA offsets add up to.
const MAX_CHANNEL_DB: f64 = 20.0;

/// Linear gain of `db`, 0 at [`CLOSED_DB`] and below.
pub fn gain(db: f64) -> f64 {
    if db <= CLOSED_DB {
        0.0
    } else {
        10f64.powf(db / 20.0)
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
                0 => theta.cos(),
                1 => theta.sin(),
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

/// The gain of each channel's meter, the same on every leg: the
/// engine meters the channel after its strip, which is the input and
/// pre-fader tap; a post-fader `meterTap` adds the fader, mute and VCAs.
pub fn meter_taps(shape: &Shape, state: &Value) -> Vec<f64> {
    shape
        .channels
        .iter()
        .map(|c| {
            let tap = match state["channels"][&c.id]["meterTap"].as_str() {
                Some("postFader") => Tap::PostFader,
                _ => Tap::PreFader,
            };
            channel(state, &c.id, tap)
        })
        .collect()
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
        s["channels"]["ch2"]["fader"] = json!(-6);
        s["channels"]["ch2"]["meterTap"] = json!("postFader");
        s["channels"]["st1"]["meterTap"] = json!("postFader");
        s["channels"]["st1"]["fader"] = json!(0);
        s["channels"]["st1"]["mute"] = json!(true);
        let t = meter_taps(&shape, &s);
        assert_eq!(t.len(), 3);
        assert!((t[0] - 1.0).abs() < 1e-12);
        assert!((t[1] - gain(-6.0)).abs() < 1e-12);
        assert_eq!(t[2], 0.0);
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
}
