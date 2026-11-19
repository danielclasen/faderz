//! A monitor session the single-replica and the set audio tests both
//! run: a stereo channel of constant legs on the programme bus, a 5.1
//! channel of constant legs on a 5.1 bus of its own, and a stereo
//! monitor output, with a sequence of changes to the monitor, each with
//! the legs its output carries after it.

use serde_json::{Value, json};

/// The legs the stereo channel `st` writes into its flow.
pub const ST: [f32; 2] = [0.5, 0.25];
/// The legs the 5.1 channel `sur` writes into its flow: L R C LFE Ls Rs,
/// each a distinct power of two.
pub const SUR: [f32; 6] = [0.25, 0.125, 0.0625, 0.03125, 0.015625, 0.007_812_5];

pub fn shape() -> Value {
    json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "st", "name": "Music", "width": 2, "layout": "stereo", "altInput": false },
            { "id": "sur", "name": "Surround", "width": 6, "layout": "5.1", "altInput": false },
        ],
        "buses": [
            { "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" },
            { "id": "s51", "name": "Clean 5.1", "kind": "cleanFeed", "width": 6, "layout": "5.1" },
        ],
        "auxes": [], "mixMinus": [], "vcas": [],
        "monitorOutputs": [{ "id": "mon", "name": "Monitor", "width": 2, "layout": "stereo" }],
        "outputs": [
            { "id": "out-mon", "name": "Mon out", "source": "mon", "width": 2, "layout": "stereo" },
        ],
    })
}

pub struct Step {
    pub what: &'static str,
    pub changes: Vec<(&'static str, Value)>,
    /// What the monitor output carries once they are applied.
    pub legs: [f32; 2],
}

/// The session: source, level, dim and its depth, cut, mono, speaker
/// mute and solo, and the downmix of a wider source, each step changing
/// what the monitor plays.
pub fn steps() -> Vec<Step> {
    let g = |db: f32| 10f32.powf(db / 20.0);
    let scaled = |by: f32| ST.map(|v| v * by);
    let h = std::f32::consts::FRAC_1_SQRT_2;
    // BS.775 Lo/Ro: centre and surrounds at -3 dB, the LFE dropped.
    let lo = SUR[0] + h * SUR[2] + h * SUR[4];
    let ro = SUR[1] + h * SUR[2] + h * SUR[5];
    // The monitor's mono sums the legs, lowered by its compensation.
    let mono = (ST[0] + ST[1]) * g(-3.0);
    let step = |what, changes: &[(&'static str, Value)], legs| Step {
        what,
        changes: changes.to_vec(),
        legs,
    };
    vec![
        step(
            "the programme at unity",
            &[
                ("/channels/st/fader", json!(0)),
                ("/channels/st/routes/s51", json!(false)),
                ("/channels/sur/fader", json!(0)),
                ("/channels/sur/routes/pgm", json!(false)),
                ("/monitor/level", json!(0)),
            ],
            ST,
        ),
        step(
            "the monitor level",
            &[("/monitor/level", json!(-6))],
            scaled(g(-6.0)),
        ),
        step(
            "dimmed by its default depth",
            &[("/monitor/level", json!(0)), ("/monitor/dim", json!(true))],
            scaled(g(-20.0)),
        ),
        step(
            "the dim depth",
            &[("/monitor/dimDepth", json!(10))],
            scaled(g(-10.0)),
        ),
        step(
            "cut",
            &[
                ("/monitor/dim", json!(false)),
                ("/monitor/cut", json!(true)),
            ],
            [0.0; 2],
        ),
        step(
            "mono, compensated",
            &[
                ("/monitor/cut", json!(false)),
                ("/monitor/mono", json!(true)),
            ],
            [mono; 2],
        ),
        step(
            "the right speaker muted",
            &[
                ("/monitor/mono", json!(false)),
                ("/monitor/speakerMute/1", json!(true)),
            ],
            [ST[0], 0.0],
        ),
        step(
            "the right speaker soloed",
            &[
                ("/monitor/speakerMute/1", json!(false)),
                ("/monitor/speakerSolo/1", json!(true)),
            ],
            [0.0, ST[1]],
        ),
        step(
            "the 5.1 bus folded down Lo/Ro",
            &[
                ("/monitor/speakerSolo/1", json!(false)),
                ("/monitor/source", json!("s51")),
            ],
            [lo, ro],
        ),
        step(
            "the 5.1 bus's L and R alone",
            &[("/monitor/downmix", json!("lr"))],
            [SUR[0], SUR[1]],
        ),
        step(
            "the programme again",
            &[("/monitor/source", json!("pgm"))],
            ST,
        ),
    ]
}
