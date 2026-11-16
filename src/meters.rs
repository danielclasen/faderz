//! Meter frames (api/ws/server-message.schema.json `MeterFrame`, format
//! 3): a 12-byte header, the replica id, then Int16 centi-dB values in
//! shape order. An element's meters are those of the first output of the
//! shape it is the source of; an element no output carries reads null.
//! So a channel is metered at its direct out's tap, not at `meterTap`,
//! and a monitor output after its level, dim, cut, mono and speaker
//! mutes rather than before them as `MonitorMeter` says: the engine
//! meters what an output flow carries.

use samplez::OutputStatus;

use crate::shape::Shape;

const MAGIC: u8 = 0x4d;
const VERSION: u8 = 3;
/// No value: silence for a level, absent for a loudness.
const NULL: i16 = i16::MIN;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Channel,
    Bus,
    Monitor,
}

/// The order and size of a frame's values for one shape.
pub struct Layout {
    /// Kind, legs and the index of the output carrying each element.
    elements: Vec<(Kind, usize, Option<usize>)>,
    /// FNV-1a 32-bit of the layout text.
    pub fingerprint: u32,
}

impl Layout {
    pub fn of(shape: &Shape) -> Layout {
        let output = |id: &str| shape.outputs.iter().position(|o| o.source == id);
        let mut text = String::new();
        let mut elements = Vec::new();
        let mut add = |kind, section: &str, id: &str, legs: usize| {
            text.push_str(&format!("{section}/{id}/{legs};"));
            elements.push((kind, legs, output(id)));
        };
        for c in &shape.channels {
            add(Kind::Channel, "c", &c.id, c.width);
        }
        for b in &shape.buses {
            add(Kind::Bus, "b", &b.id, b.width);
        }
        for a in &shape.auxes {
            add(Kind::Bus, "a", &a.id, a.width);
        }
        for m in &shape.mix_minus {
            add(Kind::Bus, "m", &m.id, m.width);
        }
        for m in &shape.monitor_outputs {
            add(Kind::Monitor, "o", &m.id, m.width);
        }
        Layout {
            elements,
            fingerprint: fnv1a(&text),
        }
    }

    /// The frame numbered `sequence` of `replica` from the engine's
    /// `outputs`.
    pub fn frame(&self, replica: &str, sequence: u32, outputs: &[OutputStatus]) -> Vec<u8> {
        let len = replica.len().min(32);
        let mut b = vec![MAGIC, VERSION, len as u8, 0];
        b.extend_from_slice(&sequence.to_le_bytes());
        b.extend_from_slice(&self.fingerprint.to_le_bytes());
        b.extend_from_slice(&replica.as_bytes()[..len]);
        if len % 2 == 1 {
            b.push(0);
        }
        let mut put = |v: i16| b.extend_from_slice(&v.to_le_bytes());
        for &(kind, legs, output) in &self.elements {
            let m = output.and_then(|o| outputs.get(o)).map(|o| &o.meters);
            let leg = |of: fn(&samplez::OutputMeters) -> &Vec<f32>, i: usize| {
                m.and_then(|m| of(m).get(i))
                    .map_or(NULL, |v| level(f64::from(*v)))
            };
            let loudness =
                |of: fn(&samplez::OutputMeters) -> Option<f64>| m.and_then(of).map_or(NULL, centi);
            for i in 0..legs {
                put(leg(|m| &m.peak, i));
            }
            if kind == Kind::Monitor {
                continue;
            }
            for i in 0..legs {
                put(leg(|m| &m.true_peak, i));
            }
            if kind == Kind::Channel {
                put(loudness(|m| m.momentary));
                continue;
            }
            for i in 0..legs {
                put(leg(|m| &m.rms, i));
            }
            put(loudness(|m| m.momentary));
            put(loudness(|m| m.short_term));
            put(loudness(|m| m.integrated));
            put(loudness(|m| m.range));
            put(loudness(|m| m.max_momentary));
            put(loudness(|m| m.max_short_term));
            // The engine measures no correlation.
            put(NULL);
            put(m.map_or(0, flags));
        }
        b
    }
}

/// Samples of running measurement after which the loudness range counts
/// as stable (model `Loudness.lraStable`).
const LRA_SETTLED: u64 = 60 * 48_000;
/// Samples in one meter window.
const WINDOW: u64 = 4_800;

/// Bit 0 `integratedValid`: nothing dropped since the measurement
/// started. Bit 1 `lraStable`: that, and 60 s measured since.
fn flags(m: &samplez::OutputMeters) -> i16 {
    let measured = match (m.window, m.since) {
        (Some(w), Some(since)) => ((w + 1) * WINDOW).saturating_sub(since),
        _ => 0,
    };
    i16::from(m.valid) | i16::from(m.valid && measured >= LRA_SETTLED) << 1
}

/// A full-scale-1.0 level in centi-dB; silence is null.
fn level(v: f64) -> i16 {
    if v > 0.0 {
        centi(20.0 * v.log10())
    } else {
        NULL
    }
}

fn centi(db: f64) -> i16 {
    if db.is_nan() {
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

    /// The client derives the same fingerprint from the shape
    /// (web/src/lib/contract/meterFrame.ts) and refuses a frame of another.
    #[test]
    fn the_fingerprint_is_the_clients() {
        let layout = Layout::of(&Shape::parse(&small()).unwrap());
        let text = "c/ch1/1;c/ch2/1;c/st1/2;b/pgm/2;b/cf/2;a/aux1/1;m/mm1/2;o/mon/2;";
        assert_eq!(layout.fingerprint, fnv1a(text));
        assert_eq!(fnv1a("a"), 0xe40c_292c);
    }

    /// Header, padding and value count as the client decodes them; the
    /// elements no output carries read null, the one that does its levels.
    #[test]
    fn a_frame_has_the_clients_length_and_values() {
        let shape = Shape::parse(&small()).unwrap();
        let layout = Layout::of(&shape);
        let mut out: OutputStatus = OutputStatus {
            name: "out-pgm".into(),
            realised: true,
            reason: None,
            attempts: 1,
            races: 0,
            create_failures: 0,
            buffer_length: None,
            hints: None,
            meters: samplez::OutputMeters::default(),
        };
        out.meters.peak = vec![0.5, 0.0];
        out.meters.true_peak = vec![0.5, 0.0];
        out.meters.rms = vec![0.5, 0.0];
        out.meters.momentary = Some(-23.0);
        let f = layout.frame("r1", 7, &[out]);
        // Channels 2*legs+1, buses 3*legs+8, monitor legs.
        let values = 3 + 3 + 5 + (6 + 8) * 2 + (3 + 8) + (6 + 8) + 2;
        assert_eq!(f.len(), 12 + 2 + 2 * values);
        assert_eq!(&f[..4], &[MAGIC, VERSION, 2, 0]);
        assert_eq!(u32::from_le_bytes(f[4..8].try_into().unwrap()), 7);
        let v = |i: usize| i16::from_le_bytes([f[14 + 2 * i], f[15 + 2 * i]]);
        assert_eq!(v(0), NULL);
        // pgm, after the 11 channel values: peak L -6.02 dB, R silent.
        assert_eq!(v(11), -602);
        assert_eq!(v(12), NULL);
        assert_eq!(v(17), -2300);
    }

    fn output(meters: samplez::OutputMeters) -> OutputStatus {
        OutputStatus {
            name: "o".into(),
            realised: true,
            reason: None,
            attempts: 1,
            races: 0,
            create_failures: 0,
            buffer_length: None,
            hints: None,
            meters,
        }
    }

    fn lin(db: f64) -> f32 {
        10f64.powf(db / 20.0) as f32
    }

    /// The example frame of server-message.schema.json `MeterFrame`,
    /// encoded from engine meters holding its values. The engine measures
    /// no correlation, so that value alone is null here.
    #[test]
    fn the_contracts_example_frame() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../api/ws/server-message.schema.json")).unwrap();
        let hex = schema["$defs"]["MeterFrame"]["examples"][0]
            .as_str()
            .unwrap();
        let mut want: Vec<u8> = hex
            .split(' ')
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect();
        // Correlation, the 16th value after the 16-byte header.
        let corr = 16 + 2 * 15;
        want[corr..corr + 2].copy_from_slice(&NULL.to_le_bytes());

        let shape: Shape = serde_json::from_value(serde_json::json!({
            "sampleRate": 48000,
            "frameRate": { "numerator": 25, "denominator": 1 },
            "channels": [{ "id": "ch1", "name": "c", "width": 1, "layout": "mono", "altInput": false }],
            "buses": [{ "id": "pgm", "name": "p", "kind": "program", "width": 2, "layout": "stereo" }],
            "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
            "outputs": [
                { "id": "do", "name": "d", "source": "ch1", "width": 1, "layout": "mono" },
                { "id": "out", "name": "o", "source": "pgm", "width": 2, "layout": "stereo" },
            ],
        }))
        .unwrap();
        let ch1 = samplez::OutputMeters {
            peak: vec![lin(-6.5)],
            true_peak: vec![lin(-5.9)],
            momentary: Some(-20.0),
            ..Default::default()
        };
        let pgm = samplez::OutputMeters {
            window: Some(10),
            since: Some(0),
            valid: true,
            peak: vec![lin(0.0), lin(3.21)],
            true_peak: vec![lin(0.5), 0.0],
            rms: vec![lin(-18.0), lin(-18.5)],
            momentary: Some(-23.0),
            integrated: Some(-23.46),
            range: Some(7.25),
            max_momentary: Some(-21.5),
            ..Default::default()
        };
        let got = Layout::of(&shape).frame("rep", 258, &[output(ch1), output(pgm)]);
        assert_eq!(got, want);
    }

    /// Loudness: integratedValid while nothing was dropped, lraStable from
    /// 60 s of measurement on.
    #[test]
    fn the_loudness_flags_follow_validity_and_measured_time() {
        let m = |valid, window| samplez::OutputMeters {
            valid,
            window: Some(window),
            since: Some(4_800),
            integrated: Some(-23.0),
            range: Some(5.0),
            ..Default::default()
        };
        assert_eq!(flags(&m(true, 10)), 1);
        assert_eq!(flags(&m(true, 599)), 1);
        assert_eq!(flags(&m(true, 600)), 3);
        assert_eq!(flags(&m(false, 600)), 0);
        assert_eq!(flags(&samplez::OutputMeters::default()), 0);
    }
}
