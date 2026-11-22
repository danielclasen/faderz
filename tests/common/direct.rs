//! A direct out session the single-replica and the set audio tests both
//! run: two mono channels reading one pulse, each with a direct out,
//! trimmed, a low shelf on and delayed alike, one direct out at the
//! input tap and the other at preFader.

use serde_json::{Value, json};

/// The sample rate.
pub const RATE: u64 = 48_000;
/// What the source writes during a pulse.
pub const PULSE: f32 = 0.5;
/// The channels' delay: off the block grid.
pub const DELAY: u64 = RATE / 100 + 7;
/// The channels' trim and their low shelf's gain, dB.
pub const TRIM_DB: f32 = -6.0;
pub const SHELF_DB: f32 = -6.0;
/// The outputs: the direct out at the input tap, then at preFader.
pub const OUTPUTS: [&str; 2] = ["do-in", "do-pre"];

/// Channels `in` and `pre`, each carried by its direct out, and a bus
/// no output carries.
pub fn shape() -> Value {
    let mono =
        |id: &str| json!({ "id": id, "name": id, "width": 1, "layout": "mono", "altInput": false });
    let direct = |id: &str, source: &str| json!({ "id": id, "name": id, "source": source, "width": 1, "layout": "mono" });
    json!({
        "sampleRate": RATE,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [mono("in"), mono("pre")],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [direct(OUTPUTS[0], "in"), direct(OUTPUTS[1], "pre")],
    })
}

/// Both channels reading `flow`, each with a delay line of 20 ms.
pub fn inputs(flow: &str) -> Value {
    json!({
        "in": { "flow": flow, "maxDelayMs": 20 },
        "pre": { "flow": flow, "maxDelayMs": 20 },
    })
}

/// The taps, then the same trim, low shelf and delay on both channels.
pub fn changes() -> Vec<(&'static str, Value)> {
    let mut changes = vec![
        ("/channels/in/directOut/tap", json!("input")),
        ("/channels/pre/directOut/tap", json!("preFader")),
    ];
    for [trim, eq, shelf, delay] in [
        [
            "/channels/in/trim",
            "/channels/in/eq/enabled",
            "/channels/in/eq/bands/0/gain",
            "/channels/in/delay",
        ],
        [
            "/channels/pre/trim",
            "/channels/pre/eq/enabled",
            "/channels/pre/eq/bands/0/gain",
            "/channels/pre/delay",
        ],
    ] {
        changes.extend([
            (trim, json!(TRIM_DB)),
            (eq, json!(true)),
            (shelf, json!(SHELF_DB)),
            (delay, json!(DELAY)),
        ]);
    }
    changes
}

/// Checks what the two direct outs carry of a pulse on the input over
/// samples `[start, end)`, `at_input` and `pre_fader` being samples
/// `[from, ...)` of each. At the input tap the pulse lands exactly
/// [`DELAY`] late, every sample at the level it was read: neither the
/// trim nor the shelf reach it. At preFader it is as late and carries
/// both, which take a constant to the sum of their gains.
pub fn check(at_input: &[f32], pre_fader: &[f32], from: u64, start: u64, end: u64) {
    let heard = |s: &[f32]| -> Vec<(u64, f32)> {
        (s.iter().enumerate())
            .filter(|(_, v)| v.abs() > 0.05)
            .map(|(i, v)| (from + i as u64, *v))
            .collect()
    };
    let input = heard(at_input);
    let (first, last) = (input.first().map(|h| h.0), input.last().map(|h| h.0));
    assert_eq!(
        (first, last, input.len() as u64),
        (Some(start + DELAY), Some(end - 1 + DELAY), end - start),
        "at the input tap the pulse [{start}, {end}) is heard from {first:?} to {last:?}"
    );
    assert!(
        input.iter().all(|h| h.1 == PULSE),
        "at the input tap the pulse is trimmed or processed: {input:?}"
    );
    let pre = heard(pre_fader);
    assert_eq!(
        pre.first().map(|h| h.0),
        Some(start + DELAY),
        "at preFader the pulse starts late by the delay"
    );
    // Half way through the pulse the shelf has settled to its DC gain.
    let mid = pre_fader[(start + DELAY + (end - start) / 2 - from) as usize];
    let want = PULSE * 10f32.powf((TRIM_DB + SHELF_DB) / 20.0);
    assert!(
        (mid - want).abs() < 0.005,
        "at preFader the pulse reads {mid}, not {want}: trim and shelf"
    );
}
