//! Duck groups in the state: the initial state of each group, the rules a
//! group keeps beyond what the schema says, the recall-safe group of each
//! of its leaves, and the engine's duck targets the state gives
//! ([`targets`], [`masks`], [`columns`]).
//!
//! Ranges (in the schema) and defaults (here) are those of samplez-dsp's
//! `duck` module: `THRESHOLD_DB`, `HYSTERESIS_DB`, `KEY_HPF_FREQ`,
//! `KEY_LPF_FREQ`, `ATTACK_MS`, `HOLD_MS`, `RELEASE_MS`, `DEPTH_DB`,
//! `MAX_KEYS` and `DetectorParams::DEFAULT`.

use samplez::DuckTargets;
use samplez_dsp::{DetectorParams, DuckMode, KeyFilter};
use serde_json::{Map, Value, json};

use crate::mix;
use crate::shape::Shape;

/// Trigger channels a group takes at most: samplez-dsp `duck::MAX_KEYS`.
pub const MAX_TRIGGERS: usize = 8;

/// The duck groups of an empty start: the detector at samplez-dsp's
/// defaults, keyed pre-fader while the channel is open, no trigger and
/// no target, a depth of 10 dB ready for each channel made a target, and
/// the buses of kind program as destinations.
pub fn initial_state(shape: &Shape) -> Value {
    let channels: Map<String, Value> = (shape.channels.iter())
        .map(|c| {
            let member = json!({ "trigger": false, "target": false, "depth": 10 });
            (c.id.clone(), member)
        })
        .collect();
    let buses = (shape.buses.iter()).map(|b| (b.id.clone(), json!(b.kind == "program")));
    let others = (shape.auxes.iter().map(|a| &a.id))
        .chain(shape.mix_minus.iter().map(|m| &m.id))
        .map(|id| (id.clone(), json!(false)));
    let destinations: Map<String, Value> = buses.chain(others).collect();
    let group = json!({
        "mode": "auto",
        "hold": false,
        "keyTap": "preFaderOpen",
        "threshold": -30,
        "hysteresis": 4,
        "keyHpf": { "enabled": true, "freq": 120 },
        "keyLpf": { "enabled": false, "freq": 4000 },
        "attackMs": 20,
        "holdMs": 500,
        "releaseMs": 1000,
        "channels": channels,
        "destinations": destinations,
    });
    let groups = shape.ducks.iter().map(|d| (d.id.clone(), group.clone()));
    Value::Object(groups.collect())
}

/// The rules of group `id` in `ducks`, the state's `ducks`: no channel is
/// trigger and target of it at once, which would duck the channel under
/// itself, and it has at most [`MAX_TRIGGERS`] triggers.
pub fn check(ducks: &Value, id: &str) -> Result<(), String> {
    let mut triggers = 0;
    for (channel, c) in ducks[id]["channels"].as_object().into_iter().flatten() {
        let trigger = c["trigger"] == true;
        if trigger && c["target"] == true {
            return Err(format!(
                "channel {channel} cannot be trigger and target of duck {id}"
            ));
        }
        triggers += usize::from(trigger);
    }
    if triggers > MAX_TRIGGERS {
        return Err(format!("duck {id} takes at most {MAX_TRIGGERS} triggers"));
    }
    Ok(())
}

/// Whether `path` is the momentary hold of a duck group.
pub fn is_hold(path: &str) -> bool {
    matches!(
        path.split('/').collect::<Vec<_>>().as_slice(),
        ["", "ducks", _, "hold"]
    )
}

/// The group `id` of a duck group whose triggers or targets `path` sets.
pub fn members_of(path: &str) -> Option<&str> {
    match path.split('/').collect::<Vec<_>>().as_slice() {
        ["", "ducks", id, "channels", _, "trigger" | "target"] => Some(*id),
        _ => None,
    }
}

/// The group `id` of a duck group whose routing `path` sets: its
/// triggers, targets and destinations.
pub fn routing_of(path: &str) -> Option<&str> {
    let tokens: Vec<String> = path.split('/').skip(3).map(str::to_owned).collect();
    match path.split('/').collect::<Vec<_>>().as_slice() {
        ["", "ducks", id, ..] if recall_group(&tokens) == Some("routing") => Some(*id),
        _ => None,
    }
}

/// The recall-safe group of a duck group's leaf, from the tokens after
/// the group id: triggers, targets and destinations are routing, the key
/// tap, detector and depths dynamics. `None` for mode and hold, which are
/// operated live and never recalled.
pub fn recall_group(key: &[String]) -> Option<&'static str> {
    match key.first()?.as_str() {
        "mode" | "hold" => None,
        "destinations" => Some("routing"),
        "channels" => match key.get(2)?.as_str() {
            "depth" => Some("dynamics"),
            _ => Some("routing"),
        },
        _ => Some("dynamics"),
    }
}

/// The engine's settings of each duck group of `shape` in `state`, one
/// per group in shape order, which is the engine's group number: the
/// detector, a key per trigger channel in shape order at the gain its
/// `keyTap` gives ([`key_gain`]), the mode, the hold, and per input slot
/// the depth of a target channel, 0 dB for any other.
pub fn targets(shape: &Shape, state: &Value) -> Vec<DuckTargets> {
    (shape.ducks.iter())
        .map(|d| group(shape, state, &state["ducks"][&d.id]))
        .collect()
}

fn group(shape: &Shape, state: &Value, g: &Value) -> DuckTargets {
    let f = |v: &Value| v.as_f64().unwrap_or(0.0) as f32;
    let filter = |v: &Value| KeyFilter {
        enabled: v["enabled"] == true,
        freq: f(&v["freq"]),
    };
    let params = DetectorParams {
        threshold_db: f(&g["threshold"]),
        hysteresis_db: f(&g["hysteresis"]),
        hpf: filter(&g["keyHpf"]),
        lpf: filter(&g["keyLpf"]),
        attack_ms: f(&g["attackMs"]),
        hold_ms: f(&g["holdMs"]),
        release_ms: f(&g["releaseMs"]),
    };
    let member = |id: &str| &g["channels"][id];
    let tap = g["keyTap"].as_str().unwrap_or_default();
    let keys = (shape.channels.iter().enumerate())
        .filter(|(_, c)| member(&c.id)["trigger"] == true)
        .map(|(slot, c)| (slot, key_gain(state, &c.id, tap)))
        .collect();
    let mode = match g["mode"].as_str() {
        Some("on") => DuckMode::On,
        Some("off") => DuckMode::Off,
        _ => DuckMode::Auto,
    };
    let depths = (shape.channels.iter())
        .map(|c| match member(&c.id) {
            m if m["target"] == true => f(&m["depth"]),
            _ => 0.0,
        })
        .collect();
    DuckTargets {
        params,
        keys,
        mode,
        held: g["hold"] == true,
        depths,
    }
}

/// The gain channel `id` keys a group at, after its strip: 1 at
/// `preFader`; at `preFaderOpen` 1 while the channel reaches a bus (fader
/// open, not muted, no VCA mute), else 0; at `postFader` its fader, mute
/// and VCA gain.
pub fn key_gain(state: &Value, id: &str, tap: &str) -> f32 {
    match tap {
        "preFader" => 1.0,
        "postFader" => mix::post_fader(state, id) as f32,
        _ => f32::from(u8::from(mix::post_fader(state, id) > 0.0)),
    }
}

/// The duck mask of each coefficient of the matrix ([`mix::matrix`]),
/// shaped as it: bit `g` for group `g` where the row's channel is a
/// target of `g` and the column carries a destination of `g`: an output
/// or meter column of that bus, aux or mix-minus output, or a monitor
/// output while the monitor's source is one, so the monitor plays and
/// meters what goes out. A mix-minus that carries its talkback
/// interrupt ([`mix::talkback`]) has no mask, and neither has a monitor
/// playing it. While anything solos the monitor plays the
/// listens, no destination, and none of its columns is masked. The
/// line-up tone's rows, which no group lowers, and the pre-strip rows
/// ([`mix::pre_rows`]), which samplez ducks never, have none. None
/// without duck groups.
pub fn masks(shape: &Shape, state: &Value) -> Vec<Vec<u8>> {
    if shape.ducks.is_empty() {
        return Vec::new();
    }
    let groups = |is: &dyn Fn(&Value) -> bool| -> u8 {
        (shape.ducks.iter().enumerate())
            .filter(|(_, d)| is(&state["ducks"][&d.id]))
            .fold(0, |bits, (g, _)| bits | 1 << g)
    };
    // A mix-minus carrying its talkback is never ducked: the talent hears
    // the producer whatever the groups do to the mix it replaced.
    let carried = |id: &str| {
        if mix::talkback(state, id).is_some() {
            0
        } else {
            groups(&|d| d["destinations"][id] == true)
        }
    };
    let monitored = if mix::solos(state).0 {
        0
    } else {
        carried(state["monitor"]["source"].as_str().unwrap_or_default())
    };
    let columns: Vec<u8> = (mix::columns(shape).into_iter())
        .flat_map(|(id, width)| {
            let monitor = shape.monitor_outputs.iter().any(|m| m.id == id);
            let bits = if monitor { monitored } else { carried(id) };
            std::iter::repeat_n(bits, width)
        })
        .collect();
    let tone = vec![0; columns.len()];
    (shape.channels.iter())
        .flat_map(|c| {
            let lowered = groups(&|d| d["channels"][&c.id]["target"] == true);
            let row: Vec<u8> = columns.iter().map(|&bits| bits & lowered).collect();
            std::iter::repeat_n(row, c.width)
        })
        .chain(std::iter::repeat_n(
            tone,
            mix::tone_widths(shape).iter().sum::<usize>() + mix::pre_strip_rows(shape),
        ))
        .collect()
}

/// The matrix columns a duck mask may duck, the engine's `duckColumns`:
/// those of every bus, aux, mix-minus and monitor output, any of which a
/// group can lower; none without duck groups, so a shape without them
/// pays nothing for ducking.
pub fn columns(shape: &Shape) -> Vec<usize> {
    if shape.ducks.is_empty() {
        return Vec::new();
    }
    let mut first = 0;
    let mut ducked = Vec::new();
    for (id, width) in mix::columns(shape) {
        if !shape.is_channel(id) {
            ducked.extend(first..first + width);
        }
        first += width;
    }
    ducked
}

#[cfg(test)]
mod tests {
    use samplez_dsp::DuckMode;
    use serde_json::{Value, json};

    use super::*;
    use crate::shape::tests::small;
    use crate::state::set_at;

    /// The small shape with a second group, `rem`: rows ch1, ch2, st1 L
    /// and R; columns pgm L R (its output), then the engine meters of cf
    /// L R, aux1, mm1 L R and mon L R.
    fn two_groups() -> (Shape, Value) {
        let mut v = small();
        v["ducks"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "id": "rem", "name": "Remote" }));
        let shape = Shape::parse(&v).unwrap();
        let state = shape.initial_state();
        (shape, state)
    }

    fn with(state: &mut Value, writes: &[(&str, Value)]) {
        for (path, value) in writes {
            set_at(state, path, value.clone());
        }
    }

    /// A mask names a group on a target's rows in the columns of the
    /// group's destinations: the output carrying pgm, the meter columns of
    /// an element no full-width output carries, and the monitor while its source is a
    /// destination. Overlapping groups set both bits; a trigger's rows and
    /// a column of no destination stay 0.
    #[test]
    fn masks_name_each_group_on_its_targets_in_its_destinations() {
        let (shape, mut state) = two_groups();
        assert!(masks(&shape, &state).iter().flatten().all(|&m| m == 0));
        with(
            &mut state,
            &[
                ("/ducks/comm/channels/ch2/trigger", json!(true)),
                ("/ducks/comm/channels/ch1/target", json!(true)),
                ("/ducks/comm/channels/st1/target", json!(true)),
                ("/ducks/comm/destinations/mm1", json!(true)),
                ("/ducks/rem/channels/ch1/target", json!(true)),
                ("/ducks/rem/destinations/pgm", json!(false)),
                ("/ducks/rem/destinations/aux1", json!(true)),
            ],
        );
        let m = masks(&shape, &state);
        // pgm out, cf, aux1, mm1, mon (source pgm).
        assert_eq!(m[0], [1, 1, 0, 0, 2, 1, 1, 1, 1]);
        assert_eq!(m[1], [0; 9], "a trigger is not lowered");
        assert_eq!(m[2], [1, 1, 0, 0, 0, 1, 1, 1, 1]);
        assert_eq!(m[3], m[2], "every leg of a target");
        // The monitor follows its source: on aux1 it carries rem, on cf
        // neither group.
        set_at(&mut state, "/monitor/source", json!("aux1"));
        assert_eq!(masks(&shape, &state)[0][7..], [2, 2]);
        set_at(&mut state, "/monitor/source", json!("cf"));
        assert_eq!(masks(&shape, &state)[0][7..], [0, 0]);
        // On the listens, a PFL of a target among them, neither.
        set_at(&mut state, "/monitor/source", json!("aux1"));
        set_at(&mut state, "/channels/ch1/solo", json!("pfl"));
        assert_eq!(masks(&shape, &state)[0][7..], [0, 0]);
        set_at(&mut state, "/channels/ch1/solo", json!("off"));
        assert_eq!(masks(&shape, &state)[0][7..], [2, 2]);
        // Both groups on one coefficient.
        set_at(&mut state, "/ducks/rem/destinations/pgm", json!(true));
        assert_eq!(masks(&shape, &state)[0][..2], [3, 3]);
    }

    /// A channel's pre-strip rows, which carry its direct out at the
    /// input tap, take no mask whatever its groups lower: samplez refuses
    /// one there. The masks cover every matrix row, as samplez takes them.
    #[test]
    fn pre_strip_rows_take_no_mask() {
        let mut v = small();
        v["outputs"].as_array_mut().unwrap().push(
            json!({ "id": "do1", "name": "DO 1", "source": "ch1", "width": 1, "layout": "mono" }),
        );
        let shape = Shape::parse(&v).unwrap();
        let mut state = shape.initial_state();
        with(
            &mut state,
            &[
                ("/ducks/comm/channels/ch1/target", json!(true)),
                ("/channels/ch1/directOut/tap", json!("input")),
            ],
        );
        let m = masks(&shape, &state);
        assert_eq!(m.len(), mix::matrix(&shape, &state).len());
        assert_eq!(m[0][..2], [1, 1], "ch1's own row on pgm");
        let pre = mix::pre_rows(&shape)[0].unwrap();
        assert_eq!(pre, m.len() - 1);
        assert!(m[pre].iter().all(|&k| k == 0));
    }

    /// Talkback is never ducked: while a mix-minus carries its interrupt
    /// its columns have no mask, nor has a monitor playing it; released,
    /// the masks return. A solo keeps the monitor unmasked either way.
    #[test]
    fn a_mix_minus_carrying_talkback_is_never_ducked() {
        let (shape, mut state) = two_groups();
        with(
            &mut state,
            &[
                ("/ducks/comm/channels/ch1/target", json!(true)),
                ("/ducks/comm/destinations/pgm", json!(false)),
                ("/ducks/comm/destinations/mm1", json!(true)),
                ("/ducks/rem/channels/ch1/target", json!(true)),
                ("/ducks/rem/destinations/pgm", json!(true)),
                ("/monitor/source", json!("mm1")),
            ],
        );
        // pgm out, cf, aux1, mm1, mon.
        assert_eq!(masks(&shape, &state)[0], [2, 2, 0, 0, 0, 1, 1, 1, 1]);
        // The talker is the target: the interrupt lifts mm1 and the monitor.
        with(
            &mut state,
            &[(
                "/mixMinus/mm1/interrupt",
                json!({ "source": "ch1", "active": true }),
            )],
        );
        assert_eq!(masks(&shape, &state)[0], [2, 2, 0, 0, 0, 0, 0, 0, 0]);
        // An interrupt with no source carries no talkback.
        set_at(&mut state, "/mixMinus/mm1/interrupt/source", Value::Null);
        assert_eq!(masks(&shape, &state)[0], [2, 2, 0, 0, 0, 1, 1, 1, 1]);
        set_at(&mut state, "/mixMinus/mm1/interrupt/source", json!("ch1"));
        // Released, the masks are back.
        set_at(&mut state, "/mixMinus/mm1/interrupt/active", json!(false));
        assert_eq!(masks(&shape, &state)[0], [2, 2, 0, 0, 0, 1, 1, 1, 1]);
        // With a solo the monitor has no mask, interrupted or not.
        set_at(&mut state, "/channels/ch1/solo", json!("pfl"));
        assert_eq!(masks(&shape, &state)[0][7..], [0, 0]);
        set_at(&mut state, "/mixMinus/mm1/interrupt/active", json!(true));
        assert_eq!(masks(&shape, &state)[0][7..], [0, 0]);
    }

    /// A destination is an element, so a group ducking pgm lowers every
    /// column that carries it: the folded stereo output as well as the
    /// 6-leg one, on each leg of a target.
    #[test]
    fn a_duck_group_lowers_the_folded_output_of_its_destination() {
        let shape = Shape::parse(&crate::shape::tests::surround()).unwrap();
        let mut state = shape.initial_state();
        with(
            &mut state,
            &[
                ("/ducks/comm/channels/sur/target", json!(true)),
                ("/ducks/comm/destinations/pgm", json!(true)),
            ],
        );
        // Rows ch1, ch2, st1 L R, sur x6, the line-up tone's generators
        // x1, x2 and x6, which no group lowers; columns out-51 x6, out-lr
        // x2, then the meters of cf x2, aux1 and mon x2, which follows pgm.
        let m = masks(&shape, &state);
        assert_eq!(m[4..10], [[1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 1, 1]; 6]);
        assert_eq!(m[10..], [[0; 13]; 9]);
        assert!(m[..4].iter().flatten().all(|&b| b == 0));
        assert_eq!(columns(&shape), (0..13).collect::<Vec<_>>());
    }

    /// Without duck groups nothing is masked and no column is ducked, so
    /// such a shape costs the engine nothing; with one, every column but
    /// a direct out's may be.
    #[test]
    fn duck_columns_are_every_column_but_a_direct_outs() {
        let mut v = small();
        v["outputs"].as_array_mut().unwrap().insert(
            0,
            json!({ "id": "do", "name": "DO", "source": "ch1", "width": 1, "layout": "mono" }),
        );
        let shape = Shape::parse(&v).unwrap();
        assert_eq!(columns(&shape), (1..10).collect::<Vec<_>>());
        v.as_object_mut().unwrap().remove("ducks");
        let shape = Shape::parse(&v).unwrap();
        assert!(columns(&shape).is_empty());
        assert!(masks(&shape, &shape.initial_state()).is_empty());
        assert!(targets(&shape, &shape.initial_state()).is_empty());
    }

    /// A trigger keys at its tap: pre-fader whatever the fader; pre-fader
    /// while open only with the fader open, unmuted and no VCA mute;
    /// post-fader at its fader, mute and VCA gain.
    #[test]
    fn a_key_takes_the_gain_of_its_tap() {
        let (_, mut state) = two_groups();
        let at = |state: &Value, tap| key_gain(state, "ch2", tap);
        // The initial fader is closed.
        assert_eq!(
            [at(&state, "preFader"), at(&state, "preFaderOpen")],
            [1.0, 0.0]
        );
        assert_eq!(at(&state, "postFader"), 0.0);
        set_at(&mut state, "/channels/ch2/fader", json!(-6));
        assert_eq!(at(&state, "preFaderOpen"), 1.0);
        let minus_6 = mix::gain(-6.0) as f32;
        assert_eq!(at(&state, "postFader").to_bits(), minus_6.to_bits());
        for (path, closing) in [
            ("/channels/ch2/mute", json!(true)),
            ("/vcas/vca1/mute", json!(true)),
        ] {
            let mut closed = state.clone();
            set_at(&mut closed, "/vcas/vca1/members", json!(["ch2"]));
            set_at(&mut closed, path, closing);
            assert_eq!(at(&closed, "preFader"), 1.0, "{path}");
            assert_eq!(at(&closed, "preFaderOpen"), 0.0, "{path}");
            assert_eq!(at(&closed, "postFader"), 0.0, "{path}");
        }
        // A VCA offset moves the post-fader key, not the open one.
        set_at(&mut state, "/vcas/vca1/members", json!(["ch2"]));
        set_at(&mut state, "/vcas/vca1/offset", json!(-6));
        let minus_12 = mix::gain(-12.0) as f32;
        assert_eq!(at(&state, "postFader").to_bits(), minus_12.to_bits());
        assert_eq!(at(&state, "preFaderOpen"), 1.0);
    }

    /// A group's targets: its detector as the state sets it, a key per
    /// trigger in shape order at its tap's gain, the depth of each target
    /// slot and 0 dB for every other slot.
    #[test]
    fn a_group_gives_its_detector_keys_and_depths() {
        let (shape, mut state) = two_groups();
        with(
            &mut state,
            &[
                ("/ducks/comm/threshold", json!(-24.5)),
                ("/ducks/comm/hysteresis", json!(3)),
                (
                    "/ducks/comm/keyHpf",
                    json!({ "enabled": false, "freq": 200 }),
                ),
                (
                    "/ducks/comm/keyLpf",
                    json!({ "enabled": true, "freq": 6000 }),
                ),
                ("/ducks/comm/attackMs", json!(10)),
                ("/ducks/comm/holdMs", json!(300)),
                ("/ducks/comm/releaseMs", json!(800)),
                ("/ducks/comm/keyTap", json!("preFaderOpen")),
                ("/ducks/comm/channels/st1/trigger", json!(true)),
                ("/ducks/comm/channels/ch1/trigger", json!(true)),
                ("/channels/ch1/fader", json!(0)),
                ("/ducks/comm/channels/ch2/target", json!(true)),
                ("/ducks/comm/channels/ch2/depth", json!(15)),
                ("/ducks/comm/channels/st1/depth", json!(20)),
            ],
        );
        let t = targets(&shape, &state);
        assert_eq!(t.len(), 2);
        let p = t[0].params;
        assert_eq!(
            (
                p.threshold_db,
                p.hysteresis_db,
                p.attack_ms,
                p.hold_ms,
                p.release_ms
            ),
            (-24.5, 3.0, 10.0, 300.0, 800.0)
        );
        assert_eq!(
            (p.hpf.enabled, p.hpf.freq, p.lpf.enabled, p.lpf.freq),
            (false, 200.0, true, 6000.0)
        );
        // ch1 open, st1 closed: slots in shape order.
        assert_eq!(t[0].keys, [(0, 1.0), (2, 0.0)]);
        // st1 is no target, so its depth is not the engine's.
        assert_eq!(t[0].depths, [0.0, 15.0, 0.0]);
        // The other group as it starts: samplez-dsp's defaults.
        assert_eq!(t[1].params, DetectorParams::DEFAULT);
        assert!(t[1].keys.is_empty());
    }

    /// Mode and hold go to the engine as they are, whose detector decides
    /// by them: a held group ducks whatever the mode, on ducks whatever
    /// the key, off bypasses.
    #[test]
    fn mode_and_hold_go_to_the_engine_as_set() {
        let (shape, mut state) = two_groups();
        for (mode, want) in [
            ("auto", DuckMode::Auto),
            ("on", DuckMode::On),
            ("off", DuckMode::Off),
        ] {
            for hold in [false, true] {
                set_at(&mut state, "/ducks/comm/mode", json!(mode));
                set_at(&mut state, "/ducks/comm/hold", json!(hold));
                let t = &targets(&shape, &state)[0];
                assert_eq!((t.mode, t.held), (want, hold), "{mode} {hold}");
            }
        }
    }

    /// The schema bounds a duck group by samplez-dsp's `duck` ranges and
    /// its detector count, so no value the state takes is one the engine
    /// refuses.
    #[test]
    fn the_schema_takes_samplez_dsps_duck_ranges() {
        let model: Value =
            serde_json::from_str(include_str!("../api/ws/model.schema.json")).unwrap();
        let defs = &model["$defs"];
        for (def, samplez) in [
            ("DuckThreshold", (-60.0, 0.0)),
            ("DuckHysteresis", (0.0, 12.0)),
            ("HpfFreq", (20.0, 500.0)),
            ("DuckKeyLpfFreq", (1000.0, 20_000.0)),
            ("DuckAttackMs", (1.0, 500.0)),
            ("DuckHoldMs", (0.0, 5000.0)),
            ("DuckReleaseMs", (10.0, 10_000.0)),
            ("DuckDepth", (0.0, 40.0)),
        ] {
            let range = (defs[def]["minimum"].as_f64(), defs[def]["maximum"].as_f64());
            assert_eq!(range, (Some(samplez.0), Some(samplez.1)), "{def}");
        }
        assert_eq!(defs["Shape"]["properties"]["ducks"]["maxItems"], 8);
    }
}
