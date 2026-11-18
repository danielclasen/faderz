//! Duck groups in the state: the initial state of each group, the rules a
//! group keeps beyond what the schema says, and the recall-safe group of
//! each of its leaves.
//!
//! Ranges (in the schema) and defaults (here) are those of samplez-dsp's
//! `duck` module: `THRESHOLD_DB`, `HYSTERESIS_DB`, `KEY_HPF_FREQ`,
//! `KEY_LPF_FREQ`, `ATTACK_MS`, `HOLD_MS`, `RELEASE_MS`, `DEPTH_DB`,
//! `MAX_KEYS` and `DetectorParams::DEFAULT`.

use serde_json::{Map, Value, json};

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

#[cfg(test)]
mod tests {
    use serde_json::Value;

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
