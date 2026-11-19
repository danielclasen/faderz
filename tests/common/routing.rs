//! A routing session the single-replica and the set audio tests both
//! run: two mono channels of constant signals, ch1 hard left and ch2
//! hard right, so each leg of a stereo element carries one of them; an
//! aux, a mix-minus, a VCA and ch1's direct out; and a sequence of
//! changes, each with the legs every output carries after it.

use std::time::Duration;

use serde_json::{Value, json};

/// How long after a change returns an output is first read: longer than
/// the readers stay behind the clock and the engine's gain ramp, so a
/// step that leaves an output alone reads it after the change, not
/// before.
pub const SETTLE: Duration = Duration::from_millis(300);

/// What ch1 and ch2 write into their flows.
pub const CH1: f32 = 0.5;
pub const CH2: f32 = 0.125;

pub fn shape() -> Value {
    json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "ch1", "name": "Pres 1", "width": 1, "layout": "mono", "altInput": false },
            { "id": "ch2", "name": "Pres 2", "width": 1, "layout": "mono", "altInput": false },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [{ "id": "aux1", "name": "IFB 1", "width": 2, "layout": "stereo" }],
        "mixMinus": [{ "id": "mm1", "name": "N-1", "bus": "pgm", "width": 2, "layout": "stereo" }],
        "vcas": [{ "id": "vca1", "name": "Pres" }],
        "monitorOutputs": [],
        "outputs": [
            { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
            { "id": "out-mm", "name": "N-1 out", "source": "mm1", "width": 2, "layout": "stereo" },
            { "id": "out-aux", "name": "IFB 1 out", "source": "aux1", "width": 2, "layout": "stereo" },
            { "id": "out-do", "name": "Pres 1 DO", "source": "ch1", "width": 1, "layout": "mono" },
        ],
    })
}

/// The outputs of [`shape`] and their widths, in the order of
/// [`Legs::each`].
pub const OUTPUTS: [(&str, u32); 4] =
    [("out-pgm", 2), ("out-mm", 2), ("out-aux", 2), ("out-do", 1)];

/// The value every sample of each leg of each output holds.
#[derive(Clone, Copy, Debug)]
pub struct Legs {
    pub pgm: [f32; 2],
    pub mm: [f32; 2],
    pub aux: [f32; 2],
    pub direct: f32,
}

impl Legs {
    /// Each output's legs, in the order of [`OUTPUTS`].
    pub fn each(&self) -> [&[f32]; 4] {
        [
            &self.pgm,
            &self.mm,
            &self.aux,
            std::slice::from_ref(&self.direct),
        ]
    }
}

/// A PATCH of the state, or a tally posted for a source.
pub enum Change {
    Patch(Vec<(&'static str, Value)>),
    Tally(&'static str, &'static str),
}

pub struct Step {
    pub what: &'static str,
    pub change: Change,
    /// What the outputs carry once it is applied.
    pub legs: Legs,
}

/// The session: direct out taps, mix-minus exclusions, the bus, aux and
/// mix-minus masters, aux sends, the VCA and the AFV stage, each step
/// changing the legs it is about and no others.
pub fn steps() -> Vec<Step> {
    let g = |db: f32| 10f32.powf(db / 20.0);
    let mut steps = Vec::new();
    let mut step = |what, changes: &[(&'static str, Value)], legs| {
        steps.push(Step {
            what,
            change: Change::Patch(changes.to_vec()),
            legs,
        })
    };
    // Faders closed: a direct out is pre-fader unless set otherwise.
    let mut l = Legs {
        pgm: [0.0; 2],
        mm: [0.0; 2],
        aux: [0.0; 2],
        direct: CH1,
    };
    step(
        "a pre-fader direct out of a closed channel",
        &[
            ("/channels/ch1/pan/lr", json!(-1)),
            ("/channels/ch2/pan/lr", json!(1)),
        ],
        l,
    );
    l.direct = 0.0;
    step(
        "a post-fader direct out of a closed channel",
        &[("/channels/ch1/directOut/tap", json!("postFader"))],
        l,
    );
    l.direct = CH1 * g(-20.0);
    l.pgm[0] = CH1 * g(-20.0);
    l.mm[0] = l.pgm[0];
    step(
        "a post-fader direct out follows its fader",
        &[("/channels/ch1/fader", json!(-20))],
        l,
    );
    l.direct = CH1;
    step(
        "a pre-fader direct out ignores its fader",
        &[("/channels/ch1/directOut/tap", json!("preFader"))],
        l,
    );
    l.pgm = [CH1, CH2 * g(-6.0)];
    l.mm = l.pgm;
    step(
        "both channels open",
        &[
            ("/channels/ch1/fader", json!(0)),
            ("/channels/ch2/fader", json!(-6)),
        ],
        l,
    );

    l.mm[1] = 0.0;
    step(
        "ch2 left out of the mix-minus",
        &[("/mixMinus/mm1/exclude", json!(["ch2"]))],
        l,
    );
    l.mm = l.pgm;
    step(
        "ch2 back in the mix-minus",
        &[("/mixMinus/mm1/exclude", json!([]))],
        l,
    );

    // The mix-minus is the bus's mix before the bus master.
    l.pgm = [CH1 * g(-6.0), CH2 * g(-12.0)];
    step("the bus fader", &[("/buses/pgm/fader", json!(-6))], l);
    l.pgm[0] = -l.pgm[0];
    step(
        "the bus's left leg inverted",
        &[("/buses/pgm/polarity/0", json!(true))],
        l,
    );
    l.pgm = [0.0; 2];
    step("the bus muted", &[("/buses/pgm/mute", json!(true))], l);
    l.pgm = [CH1, CH2 * g(-6.0)];
    step(
        "the bus master back at unity",
        &[
            ("/buses/pgm/mute", json!(false)),
            ("/buses/pgm/fader", json!(0)),
            ("/buses/pgm/polarity/0", json!(false)),
        ],
        l,
    );
    l.mm = [CH1 * g(-6.0), CH2 * g(-12.0)];
    step(
        "the mix-minus fader",
        &[("/mixMinus/mm1/fader", json!(-6))],
        l,
    );
    l.mm = [0.0; 2];
    step(
        "the mix-minus muted",
        &[("/mixMinus/mm1/mute", json!(true))],
        l,
    );
    l.mm = l.pgm;
    step(
        "the mix-minus master back at unity",
        &[
            ("/mixMinus/mm1/mute", json!(false)),
            ("/mixMinus/mm1/fader", json!(0)),
        ],
        l,
    );

    step(
        "a send that is off",
        &[
            ("/channels/ch1/sends/aux1/level", json!(0)),
            ("/channels/ch1/sends/aux1/pan", json!(-1)),
        ],
        l,
    );
    l.aux = [CH1, 0.0];
    step(
        "a pre-fader send",
        &[("/channels/ch1/sends/aux1/on", json!(true))],
        l,
    );
    l.pgm[0] = CH1 * g(-20.0);
    l.mm[0] = l.pgm[0];
    step(
        "a pre-fader send ignores the fader",
        &[("/channels/ch1/fader", json!(-20))],
        l,
    );
    l.aux = [CH1 * g(-20.0), 0.0];
    step(
        "a post-fader send follows the fader",
        &[("/channels/ch1/sends/aux1/pre", json!(false))],
        l,
    );
    l.pgm[0] = CH1;
    l.mm[0] = CH1;
    l.aux = [CH1, 0.0];
    step(
        "a post-fader send with the fader open",
        &[("/channels/ch1/fader", json!(0))],
        l,
    );
    l.aux = [CH1 * g(-6.0), 0.0];
    step(
        "the send level",
        &[("/channels/ch1/sends/aux1/level", json!(-6))],
        l,
    );
    let centre = CH1 * g(-6.0) * std::f32::consts::FRAC_1_SQRT_2;
    l.aux = [centre, centre];
    step(
        "the send panned to the centre at constant power",
        &[("/channels/ch1/sends/aux1/pan", json!(0))],
        l,
    );
    l.aux = [0.0, CH1 * g(-6.0)];
    step(
        "the send panned hard right",
        &[("/channels/ch1/sends/aux1/pan", json!(1))],
        l,
    );
    l.aux[1] = CH1 * g(-12.0);
    step("the aux fader", &[("/auxes/aux1/fader", json!(-6))], l);
    l.aux[1] = -l.aux[1];
    step(
        "the aux's right leg inverted",
        &[("/auxes/aux1/polarity/1", json!(true))],
        l,
    );
    l.aux = [0.0; 2];
    step("the aux muted", &[("/auxes/aux1/mute", json!(true))], l);
    l.aux = [0.0, CH1 * g(-6.0)];
    step(
        "the aux master back at unity",
        &[
            ("/auxes/aux1/mute", json!(false)),
            ("/auxes/aux1/fader", json!(0)),
            ("/auxes/aux1/polarity/1", json!(false)),
        ],
        l,
    );

    step(
        "a VCA offset without members",
        &[("/vcas/vca1/offset", json!(-6))],
        l,
    );
    // The direct out stays pre-fader.
    l.pgm[0] = CH1 * g(-6.0);
    l.mm[0] = l.pgm[0];
    l.aux[1] = CH1 * g(-12.0);
    step(
        "the VCA offset on its member and its post-fader send",
        &[("/vcas/vca1/members", json!(["ch1"]))],
        l,
    );
    l.pgm[0] = 0.0;
    l.mm[0] = 0.0;
    l.aux = [0.0; 2];
    step(
        "the VCA mute silences its member",
        &[("/vcas/vca1/mute", json!(true))],
        l,
    );
    l.pgm = [CH1, CH2 * g(-12.0)];
    l.mm = l.pgm;
    l.aux = [0.0, CH1 * g(-6.0)];
    step(
        "the VCA offset follows its membership",
        &[
            ("/vcas/vca1/mute", json!(false)),
            ("/vcas/vca1/members", json!(["ch2"])),
        ],
        l,
    );

    step(
        "an AFV stage with no tally yet stays open",
        &[
            ("/afv/ch1/enabled", json!(true)),
            ("/afv/ch1/offLevel", json!(-20)),
            ("/afv/ch1/source", json!("cam1")),
        ],
        l,
    );
    // After the fader and before the buses and post-fader sends; the
    // direct out is before it.
    let mut closed = l;
    closed.pgm[0] = CH1 * g(-20.0);
    closed.mm[0] = closed.pgm[0];
    closed.aux[1] = CH1 * g(-26.0);
    steps.push(Step {
        what: "the AFV stage closed to its off level off program",
        change: Change::Tally("cam1", "preview"),
        legs: closed,
    });
    steps.push(Step {
        what: "the AFV stage open again on program",
        change: Change::Tally("cam1", "program"),
        legs: l,
    });
    steps
}
