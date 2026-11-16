//! The live state as the engine's gain matrix: one row per input channel
//! (the legs of each channel in shape order) and one column per output
//! channel (the legs of each shape output in shape order).
//!
//! What a matrix coefficient can carry is applied: trim, leg trim and
//! polarity, fader, mute, VCA offsets and mutes, AFV open or closed, pan
//! of a mono leg onto a stereo element and balance of a stereo one, bus
//! routes, sends and their levels, bus, aux and mix-minus faders, mutes
//! and polarity, the mix-minus exclusions, and the monitor's source,
//! level, dim, cut, mono and speaker mutes and solos. Everything else in
//! the state is kept and not applied.

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
    /// The selected input, before trim.
    Input,
    /// After trim, leg trim and polarity.
    PreFader,
    /// After fader, mute and VCAs, before AFV.
    PostFader,
    /// After AFV: what buses and post-fader sends get.
    Mix,
}

/// The gain of leg `leg` of channel `id` at `tap`.
fn channel(state: &Value, id: &str, leg: usize, tap: Tap) -> f64 {
    if tap == Tap::Input {
        return 1.0;
    }
    let c = &state["channels"][id];
    let sign = if on(&c["polarity"][leg]) { -1.0 } else { 1.0 };
    let mut g = sign * gain(num(&c["trim"]) + num(&c["legTrim"][leg]));
    if tap == Tap::PreFader {
        return g;
    }
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
/// element, `pan` being the pan (mono onto stereo, constant power) or
/// balance (stereo onto stereo) from -1 to 1.
pub fn pan(from: usize, to: usize, i: usize, j: usize, pan: f64) -> f64 {
    let pan = pan.clamp(-1.0, 1.0);
    match (from, to) {
        (1, 1) => 1.0,
        // Mono onto the first two legs of a wider element.
        (1, _) => {
            let theta = (pan + 1.0) * std::f64::consts::FRAC_PI_4;
            match j {
                0 => theta.cos(),
                1 => theta.sin(),
                _ => 0.0,
            }
        }
        // Stereo summed to mono at -6 dB, so a correlated pair keeps its
        // level.
        (_, 1) => 0.5,
        (2, 2) if i == j => match i {
            0 => (1.0 - pan).min(1.0),
            _ => (1.0 + pan).min(1.0),
        },
        _ => f64::from(u8::from(i == j)),
    }
}

/// The output channels' gains for each input channel: `[input][output]`.
pub fn matrix(shape: &Shape, state: &Value) -> Vec<Vec<f32>> {
    let rows: usize = shape.channels.iter().map(|c| c.width).sum();
    let mut m = vec![Vec::new(); rows];
    for o in &shape.outputs {
        let element = element(shape, state, &o.source);
        for leg in 0..o.width {
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
                let at = channel(state, &c.id, i, tap) * level;
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

/// The monitor output: its source's legs onto the monitor's, then level,
/// dim, cut, mono and the speaker mutes and solos.
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
    let mono = on(&m["mono"]);
    if mono {
        level *= gain(-num(&m["monoCompensation"]));
    }
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
                        legs.iter().sum()
                    } else {
                        (0..from)
                            .map(|i| legs[i] * pan(from, width, i, j, 0.0))
                            .sum()
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
        assert!(m.iter().all(|r| r.len() == 8));
        for (row, r) in m.iter().enumerate() {
            for (col, g) in r.iter().enumerate() {
                let want = if row == 0 && col == 7 { 1.0 } else { 0.0 };
                assert!(close(*g, want), "[{row}][{col}] = {g}");
            }
        }
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

    /// Fader, trim, polarity, mute, VCA and bus fader all multiply into
    /// one coefficient.
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
        assert!(close(m[2][0], gain(3.0 - 6.0 + 1.0 - 10.0)));
        assert!(close(m[2][1], 0.0));
        assert!(close(m[3][1], -gain(3.0 - 3.0 - 6.0 + 1.0 - 10.0)));
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
}
