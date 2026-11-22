//! A metering session the single-replica and the set audio tests both
//! run: a 1 kHz tone hard left and a constant hard right on the
//! programme bus, its mix-minus and an aux, each at a level of its own,
//! a duck group keyed by a constant on a channel of its own, then the
//! tone alone in the centre with one leg of the bus inverted, then
//! trimmed and read before and after its strip, with the meter frame
//! values each step gives.

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
    /// The elements whose loudness is reset after the changes, once the
    /// signal has held for a short-term window.
    pub resets: &'static [&'static str],
    /// Index in the frame's values, value and tolerance, centi-units.
    pub values: Vec<(usize, i16, i16)>,
    /// The channels whose pre-strip meter runs after the changes: those
    /// at the input meter tap.
    pub pre_strip: &'static [&'static str],
}

/// Whether the engine meters exactly the inputs `step` names before
/// their strip: it reports those readings only while that meter runs.
pub fn pre_strip_runs(status: &samplez::Status, step: &Step) -> bool {
    (status.inputs.iter())
        .all(|i| i.pre_strip_meters.is_some() == step.pre_strip.contains(&i.name.as_str()))
}

/// The session: the tone and the constant at -6 dB on the bus, unity on
/// the mix-minus, which is before the bus master, and -12 dB on the aux
/// sends, each element read on its peak, true peak, RMS and momentary
/// loudness, and the channels, none of which has a direct out, on their
/// true peaks; then the duck group keyed below, above and below its
/// threshold again, the key channel's true peak following its fader;
/// then the correlations of the tone on both legs; then the tone trimmed,
/// read at its pre-fader and at its input meter tap and at its pre-fader
/// tap again. Only a channel at the input tap has its pre-strip meter on.
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
    // A sine and a constant over whole periods do not correlate.
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
            (e + 12, 0, 2),
        ]
    };
    // A true peak is never below the sample peak: from it to 0.1 dB above,
    // read at the channel's meter tap.
    let true_peak = |id: &str, db: f32| (at(id) + 1, centi(db) + 5, 5);
    // A mono channel's peak, true peak and momentary loudness at its meter
    // tap, the tone on its one leg.
    let channel = |id: &str, db: f32| {
        vec![
            (at(id), centi(db), 5),
            true_peak(id, db),
            (at(id) + 2, centi(db - 3.0103), 10),
        ]
    };
    // A constant measured from a reset taken while it held: integrated
    // and short-term loudness as the momentary, no loudness range, the
    // maxima as the present values, the integrated loudness valid and
    // the range not yet stable (that takes 60 s).
    let settled = |id: &str, db: f32| {
        let e = at(id);
        let loudness = centi(tone + db - 3.0103);
        vec![
            (e + 7, loudness, 10),
            (e + 8, loudness, 15),
            (e + 9, 0, 10),
            (e + 10, loudness, 10),
            (e + 11, loudness, 10),
            (e + 13, 1, 0),
        ]
    };
    let mut measured = settled("pgm", -6.0);
    measured.extend(settled("mm1", 0.0));
    measured.extend(settled("aux1", -12.0));
    let mut buses = element("pgm", -6.0);
    buses.extend(element("mm1", 0.0));
    buses.extend(element("aux1", -12.0));
    buses.extend([true_peak("tone", tone), true_peak("dc", dc)]);
    let key_db = |db: f32| 20.0 * KEY.log10() + db;
    let key = |db: f32| centi(key_db(db));
    vec![
        Step {
            what: "the tone and the constant on the bus, mix-minus and aux",
            resets: &[],
            pre_strip: &[],
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
            resets: &[],
            pre_strip: &[],
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
            values: vec![
                (duck, key(-24.0), 5),
                (duck + 1, 0, 0),
                (duck + 2, 0, 0),
                true_peak("key", key_db(-24.0)),
            ],
        },
        Step {
            what: "a key over the threshold",
            resets: &[],
            pre_strip: &[],
            changes: vec![("/channels/key/fader", json!(0))],
            values: vec![
                (duck, key(0.0), 5),
                (duck + 1, 10_000, 0),
                (duck + 2, 1, 0),
                true_peak("key", key_db(0.0)),
            ],
        },
        Step {
            what: "the key under the threshold again",
            resets: &[],
            pre_strip: &[],
            changes: vec![("/channels/key/fader", json!(-24))],
            values: vec![
                (duck, key(-24.0), 5),
                (duck + 1, 0, 0),
                (duck + 2, 0, 0),
                true_peak("key", key_db(-24.0)),
            ],
        },
        Step {
            what: "the loudness measured from a reset while the signal held",
            resets: &["pgm", "mm1", "aux1"],
            pre_strip: &[],
            changes: vec![],
            values: measured,
        },
        // The same tone on both legs of the mix-minus correlates fully;
        // the bus inverts its right leg, which the mix-minus, before the
        // bus master, does not take. The constant's closed post-fader tap
        // reads silence on its true peak too.
        Step {
            what: "the tone centred and the bus's right leg inverted",
            resets: &[],
            pre_strip: &[],
            changes: vec![
                ("/channels/tone/pan/lr", json!(0)),
                ("/channels/dc/mute", json!(true)),
                ("/buses/pgm/polarity/1", json!(true)),
            ],
            values: vec![
                (at("pgm") + 12, -100, 1),
                (at("mm1") + 12, 100, 1),
                (at("dc") + 1, FLOOR, 0),
            ],
        },
        // The trim is the strip's: the pre-fader tap reads it, the input
        // tap, before the strip, does not.
        Step {
            what: "the tone trimmed by 6 dB at its pre-fader meter tap",
            resets: &[],
            pre_strip: &[],
            changes: vec![
                ("/channels/tone/trim", json!(-6)),
                ("/channels/tone/meterTap", json!("preFader")),
            ],
            values: channel("tone", tone - 6.0),
        },
        Step {
            what: "the trimmed tone at its input meter tap",
            resets: &[],
            pre_strip: &["tone"],
            changes: vec![("/channels/tone/meterTap", json!("input"))],
            values: channel("tone", tone),
        },
        Step {
            what: "the trimmed tone back at its pre-fader meter tap",
            resets: &[],
            pre_strip: &[],
            changes: vec![("/channels/tone/meterTap", json!("preFader"))],
            values: channel("tone", tone - 6.0),
        },
    ]
}
