//! A channel's processing in the live state as the parameters of its
//! input's strip in the engine (`samplez_dsp::strip`): trim, leg trim,
//! polarity, high-pass, the four EQ bands, gate, compressor and their
//! order. The model's ranges are the strip's. Delay is kept and not
//! applied.

use samplez_dsp::{BandType, CompParams, EqBand, GateParams, StripParam};
use serde_json::Value;

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
    p.push(StripParam::Eq {
        enabled: on(&c["eq"]["enabled"]),
    });
    for band in 0..samplez_dsp::strip::EQ_BANDS {
        let b = &c["eq"]["bands"][band];
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
    let g = &c["gate"];
    p.push(StripParam::Gate(GateParams {
        enabled: on(&g["enabled"]),
        threshold_db: num(&g["threshold"]),
        range_db: num(&g["range"]),
        attack_ms: num(&g["attack"]),
        release_ms: num(&g["release"]),
    }));
    let k = &c["comp"];
    p.push(StripParam::Comp(CompParams {
        enabled: on(&k["enabled"]),
        threshold_db: num(&k["threshold"]),
        ratio: num(&k["ratio"]),
        attack_ms: num(&k["attack"]),
        release_ms: num(&k["release"]),
        knee_db: KNEE_DB,
        makeup_db: num(&k["makeup"]),
    }));
    p.push(StripParam::DynamicsBeforeEq {
        enabled: on(&c["dynamicsBeforeEq"]),
    });
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::Shape;
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
        c["comp"] = json!({ "enabled": true, "threshold": -60, "ratio": 20, "attack": 200, "release": 5, "makeup": 24 });
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
            })
        );
        assert_eq!(p[13], StripParam::DynamicsBeforeEq { enabled: true });
        for q in p {
            q.check(2).unwrap_or_else(|e| panic!("{q:?}: {e}"));
        }
    }
}
