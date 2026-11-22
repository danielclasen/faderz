//! Meter frames (api/ws/server-message.schema.json `MeterFrame`, format
//! 4): a 12-byte header, the replica id, then Int16 values in shape
//! order, the duck groups' readings last.
//!
//! A channel's peak, true peak and momentary loudness are an engine meter
//! of its input at its `meterTap`: input reads the meter after the
//! channel delay and before the strip (samplez `preStripMeter`), so trim,
//! filters and dynamics do not reach it, and null while there is none.
//! That meter runs only while the tap is input
//! ([`crate::mix::pre_strip_meters`]), and measures afresh from the switch
//! to input. preFader reads the meter after
//! the strip; postFader that one times the channel's fader, mute and VCA
//! gain. The tap gains are those of the state last applied, while the
//! engine ramps a change from its index and meters in 100 ms windows, so
//! for a window or so after a fader move the levels lead the audio. A
//! bus, aux, mix-minus or monitor output reads the meters of the first
//! full-width output it is the source of, or the engine meter on matrix
//! columns of its own when no full-width output carries it. So a monitor
//! output is metered on what it plays, the listen sum while anything
//! solos ([`crate::mix`]), after its level, dim, cut, mono and speaker
//! mutes rather than before them as `MonitorMeter` says. A bus, aux or
//! mix-minus output of the stereo layout reads the engine's correlation
//! of its L and R legs, any other null. A level of digital silence reads
//! the floor, -120 dB; null is a value not measured.
//!
//! A duck group reads the engine's `DuckMeter` of its group over the
//! last meter slice of 10 ms it was computed in: the largest key level,
//! the largest envelope and whether its detector was engaged.

use samplez::{DuckMeter, InputMeters, OutputMeters, Status};

use crate::mix::MeterTap;
use crate::shape::Shape;

const MAGIC: u8 = 0x4d;
const VERSION: u8 = 4;
/// Full envelope, full depth: the envelope 0 to 1 in ten-thousandths.
const ENVELOPE_FULL: f64 = 10_000.0;
/// No value: not measured.
const NULL: i16 = i16::MIN;
/// Digital silence: -120 dB, the floor of a level.
const FLOOR: i16 = -12_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Channel,
    Bus,
    Monitor,
}

/// Where an element's meters come from.
#[derive(Clone, Copy)]
enum Source {
    /// Input slot `slot`.
    Channel(usize),
    Output(usize),
    /// An engine meter, numbered as the engine's `meters`.
    Meter(usize),
}

/// The meters a frame is made of.
pub struct Readings<'a> {
    pub outputs: Vec<&'a OutputMeters>,
    pub meters: Vec<&'a OutputMeters>,
    pub inputs: Vec<&'a InputMeters>,
    /// Each input's meter before its strip, as `inputs`.
    pub pre_strip: Vec<Option<&'a InputMeters>>,
    /// Each duck group's readings, numbered as the shape's ducks.
    pub ducks: &'a [DuckMeter],
    /// Each channel's tap gains ([`crate::mix::meter_taps`]).
    pub taps: &'a [MeterTap],
}

impl<'a> Readings<'a> {
    pub fn of(status: &'a Status, taps: &'a [MeterTap]) -> Readings<'a> {
        Readings {
            outputs: status.outputs.iter().map(|o| &o.meters).collect(),
            meters: status.meters.iter().map(|m| &m.meters).collect(),
            inputs: status.inputs.iter().map(|i| &i.meters).collect(),
            pre_strip: (status.inputs.iter())
                .map(|i| i.pre_strip_meters.as_ref())
                .collect(),
            ducks: &status.ducks,
            taps,
        }
    }
}

/// The order and size of a frame's values for one shape.
pub struct Layout {
    /// Kind, legs and source of each element.
    elements: Vec<(Kind, usize, Source)>,
    /// The id of each element, as `elements`.
    ids: Vec<String>,
    /// The shape's outputs: the engine's meters are numbered after them.
    outputs: usize,
    /// Duck groups, one engine group each in shape order.
    ducks: usize,
    /// FNV-1a 32-bit of the layout text.
    pub fingerprint: u32,
}

impl Layout {
    pub fn of(shape: &Shape) -> Layout {
        let output = |id: &str, width: usize| shape.full_output(id, width);
        let meters = shape.meter_columns();
        let source = |id: &str, width: usize| match output(id, width) {
            Some(o) => Source::Output(o),
            None => Source::Meter(
                meters
                    .iter()
                    .position(|m| m.0 == id)
                    .expect("an element no full-width output carries has a meter"),
            ),
        };
        let mut text = String::new();
        let mut elements = Vec::new();
        let mut ids = Vec::new();
        let mut add = |kind, section: &str, id: &str, legs: usize, source| {
            text.push_str(&format!("{section}/{id}/{legs};"));
            elements.push((kind, legs, source));
            ids.push(id.to_owned());
        };
        for (slot, c) in shape.channels.iter().enumerate() {
            add(Kind::Channel, "c", &c.id, c.width, Source::Channel(slot));
        }
        for b in &shape.buses {
            add(Kind::Bus, "b", &b.id, b.width, source(&b.id, b.width));
        }
        for a in &shape.auxes {
            add(Kind::Bus, "a", &a.id, a.width, source(&a.id, a.width));
        }
        for m in &shape.mix_minus {
            add(Kind::Bus, "m", &m.id, m.width, source(&m.id, m.width));
        }
        for m in &shape.monitor_outputs {
            add(Kind::Monitor, "o", &m.id, m.width, source(&m.id, m.width));
        }
        for d in &shape.ducks {
            text.push_str(&format!("d/{}/1;", d.id));
        }
        Layout {
            elements,
            ids,
            outputs: shape.outputs.len(),
            ducks: shape.ducks.len(),
            fingerprint: fnv1a(&text),
        }
    }

    /// The number the engine gives the measurement of bus, aux or
    /// mix-minus `id`: its first full-width output's, or after the
    /// outputs its engine meter's ([`samplez::Entry::ResetLoudness`]).
    pub fn loudness_meter(&self, id: &str) -> Option<usize> {
        let at = self.ids.iter().position(|i| i == id)?;
        match self.elements[at] {
            (Kind::Bus, _, Source::Output(o)) => Some(o),
            (Kind::Bus, _, Source::Meter(m)) => Some(self.outputs + m),
            _ => None,
        }
    }

    /// The index in a frame's values of the first value of element `id`
    /// (a channel, bus, aux, mix-minus or monitor output), its peak of
    /// the first leg: the elements come in shape order, a channel with
    /// 2n + 1 values for n legs, a monitor output n and any other 3n + 8.
    pub fn offset(&self, id: &str) -> Option<usize> {
        let at = self.ids.iter().position(|i| i == id)?;
        Some(
            self.elements[..at]
                .iter()
                .map(|&(kind, n, _)| match kind {
                    Kind::Channel => 2 * n + 1,
                    Kind::Monitor => n,
                    Kind::Bus => 3 * n + 8,
                })
                .sum(),
        )
    }

    /// The frame numbered `sequence` of `replica` from the engine's
    /// readings.
    pub fn frame(&self, replica: &str, sequence: u32, r: &Readings<'_>) -> Vec<u8> {
        let len = replica.len().min(32);
        let mut b = vec![MAGIC, VERSION, len as u8, 0];
        b.extend_from_slice(&sequence.to_le_bytes());
        b.extend_from_slice(&self.fingerprint.to_le_bytes());
        b.extend_from_slice(&replica.as_bytes()[..len]);
        if len % 2 == 1 {
            b.push(0);
        }
        let mut put = |v: i16| b.extend_from_slice(&v.to_le_bytes());
        let legs = |v: Option<&Vec<f32>>, i: usize, g: f64| {
            v.and_then(|v| v.get(i))
                .map_or(NULL, |v| level(f64::from(*v) * g))
        };
        for &(kind, n, source) in &self.elements {
            if let Source::Channel(slot) = source {
                let tap = r.taps.get(slot);
                let g = tap.map_or(1.0, |t| t.meter);
                let input = if tap.is_some_and(|t| t.pre_strip) {
                    r.pre_strip.get(slot).copied().flatten()
                } else {
                    r.inputs.get(slot).copied()
                };
                for i in 0..n {
                    put(legs(input.map(|m| &m.peak), i, g));
                }
                for i in 0..n {
                    put(legs(input.map(|m| &m.true_peak), i, g));
                }
                // Loudness scales with the power, g squared; a tap that
                // passes nothing reads silence, as its peak does, once the
                // input is measured.
                let gain_db = 20.0 * g.log10();
                put(match input.and_then(|m| m.momentary) {
                    Some(_) if g <= 0.0 => FLOOR,
                    Some(l) => centi(l + gain_db),
                    None => NULL,
                });
                continue;
            }
            let m = match source {
                Source::Output(o) => r.outputs.get(o),
                Source::Meter(m) => r.meters.get(m),
                Source::Channel(_) => unreachable!("handled above"),
            }
            .copied();
            for i in 0..n {
                put(legs(m.map(|m| &m.peak), i, 1.0));
            }
            if kind == Kind::Monitor {
                continue;
            }
            for i in 0..n {
                put(legs(m.map(|m| &m.true_peak), i, 1.0));
            }
            for i in 0..n {
                put(legs(m.map(|m| &m.rms), i, 1.0));
            }
            let l = |of: fn(&OutputMeters) -> Option<f64>| m.and_then(of).map_or(NULL, centi);
            put(l(|m| m.momentary));
            put(l(|m| m.short_term));
            put(l(|m| m.integrated));
            put(l(|m| m.range));
            put(l(|m| m.max_momentary));
            put(l(|m| m.max_short_term));
            put(m.and_then(|m| m.correlation).map_or(NULL, centi));
            put(m.map_or(0, flags));
        }
        for g in 0..self.ducks {
            let [key, envelope, flags] = duck(r.ducks.get(g));
            put(key);
            put(envelope);
            put(flags);
        }
        b
    }
}

/// A duck group's values: its key level in centi-dBFS, null while the
/// group is not in use; its envelope in ten-thousandths, 0 while not in
/// use; flags, bit 0 engaged (triggered or holding).
fn duck(m: Option<&DuckMeter>) -> [i16; 3] {
    match m {
        Some(m) if m.slice.is_some() => {
            let envelope = (f64::from(m.envelope) * ENVELOPE_FULL + 0.5).floor();
            [
                level(f64::from(m.key)),
                envelope.clamp(0.0, ENVELOPE_FULL) as i16,
                i16::from(m.engaged),
            ]
        }
        _ => [NULL, 0, 0],
    }
}

/// Samples of running measurement after which the loudness range counts
/// as stable (model `Loudness.lraStable`).
const LRA_SETTLED: u64 = 60 * 48_000;
/// Samples in one meter window.
const WINDOW: u64 = 4_800;

/// Bit 0 `integratedValid`: an integrated loudness is measured; a gap
/// or a lost slice does not clear it, as samplez measures across them.
/// Bit 1 `lraStable`: that, and 60 s measured since.
fn flags(m: &OutputMeters) -> i16 {
    let measured = match (m.window, m.since) {
        (Some(w), Some(since)) => ((w + 1) * WINDOW).saturating_sub(since),
        _ => 0,
    };
    let valid = m.integrated.is_some();
    i16::from(valid) | i16::from(valid && measured >= LRA_SETTLED) << 1
}

/// A full-scale-1.0 level in centi-dB; silence is the floor, NaN null.
fn level(v: f64) -> i16 {
    if v > 0.0 {
        centi(20.0 * v.log10())
    } else if v == 0.0 {
        FLOOR
    } else {
        NULL
    }
}

/// Centi-units of a level, loudness or loudness range; NaN and
/// -Infinity, a loudness of silence, are null.
fn centi(db: f64) -> i16 {
    if db.is_nan() || db == f64::NEG_INFINITY {
        return NULL;
    }
    (db * 100.0 + 0.5).floor().clamp(-32767.0, 32767.0) as i16
}

fn fnv1a(text: &str) -> u32 {
    text.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::tests::small;
    use serde_json::Value;

    /// The client derives the same fingerprint from the shape
    /// (web/src/lib/contract/meterFrame.ts) and refuses a frame of another.
    #[test]
    fn the_fingerprint_is_the_clients() {
        let layout = Layout::of(&Shape::parse(&small()).unwrap());
        let text = "c/ch1/1;c/ch2/1;c/st1/2;b/pgm/2;b/cf/2;a/aux1/1;m/mm1/2;o/mon/2;d/comm/1;";
        assert_eq!(layout.fingerprint, fnv1a(text));
        assert_eq!(fnv1a("a"), 0xe40c_292c);
    }

    #[test]
    fn a_loudness_meter_is_its_output_or_its_engine_meter_after_them() {
        let layout = Layout::of(&Shape::parse(&small()).unwrap());
        // Outputs: out-pgm. Meters: cf, aux1, mm1, mon.
        assert_eq!(layout.loudness_meter("pgm"), Some(0));
        assert_eq!(layout.loudness_meter("cf"), Some(1));
        assert_eq!(layout.loudness_meter("aux1"), Some(2));
        assert_eq!(layout.loudness_meter("mm1"), Some(3));
        assert_eq!(layout.loudness_meter("ch1"), None);
        assert_eq!(layout.loudness_meter("nope"), None);
    }

    /// An element's offset is the values of every element before it, so
    /// it moves when an element is added ahead of it.
    #[test]
    fn an_offset_counts_the_values_of_the_elements_before() {
        let layout = Layout::of(&Shape::parse(&small()).unwrap());
        // ch1 3, ch2 3, st1 5, pgm 14, cf 14, aux1 11, mm1 14.
        assert_eq!(layout.offset("ch1"), Some(0));
        assert_eq!(layout.offset("ch2"), Some(3));
        assert_eq!(layout.offset("pgm"), Some(11));
        assert_eq!(layout.offset("mon"), Some(11 + 14 + 14 + 11 + 14));
        assert_eq!(layout.offset("nope"), None);
        let mut with_a_bus = small();
        with_a_bus["buses"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "id": "b2", "name": "B2", "kind": "cleanFeed", "width": 2, "layout": "stereo" }));
        let more = Layout::of(&Shape::parse(&with_a_bus).unwrap());
        assert_eq!(more.offset("mon"), Some(64 + 14));
    }

    /// An element is metered by the output that carries it at its full
    /// width; one a narrower output alone carries has an engine meter.
    #[test]
    fn a_fold_down_output_is_not_its_sources_meter() {
        let mut v = crate::shape::tests::surround();
        let source = |v: &Value| {
            let layout = Layout::of(&Shape::parse(v).unwrap());
            layout
                .elements
                .iter()
                .find(|e| e.1 == 6 && e.0 == Kind::Bus)
                .unwrap()
                .2
        };
        assert!(matches!(source(&v), Source::Output(0)));
        v["outputs"].as_array_mut().unwrap().remove(0);
        assert!(matches!(source(&v), Source::Meter(0)));
    }

    fn input(peak: Vec<f32>, momentary: Option<f64>) -> InputMeters {
        InputMeters {
            window: Some(10),
            peak,
            rms: Vec::new(),
            momentary,
            true_peak: Vec::new(),
            max_true_peak: Vec::new(),
        }
    }

    const TAP: MeterTap = MeterTap {
        meter: 1.0,
        pre_strip: false,
    };

    fn tap(meter: f64) -> MeterTap {
        MeterTap { meter, ..TAP }
    }

    /// [`input`] with a true peak.
    fn input_tp(peak: Vec<f32>, true_peak: Vec<f32>, momentary: Option<f64>) -> InputMeters {
        InputMeters {
            true_peak,
            ..input(peak, momentary)
        }
    }

    fn lin(db: f64) -> f32 {
        10f64.powf(db / 20.0) as f32
    }

    /// Header, padding and value count as the client decodes them. Every
    /// element is metered whether an output carries it or not: channels
    /// by their input meters, pgm by its output, the other buses, the
    /// aux, the mix-minus and the monitor by engine meters of their own.
    /// Silence reads the floor, a value not measured null.
    #[test]
    fn a_frame_has_the_clients_length_and_values() {
        let shape = Shape::parse(&small()).unwrap();
        let layout = Layout::of(&shape);
        let pgm = OutputMeters {
            peak: vec![0.5, 0.0],
            true_peak: vec![0.5, 0.0],
            rms: vec![0.5, 0.0],
            momentary: Some(-23.0),
            correlation: Some(-0.85),
            ..Default::default()
        };
        let cf = OutputMeters {
            peak: vec![0.25, 0.25],
            ..Default::default()
        };
        let mon = OutputMeters {
            peak: vec![0.5, 0.5],
            ..Default::default()
        };
        let none = OutputMeters::default();
        let ch1 = input_tp(vec![0.25], vec![0.5], Some(-20.0));
        let ch2 = InputMeters::default();
        let st1 = input(vec![0.0, 0.0], None);
        let r = Readings {
            outputs: vec![&pgm],
            meters: vec![&cf, &none, &none, &mon],
            inputs: vec![&ch1, &ch2, &st1],
            pre_strip: vec![None; 3],
            ducks: &[DuckMeter {
                slice: Some(3),
                key: 0.5,
                envelope: 0.25,
                engaged: true,
            }],
            taps: &[TAP; 3],
        };
        let f = layout.frame("r1", 7, &r);
        // Channels 2*legs+1, buses 3*legs+8, monitor legs, ducks 3.
        let values = 3 + 3 + 5 + (6 + 8) * 2 + (3 + 8) + (6 + 8) + 2 + 3;
        assert_eq!(f.len(), 12 + 2 + 2 * values);
        assert_eq!(&f[..4], &[MAGIC, VERSION, 2, 0]);
        assert_eq!(u32::from_le_bytes(f[4..8].try_into().unwrap()), 7);
        let v = |i: usize| i16::from_le_bytes([f[14 + 2 * i], f[15 + 2 * i]]);
        // ch1 at its input meter, its true peak too: it has no direct out.
        assert_eq!([v(0), v(1), v(2)], [-1204, -602, -2000]);
        // ch2 not measured yet.
        assert_eq!([v(3), v(4), v(5)], [NULL, NULL, NULL]);
        // st1 silent.
        assert_eq!(
            [v(6), v(7), v(8), v(9), v(10)],
            [FLOOR, FLOOR, NULL, NULL, NULL]
        );
        // pgm, from its output: peak L -6.02 dB, R silent; its
        // correlation, the 13th value after its legs.
        assert_eq!([v(11), v(12), v(17), v(23)], [-602, FLOOR, -2300, -85]);
        // cf, from its engine meter, which measured no correlation.
        assert_eq!([v(25), v(26), v(37)], [-1204, -1204, NULL]);
        // The monitor, then the duck group: key -6.02 dBFS, envelope a
        // quarter, engaged.
        assert_eq!([v(values - 5), v(values - 4)], [-602, -602]);
        assert_eq!(
            [v(values - 3), v(values - 2), v(values - 1)],
            [-602, 2500, 1]
        );
    }

    /// A duck group not in use reads a null key, no envelope and no flag;
    /// a silent key the floor; the envelope rounds to ten-thousandths and
    /// stays within 0 to 10000.
    #[test]
    fn a_duck_group_reads_its_engine_meter() {
        let m = |slice, key, envelope, engaged| DuckMeter {
            slice,
            key,
            envelope,
            engaged,
        };
        assert_eq!(duck(None), [NULL, 0, 0]);
        assert_eq!(duck(Some(&m(None, 0.5, 0.5, true))), [NULL, 0, 0]);
        assert_eq!(duck(Some(&m(Some(0), 0.0, 0.0, false))), [FLOOR, 0, 0]);
        assert_eq!(duck(Some(&m(Some(9), 1.0, 1.0, true))), [0, 10_000, 1]);
        assert_eq!(duck(Some(&m(Some(9), 1.0, 0.123_45, false))), [0, 1235, 0]);
        assert_eq!(duck(Some(&m(Some(9), 1.0, 1.5, false)))[1], 10_000);
    }

    /// A channel's meter follows its meterTap: the gain of a post-fader
    /// tap scales the levels and the momentary loudness by the power; a
    /// closed fader reads silence in both, and an input not measured yet
    /// reads no loudness whatever its tap.
    #[test]
    fn a_channel_meter_takes_its_tap_gain() {
        let shape = Shape::parse(&small()).unwrap();
        let layout = Layout::of(&shape);
        let none = OutputMeters::default();
        let ch1 = input(vec![0.5], Some(-20.0));
        let st1 = input(vec![0.5, 0.25], Some(-20.0));
        let r = Readings {
            outputs: vec![&none],
            meters: vec![&none; 4],
            inputs: vec![&ch1, &ch1, &st1],
            pre_strip: vec![None; 3],
            ducks: &[],
            taps: &[tap(0.5), tap(0.0), TAP],
        };
        let f = layout.frame("r1", 0, &r);
        let v = |i: usize| i16::from_le_bytes([f[14 + 2 * i], f[15 + 2 * i]]);
        assert_eq!([v(0), v(2)], [-1204, -2602]);
        assert_eq!([v(3), v(5)], [FLOOR, FLOOR]);
        // An input not measured yet has no loudness, closed or not.
        let unmeasured = input(vec![0.5], None);
        let r = Readings {
            inputs: vec![&unmeasured, &unmeasured, &st1],
            ..r
        };
        let f = layout.frame("r1", 0, &r);
        let v = |i: usize| i16::from_le_bytes([f[14 + 2 * i], f[15 + 2 * i]]);
        assert_eq!([v(2), v(5)], [NULL, NULL]);
        assert_eq!([v(6), v(7), v(10)], [-602, -1204, -2000]);
    }

    /// A channel's true peak is its input meter's, taken to the meter tap
    /// as its peak is: scaled by a post-fader tap's gain, the floor on a
    /// closed one. A direct out's meter does not enter it, and a channel
    /// without one has it too; an input with no true peak reads null.
    #[test]
    fn a_channel_true_peak_is_its_inputs_at_its_meter_tap() {
        let mut v = crate::shape::tests::small();
        v["outputs"] = serde_json::json!([
            { "id": "do", "name": "d", "source": "ch1", "width": 1, "layout": "mono" },
            { "id": "out", "name": "o", "source": "pgm", "width": 2, "layout": "stereo" },
        ]);
        let layout = Layout::of(&Shape::parse(&v).unwrap());
        let ch = input_tp(vec![0.25], vec![0.5], Some(-20.0));
        let st1 = input_tp(vec![0.25, 0.25], vec![0.5, 0.25], Some(-20.0));
        let direct = OutputMeters {
            true_peak: vec![1.0],
            ..Default::default()
        };
        let none = OutputMeters::default();
        let frame = |t: MeterTap, ch2: &InputMeters| {
            let r = Readings {
                outputs: vec![&direct, &none],
                meters: vec![&none; 4],
                inputs: vec![&ch, ch2, &st1],
                pre_strip: vec![None; 3],
                ducks: &[],
                taps: &[t, t, t],
            };
            let f = layout.frame("r1", 0, &r);
            let v = |i: usize| i16::from_le_bytes([f[14 + 2 * i], f[15 + 2 * i]]);
            // ch1 with its direct out, ch2 and st1 without one.
            [v(1), v(4), v(8), v(9)]
        };
        assert_eq!(frame(TAP, &ch), [-602, -602, -602, -1204]);
        // A post-fader meter at -6 dB.
        assert_eq!(frame(tap(0.5), &ch), [-1204, -1204, -1204, -1806]);
        // A closed fader.
        assert_eq!(frame(tap(0.0), &ch), [FLOOR, FLOOR, FLOOR, FLOOR]);
        // An input whose engine meters no true peak.
        let without = input(vec![0.25], Some(-20.0));
        assert_eq!(frame(TAP, &without)[1], NULL);
    }

    /// A channel at its input tap reads its input's meter before the
    /// strip, peak, true peak and momentary loudness, at unity whatever
    /// its fader; at its pre-fader tap the meter after the strip. An
    /// input tap on an input with no meter before its strip reads null,
    /// not the meter after it.
    #[test]
    fn an_input_meter_tap_reads_the_input_before_its_strip() {
        let layout = Layout::of(&Shape::parse(&small()).unwrap());
        let none = OutputMeters::default();
        let after = input_tp(vec![lin(-12.0)], vec![lin(-11.0)], Some(-30.0));
        let before = input_tp(vec![lin(-6.0)], vec![lin(-5.0)], Some(-24.0));
        let st1 = input(vec![0.5, 0.5], Some(-20.0));
        let input_tap = MeterTap {
            meter: 1.0,
            pre_strip: true,
        };
        let frame = |taps: &[MeterTap], pre: Option<&InputMeters>| {
            let r = Readings {
                outputs: vec![&none],
                meters: vec![&none; 4],
                inputs: vec![&after, &after, &st1],
                pre_strip: vec![pre, pre, None],
                ducks: &[],
                taps,
            };
            let f = layout.frame("r1", 0, &r);
            let v = |i: usize| i16::from_le_bytes([f[14 + 2 * i], f[15 + 2 * i]]);
            [v(0), v(1), v(2), v(3), v(4), v(5)]
        };
        assert_eq!(
            frame(&[input_tap, TAP, TAP], Some(&before)),
            [-600, -500, -2400, -1200, -1100, -3000]
        );
        assert_eq!(
            frame(&[input_tap, TAP, TAP], None),
            [NULL, NULL, NULL, -1200, -1100, -3000]
        );
    }

    /// A bus's correlation in hundredths, rounded half up and within -1
    /// to 1 (-100..100); null where the engine measured none.
    #[test]
    fn a_bus_correlation_is_its_meters_in_hundredths() {
        let layout = Layout::of(&Shape::parse(&small()).unwrap());
        let none = OutputMeters::default();
        let ins = InputMeters::default();
        let corr = |c: Option<f64>| {
            let pgm = OutputMeters {
                correlation: c,
                ..Default::default()
            };
            let r = Readings {
                outputs: vec![&pgm],
                meters: vec![&none; 4],
                inputs: vec![&ins; 3],
                pre_strip: vec![None; 3],
                ducks: &[],
                taps: &[TAP; 3],
            };
            let f = layout.frame("r1", 0, &r);
            let at = layout.offset("pgm").unwrap() + 3 * 2 + 6;
            i16::from_le_bytes([f[14 + 2 * at], f[15 + 2 * at]])
        };
        assert_eq!(corr(Some(0.85)), 85);
        assert_eq!(corr(Some(1.0)), 100);
        assert_eq!(corr(Some(-1.0)), -100);
        assert_eq!(corr(Some(0.0)), 0);
        assert_eq!(corr(Some(0.004_9)), 0);
        assert_eq!(corr(Some(-0.004)), 0);
        assert_eq!(corr(Some(-0.015_1)), -2);
        assert_eq!(corr(None), NULL);
    }

    /// The example frame of server-message.schema.json `MeterFrame`,
    /// encoded from engine meters holding its values. ch1's true peak is
    /// its input's, whatever its direct out reads.
    #[test]
    fn the_contracts_example_frame() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../api/ws/server-message.schema.json")).unwrap();
        let hex = schema["$defs"]["MeterFrame"]["examples"][0]
            .as_str()
            .unwrap();
        let want: Vec<u8> = hex
            .split(' ')
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect();

        let shape: Shape = serde_json::from_value(serde_json::json!({
            "sampleRate": 48000,
            "frameRate": { "numerator": 25, "denominator": 1 },
            "channels": [{ "id": "ch1", "name": "c", "width": 1, "layout": "mono", "altInput": false }],
            "buses": [{ "id": "pgm", "name": "p", "kind": "program", "width": 2, "layout": "stereo" }],
            "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
            "ducks": [{ "id": "comm", "name": "c" }],
            "outputs": [
                { "id": "do", "name": "d", "source": "ch1", "width": 1, "layout": "mono" },
                { "id": "out", "name": "o", "source": "pgm", "width": 2, "layout": "stereo" },
            ],
        }))
        .unwrap();
        let ch1 = input_tp(vec![lin(-6.5)], vec![lin(-5.9)], Some(-20.0));
        let direct = OutputMeters {
            true_peak: vec![lin(-1.0)],
            ..Default::default()
        };
        let pgm = OutputMeters {
            window: Some(10),
            since: Some(0),
            peak: vec![lin(0.0), lin(3.21)],
            true_peak: vec![lin(0.5), 0.0],
            rms: vec![lin(-18.0), lin(-18.5)],
            momentary: Some(-23.0),
            integrated: Some(-23.46),
            range: Some(7.25),
            max_momentary: Some(-21.5),
            correlation: Some(0.85),
            ..Default::default()
        };
        let r = Readings {
            outputs: vec![&direct, &pgm],
            meters: Vec::new(),
            inputs: vec![&ch1],
            pre_strip: vec![None; 1],
            ducks: &[DuckMeter {
                slice: Some(0),
                key: lin(-20.0),
                envelope: 0.5,
                engaged: true,
            }],
            taps: &[TAP],
        };
        let got = Layout::of(&shape).frame("rep", 258, &r);
        assert_eq!(got, want);
        // The same programme with gaps and a slice lost: samplez measures
        // across them, and the frame carries the integrated loudness and
        // its valid flag as before.
        let gapped = OutputMeters {
            gaps: 12,
            lost: 3,
            ..pgm.clone()
        };
        let r = Readings {
            outputs: vec![&direct, &gapped],
            ..r
        };
        assert_eq!(Layout::of(&shape).frame("rep", 258, &r), want);
    }

    /// Loudness: integratedValid while an integrated loudness is measured,
    /// across gaps and lost slices, which never blank it; lraStable from
    /// 60 s of measurement on.
    #[test]
    fn the_loudness_flags_follow_the_measurement_not_its_gaps() {
        let m = |gaps, lost, window| OutputMeters {
            gaps,
            lost,
            window: Some(window),
            since: Some(4_800),
            integrated: Some(-23.0),
            range: Some(5.0),
            ..Default::default()
        };
        assert_eq!(flags(&m(0, 0, 10)), 1);
        assert_eq!(flags(&m(0, 0, 599)), 1);
        assert_eq!(flags(&m(0, 0, 600)), 3);
        assert_eq!(flags(&m(1, 0, 600)), 3, "a gap");
        assert_eq!(flags(&m(0, 1, 10)), 1, "a slice lost");
        assert_eq!(flags(&OutputMeters::default()), 0, "nothing measured");
    }
}
