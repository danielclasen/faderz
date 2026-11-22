//! A channel's processing in the live state as the parameters of its
//! input's strip in the engine (`samplez_dsp::strip`): trim, leg trim,
//! polarity, high-pass, the four EQ bands, gate, compressor and their
//! order. The model's ranges are the strip's. Also each channel's input
//! delay and a tone channel's generator, and each output's strip and
//! limiter, from the bus, aux or mix-minus output it carries.

use samplez_dsp::{
    BandType, CompParams, EqBand, GateParams, LimiterParams, StripParam, ToneParams,
};
use serde_json::Value;

use crate::shape::Shape;

/// faderz has no compressor knee control: a hard knee.
const KNEE_DB: f32 = 0.0;

fn num(v: &Value) -> f32 {
    v.as_f64().unwrap_or(0.0) as f32
}

fn on(v: &Value) -> bool {
    v.as_bool().unwrap_or(false)
}

/// The strip parameters of channel state `c` (model `ChannelState`) of a
/// channel `width` legs wide, always the same parameters in the same
/// order, so two lists compare entry by entry.
pub fn params(c: &Value, width: usize) -> Vec<StripParam> {
    let mut p = vec![StripParam::Trim {
        db: num(&c["trim"]),
    }];
    p.extend((0..width).map(|leg| StripParam::LegTrim {
        leg,
        db: num(&c["legTrim"][leg]),
    }));
    p.extend((0..width).map(|leg| StripParam::Polarity {
        leg,
        inverted: on(&c["polarity"][leg]),
    }));
    p.push(StripParam::Hpf {
        enabled: on(&c["hpf"]["enabled"]),
        freq: num(&c["hpf"]["freq"]),
    });
    p.extend(processing(c));
    p
}

/// The EQ, gate, compressor and their order of `e`, a channel's state
/// or the state of a bus, aux or mix-minus output (model `BusState`,
/// `MixMinusState`), always the same parameters in the same order.
fn processing(e: &Value) -> Vec<StripParam> {
    let mut p = vec![StripParam::Eq {
        enabled: on(&e["eq"]["enabled"]),
    }];
    for band in 0..samplez_dsp::strip::EQ_BANDS {
        let b = &e["eq"]["bands"][band];
        let kind = match b["type"].as_str() {
            Some("lowShelf") => BandType::LowShelf,
            Some("highShelf") => BandType::HighShelf,
            _ => BandType::Peak,
        };
        p.push(StripParam::EqBand {
            band,
            value: EqBand {
                kind,
                freq: num(&b["freq"]),
                gain_db: num(&b["gain"]),
                q: num(&b["q"]),
            },
        });
    }
    let g = &e["gate"];
    p.push(StripParam::Gate(GateParams {
        enabled: on(&g["enabled"]),
        threshold_db: num(&g["threshold"]),
        range_db: num(&g["range"]),
        attack_ms: num(&g["attack"]),
        release_ms: num(&g["release"]),
    }));
    let k = &e["comp"];
    p.push(StripParam::Comp(CompParams {
        enabled: on(&k["enabled"]),
        threshold_db: num(&k["threshold"]),
        ratio: num(&k["ratio"]),
        attack_ms: num(&k["attack"]),
        release_ms: num(&k["release"]),
        knee_db: KNEE_DB,
        makeup_db: num(&k["makeup"]),
        // Percent; a state saved before the mix holds none: the compressor alone.
        mix: (k["mix"].as_f64().unwrap_or(100.0) / 100.0) as f32,
    }));
    p.push(StripParam::DynamicsBeforeEq {
        enabled: on(&e["dynamicsBeforeEq"]),
    });
    p
}

/// The delay of channel state `c` (model `ChannelState`) or of a bus's
/// or aux's state (model `BusState`), in samples. A write is refused past the channel's delay line
/// ([`crate::shape::ChannelShape::max_delay`]), so the engine takes it.
pub fn delay(c: &Value) -> u32 {
    c["delay"].as_u64().unwrap_or(0) as u32
}

/// The tone generator parameters of a tone channel's state `c` (model
/// `ChannelState`), whose `tone` the contract keeps within samplez's
/// ranges, so the engine takes them.
pub fn tone(c: &Value) -> ToneParams {
    let t = &c["tone"];
    ToneParams {
        frequency_hz: t["frequency"].as_f64().unwrap_or(0.0) as u32,
        level_db: num(&t["level"]),
        ident: on(&t["ident"]),
    }
}

/// The state of the bus, aux or mix-minus output `id` in `state`; null
/// for any other element.
fn element<'s>(state: &'s Value, id: &str) -> &'s Value {
    (["buses", "auxes", "mixMinus"].into_iter())
        .map(|group| &state[group][id])
        .find(|e| !e.is_null())
        .unwrap_or(&Value::Null)
}

/// Per output of `shape`, the strip of the bus, aux or mix-minus output
/// it carries in `state` ([`processing`]): EQ, gate, compressor and
/// their order, after the element's fader and before the output's
/// limiter. Empty for an output carrying anything else, which has no
/// strip in the engine ([`Shape::has_strip`]). A bus's polarity is not
/// in it: the gain matrix applies it per leg of the bus, before an
/// output folds the bus down.
pub fn output_strips(shape: &Shape, state: &Value) -> Vec<Vec<StripParam>> {
    (shape.outputs.iter())
        .map(|o| {
            if shape.has_strip(&o.source) {
                processing(element(state, &o.source))
            } else {
                Vec::new()
            }
        })
        .collect()
}

/// Per output of `shape`, the delay of the bus or aux it carries in
/// `state`, in samples, after its strip and before its limiter; 0 for an
/// output carrying anything else. A write is refused past the element's
/// delay line ([`Shape::delay_line`]), the line of every output carrying
/// it, so the engine takes it.
pub fn output_delays(shape: &Shape, state: &Value) -> Vec<u32> {
    (shape.outputs.iter())
        .map(|o| {
            if shape.has_limiter(&o.source) {
                delay(element(state, &o.source))
            } else {
                0
            }
        })
        .collect()
}

/// Per output of `shape`, the limiter of the bus or aux it carries in
/// `state`; none for an output carrying anything else, which has no
/// limiter in the engine ([`Shape::has_limiter`]). The model has no release
/// control: the engine's default release.
pub fn limiters(shape: &Shape, state: &Value) -> Vec<Option<LimiterParams>> {
    (shape.outputs.iter())
        .map(|o| {
            shape.has_limiter(&o.source).then(|| {
                let l = &element(state, &o.source)["limiter"];
                LimiterParams {
                    enabled: on(&l["enabled"]),
                    ceiling_db: num(&l["ceiling"]),
                    // Stated, not the engine's default: a default that
                    // moved would change what one state derives.
                    release_ms: 50.0,
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::tests::small;
    use serde_json::json;

    /// Every parameter of the initial state is one the strip takes.
    #[test]
    fn the_initial_state_is_a_valid_strip() {
        let shape = Shape::parse(&small()).unwrap();
        let state = shape.initial_state();
        for c in &shape.channels {
            let p = params(&state["channels"][&c.id], c.width);
            assert_eq!(p.len(), 1 + 2 * c.width + 2 + 4 + 3);
            for q in p {
                q.check(c.width)
                    .unwrap_or_else(|e| panic!("{}: {q:?}: {e}", c.id));
            }
        }
    }

    /// Each control of the model lands in its parameter, at the model's
    /// extremes too.
    #[test]
    fn the_controls_map_to_their_parameters() {
        let shape = Shape::parse(&small()).unwrap();
        let mut s = shape.initial_state();
        let c = &mut s["channels"]["st1"];
        c["trim"] = json!(-20);
        c["legTrim"] = json!([20, -3.5]);
        c["polarity"] = json!([false, true]);
        c["hpf"] = json!({ "enabled": true, "freq": 500 });
        c["eq"]["enabled"] = json!(true);
        c["eq"]["bands"][3] = json!({ "type": "highShelf", "freq": 20000, "gain": -18, "q": 0.1 });
        c["gate"] = json!({ "enabled": true, "threshold": -80, "range": 80, "attack": 0.1, "release": 5000 });
        c["comp"] = json!({ "enabled": true, "threshold": -60, "ratio": 20, "attack": 200, "release": 5, "makeup": 24, "mix": 37.5 });
        c["dynamicsBeforeEq"] = json!(true);
        let p = params(c, 2);
        assert_eq!(p[0], StripParam::Trim { db: -20.0 });
        assert_eq!(p[1], StripParam::LegTrim { leg: 0, db: 20.0 });
        assert_eq!(p[2], StripParam::LegTrim { leg: 1, db: -3.5 });
        assert_eq!(
            p[3],
            StripParam::Polarity {
                leg: 0,
                inverted: false
            }
        );
        assert_eq!(
            p[4],
            StripParam::Polarity {
                leg: 1,
                inverted: true
            }
        );
        assert_eq!(
            p[5],
            StripParam::Hpf {
                enabled: true,
                freq: 500.0
            }
        );
        assert_eq!(p[6], StripParam::Eq { enabled: true });
        assert_eq!(
            p[7],
            StripParam::EqBand {
                band: 0,
                value: EqBand {
                    kind: BandType::LowShelf,
                    freq: 100.0,
                    gain_db: 0.0,
                    q: 0.7
                },
            }
        );
        assert_eq!(
            p[10],
            StripParam::EqBand {
                band: 3,
                value: EqBand {
                    kind: BandType::HighShelf,
                    freq: 20_000.0,
                    gain_db: -18.0,
                    q: 0.1
                },
            }
        );
        assert_eq!(
            p[11],
            StripParam::Gate(GateParams {
                enabled: true,
                threshold_db: -80.0,
                range_db: 80.0,
                attack_ms: 0.1,
                release_ms: 5000.0,
            })
        );
        assert_eq!(
            p[12],
            StripParam::Comp(CompParams {
                enabled: true,
                threshold_db: -60.0,
                ratio: 20.0,
                attack_ms: 200.0,
                release_ms: 5.0,
                knee_db: 0.0,
                makeup_db: 24.0,
                mix: 0.375,
            })
        );
        assert_eq!(p[13], StripParam::DynamicsBeforeEq { enabled: true });
        for q in p {
            q.check(2).unwrap_or_else(|e| panic!("{q:?}: {e}"));
        }
    }

    /// An output carrying a bus or an aux takes that element's limiter,
    /// whichever output reads it; an output carrying a mix-minus or a
    /// monitor output has none.
    #[test]
    fn each_output_takes_its_elements_limiter() {
        let mut v = small();
        v["outputs"] = json!([
            { "id": "out-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
            { "id": "out-pgm-m", "name": "PGM mono", "source": "pgm", "width": 1, "layout": "mono" },
            { "id": "out-aux", "name": "IFB", "source": "aux1", "width": 1, "layout": "mono" },
            { "id": "out-mm", "name": "N-1", "source": "mm1", "width": 2, "layout": "stereo" },
            { "id": "out-mon", "name": "Mon", "source": "mon", "width": 2, "layout": "stereo" },
            { "id": "out-cf", "name": "Clean", "source": "cf", "width": 2, "layout": "stereo" },
        ]);
        let shape = Shape::parse(&v).unwrap();
        let mut state = shape.initial_state();
        state["buses"]["pgm"]["limiter"] = json!({ "enabled": true, "ceiling": -10 });
        state["auxes"]["aux1"]["limiter"] = json!({ "enabled": true, "ceiling": -2.5 });
        let limiter = |enabled, ceiling_db| {
            Some(LimiterParams {
                enabled,
                ceiling_db,
                release_ms: 50.0,
            })
        };
        assert_eq!(
            limiters(&shape, &state),
            vec![
                limiter(true, -10.0),
                limiter(true, -10.0),
                limiter(true, -2.5),
                None,
                None,
                limiter(false, -1.0),
            ]
        );
        for p in limiters(&shape, &state).into_iter().flatten() {
            p.check().unwrap_or_else(|e| panic!("{p:?}: {e}"));
        }
    }

    /// An output carrying a bus, an aux or a mix-minus output takes that
    /// element's EQ, gate, compressor and their order, whichever output
    /// reads it, a narrower one included; an output carrying the monitor
    /// has no strip. A bus's polarity is not in it.
    #[test]
    fn each_output_takes_its_elements_strip() {
        let mut v = small();
        v["outputs"] = json!([
            { "id": "out-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
            { "id": "out-pgm-m", "name": "PGM mono", "source": "pgm", "width": 1, "layout": "mono" },
            { "id": "out-aux", "name": "IFB", "source": "aux1", "width": 1, "layout": "mono" },
            { "id": "out-mm", "name": "N-1", "source": "mm1", "width": 2, "layout": "stereo" },
            { "id": "out-mon", "name": "Mon", "source": "mon", "width": 2, "layout": "stereo" },
            { "id": "out-cf", "name": "Clean", "source": "cf", "width": 2, "layout": "stereo" },
        ]);
        let shape = Shape::parse(&v).unwrap();
        let mut state = shape.initial_state();
        state["buses"]["pgm"]["polarity"] = json!([true, false]);
        state["buses"]["pgm"]["eq"]["enabled"] = json!(true);
        state["buses"]["pgm"]["eq"]["bands"][1]["gain"] = json!(6);
        state["auxes"]["aux1"]["comp"]["enabled"] = json!(true);
        state["auxes"]["aux1"]["comp"]["ratio"] = json!(8);
        state["mixMinus"]["mm1"]["gate"]["enabled"] = json!(true);
        state["mixMinus"]["mm1"]["dynamicsBeforeEq"] = json!(true);
        let strips = output_strips(&shape, &state);
        let of = processing;
        assert_eq!(strips[0], of(&state["buses"]["pgm"]));
        assert_eq!(strips[0][0], StripParam::Eq { enabled: true });
        assert_eq!(
            strips[0][2],
            StripParam::EqBand {
                band: 1,
                value: EqBand {
                    kind: BandType::Peak,
                    freq: 400.0,
                    gain_db: 6.0,
                    q: 1.0
                },
            }
        );
        assert_eq!(strips[1], strips[0], "the mono fold-down takes the bus's");
        assert_eq!(strips[2], of(&state["auxes"]["aux1"]));
        assert_eq!(strips[3], of(&state["mixMinus"]["mm1"]));
        assert_eq!(strips[4], Vec::new(), "the monitor output has none");
        assert_eq!(strips[5], of(&state["buses"]["cf"]));
        let off = of(&shape.initial_state()["buses"]["cf"]);
        for (n, s) in strips.iter().enumerate().filter(|(n, _)| *n != 4) {
            assert_eq!(s.len(), 8, "output {n}: EQ, 4 bands, gate, comp, order");
            assert_eq!(*s != off, n != 5, "output {n}");
            assert!(
                !s.iter().any(|p| matches!(p, StripParam::Polarity { .. })),
                "output {n}"
            );
        }
        assert_eq!(
            strips[3].last(),
            Some(&StripParam::DynamicsBeforeEq { enabled: true })
        );
    }

    /// Every output carrying a bus or an aux takes that element's delay,
    /// a narrower one included; an output carrying a mix-minus output or
    /// the monitor takes none.
    #[test]
    fn each_output_takes_its_elements_delay() {
        let mut v = small();
        v["outputs"] = json!([
            { "id": "out-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
            { "id": "out-pgm-m", "name": "PGM mono", "source": "pgm", "width": 1, "layout": "mono" },
            { "id": "out-aux", "name": "IFB", "source": "aux1", "width": 1, "layout": "mono" },
            { "id": "out-mm", "name": "N-1", "source": "mm1", "width": 2, "layout": "stereo" },
            { "id": "out-mon", "name": "Mon", "source": "mon", "width": 2, "layout": "stereo" },
            { "id": "out-cf", "name": "Clean", "source": "cf", "width": 2, "layout": "stereo" },
        ]);
        let shape = Shape::parse(&v).unwrap();
        let mut state = shape.initial_state();
        state["buses"]["pgm"]["delay"] = json!(960);
        state["auxes"]["aux1"]["delay"] = json!(48);
        assert_eq!(output_delays(&shape, &state), [960, 960, 48, 0, 0, 0]);
    }

    /// Every EQ, gate and compressor value the state takes for a bus, an
    /// aux or a mix-minus output is one the output strip takes, at the
    /// model's extremes, at every output width: no stored value is
    /// refused by the engine for good.
    #[test]
    fn every_output_strip_value_the_state_takes_the_engine_takes() {
        let model: Value =
            serde_json::from_str(include_str!("../api/ws/model.schema.json")).unwrap();
        let bound = |def: &str, end: &str| model["$defs"][def][end].as_f64().unwrap();
        let mut v = small();
        v["outputs"] = json!([
            { "id": "out-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
            { "id": "out-pgm-m", "name": "PGM mono", "source": "pgm", "width": 1, "layout": "mono" },
            { "id": "out-aux", "name": "IFB", "source": "aux1", "width": 1, "layout": "mono" },
            { "id": "out-mm", "name": "N-1", "source": "mm1", "width": 2, "layout": "stereo" },
        ]);
        let shape = Shape::parse(&v).unwrap();
        let mixer = crate::state::Mixer::new(shape.clone());
        for end in ["minimum", "maximum"] {
            let mut state = shape.initial_state();
            for e in ["/buses/pgm", "/auxes/aux1", "/mixMinus/mm1"] {
                let mut leaves = vec![
                    ("eq/enabled".to_owned(), json!(true)),
                    ("gate/enabled".to_owned(), json!(true)),
                    ("comp/enabled".to_owned(), json!(true)),
                    ("dynamicsBeforeEq".to_owned(), json!(true)),
                ];
                for band in 0..4 {
                    for (key, def) in [("freq", "EqFreq"), ("gain", "EqGain"), ("q", "EqQ")] {
                        leaves.push((format!("eq/bands/{band}/{key}"), json!(bound(def, end))));
                    }
                }
                for (key, def) in [
                    ("gate/threshold", "GateThreshold"),
                    ("gate/range", "GateRange"),
                    ("gate/attack", "AttackMs"),
                    ("gate/release", "ReleaseMs"),
                    ("comp/threshold", "CompThreshold"),
                    ("comp/ratio", "CompRatio"),
                    ("comp/attack", "AttackMs"),
                    ("comp/release", "ReleaseMs"),
                    ("comp/makeup", "MakeupDb"),
                    ("comp/mix", "CompMix"),
                ] {
                    leaves.push((key.to_owned(), json!(bound(def, end))));
                }
                for (leaf, value) in leaves {
                    let path = format!("{e}/{leaf}");
                    assert!(mixer.check_write(&path, &value).is_ok(), "{path} {value}");
                    *state.pointer_mut(&path).unwrap() = value;
                }
            }
            let strips = output_strips(&shape, &state);
            for (o, params) in shape.outputs.iter().zip(&strips) {
                assert_eq!(params.len(), 8, "{}", o.id);
                for p in params {
                    p.check(o.width)
                        .unwrap_or_else(|e| panic!("{} at the {end}: {p:?}: {e}", o.id));
                }
            }
        }
    }

    /// Every ceiling the state takes is one the engine takes: the
    /// model's bounds lie within samplez's, so no stored value is
    /// refused by the engine for good; one outside them is refused at
    /// the write.
    #[test]
    fn every_ceiling_the_state_takes_the_engine_takes() {
        let model: Value =
            serde_json::from_str(include_str!("../api/ws/model.schema.json")).unwrap();
        let bounds = &model["$defs"]["LimiterCeiling"];
        let (lo, hi) = (
            bounds["minimum"].as_f64().unwrap(),
            bounds["maximum"].as_f64().unwrap(),
        );
        let engine = samplez_dsp::limiter::LIMITER_CEILING_DB;
        for ceiling in [lo, hi] {
            assert!(
                engine.contains(&(ceiling as f32)),
                "{ceiling} outside {engine:?}"
            );
        }
        let shape = Shape::parse(&small()).unwrap();
        let mixer = crate::state::Mixer::new(shape.clone());
        for ceiling in [lo, hi] {
            assert!(
                mixer
                    .check_write("/buses/pgm/limiter/ceiling", &json!(ceiling))
                    .is_ok(),
                "{ceiling}"
            );
        }
        assert!(
            mixer
                .check_write("/buses/pgm/limiter/ceiling", &json!(lo - 0.5))
                .is_err()
        );
    }
}
