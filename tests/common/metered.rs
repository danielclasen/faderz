//! A metering session the single-replica and the set audio tests both
//! run: a 1 kHz tone hard left and a constant hard right on the
//! programme bus, its mix-minus and an aux, each at a level of its own,
//! and a duck group keyed by a constant on a channel of its own, with
//! the meter frame values each step gives.

use faderz::meters::Layout;
use faderz::shape::Shape;
use serde_json::{Value, json};

/// The tone's amplitude and frequency: 100 whole periods in a meter
/// window, and a sample on every crest.
pub const TONE: (f32, u64) = (0.5, 1_000);
/// What the dc channel writes into its flow.
pub const DC: f32 = 0.25;
/// What the key channel writes into its flow.
pub const KEY: f32 = 0.5;
/// A level of digital silence, -120 dB.
pub const FLOOR: i16 = -12_000;

pub fn shape() -> Value {
    let mono =
        |id: &str| json!({ "id": id, "name": id, "width": 1, "layout": "mono", "altInput": false });
    json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [mono("tone"), mono("dc"), mono("key"), mono("bed")],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [{ "id": "aux1", "name": "IFB 1", "width": 2, "layout": "stereo" }],
        "mixMinus": [{ "id": "mm1", "name": "N-1", "bus": "pgm", "width": 2, "layout": "stereo" }],
        "vcas": [], "monitorOutputs": [],
        "ducks": [{ "id": "talk", "name": "Talk" }],
        // aux1 has no output: it is metered on an engine meter of its own.
        "outputs": [
            { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
            { "id": "out-mm", "name": "N-1 out", "source": "mm1", "width": 2, "layout": "stereo" },
        ],
    })
}

pub struct Step {
    pub what: &'static str,
    pub changes: Vec<(&'static str, Value)>,
    /// Index in the frame's values, value and tolerance, centi-units.
    pub values: Vec<(usize, i16, i16)>,
}

/// The session: the tone and the constant at -6 dB on the bus, unity on
/// the mix-minus, which is before the bus master, and -12 dB on the aux
/// sends, each element read on its peak, true peak, RMS and momentary
/// loudness; then the duck group keyed below, above and below its
/// threshold again.
pub fn steps() -> Vec<Step> {
    let shape = Shape::parse(&shape()).unwrap();
    let layout = Layout::of(&shape);
    let at = |id: &str| layout.offset(id).unwrap();
    // mm1 is the last element, 3n + 8 values; the duck group's key,
    // envelope and flags follow.
    let duck = at("mm1") + 3 * 2 + 8;
    let (tone, dc) = (20.0 * TONE.0.log10(), 20.0 * DC.log10());
    let centi = |db: f32| (db * 100.0).round() as i16;
    // The RMS is referenced to a sine (AES17): a sine reads its peak, a
    // constant 3.01 dB above it. The K-weighting is 0 dB at 1 kHz and
    // takes out a constant: the momentary loudness is that of the tone
    // on one leg, 3.01 dB below its peak.
    let element = |id: &str, db: f32| {
        let (l, r) = (centi(tone + db), centi(dc + db));
        let e = at(id);
        vec![
            (e, l, 5),
            (e + 1, r, 5),
            (e + 2, l, 10),
            (e + 3, r, 10),
            (e + 4, l, 5),
            (e + 5, centi(dc + db + 3.0103), 5),
            (e + 6, centi(tone + db - 3.0103), 10),
        ]
    };
    let mut buses = element("pgm", -6.0);
    buses.extend(element("mm1", 0.0));
    buses.extend(element("aux1", -12.0));
    let key = |db: f32| centi(20.0 * KEY.log10() + db);
    vec![
        Step {
            what: "the tone and the constant on the bus, mix-minus and aux",
            changes: vec![
                ("/channels/tone/fader", json!(0)),
                ("/channels/tone/pan/lr", json!(-1)),
                ("/channels/dc/fader", json!(0)),
                ("/channels/dc/pan/lr", json!(1)),
                ("/channels/dc/sends/aux1/on", json!(true)),
                ("/channels/dc/sends/aux1/level", json!(-12)),
                ("/channels/dc/sends/aux1/pan", json!(1)),
                ("/buses/pgm/fader", json!(-6)),
                ("/channels/tone/sends/aux1/on", json!(true)),
                ("/channels/tone/sends/aux1/level", json!(-12)),
                ("/channels/tone/sends/aux1/pan", json!(-1)),
            ],
            values: buses,
        },
        // Keyed after the key's fader, off every bus, and ducking a
        // silent channel: the tone's meters stay as they are.
        Step {
            what: "a key 6 dB under the threshold",
            changes: vec![
                ("/channels/key/fader", json!(-24)),
                ("/channels/key/routes/pgm", json!(false)),
                ("/ducks/talk/channels/key/trigger", json!(true)),
                ("/ducks/talk/channels/bed/target", json!(true)),
                ("/ducks/talk/keyTap", json!("postFader")),
                ("/ducks/talk/keyHpf/enabled", json!(false)),
                ("/ducks/talk/threshold", json!(-24)),
                ("/ducks/talk/attackMs", json!(10)),
                ("/ducks/talk/holdMs", json!(50)),
                ("/ducks/talk/releaseMs", json!(100)),
            ],
            values: vec![(duck, key(-24.0), 5), (duck + 1, 0, 0), (duck + 2, 0, 0)],
        },
        Step {
            what: "a key over the threshold",
            changes: vec![("/channels/key/fader", json!(0))],
            values: vec![(duck, key(0.0), 5), (duck + 1, 10_000, 0), (duck + 2, 1, 0)],
        },
        Step {
            what: "the key under the threshold again",
            changes: vec![("/channels/key/fader", json!(-24))],
            values: vec![(duck, key(-24.0), 5), (duck + 1, 0, 0), (duck + 2, 0, 0)],
        },
    ]
}
