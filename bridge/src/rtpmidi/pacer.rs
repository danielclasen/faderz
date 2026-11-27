//! Paces what goes to a network surface: a byte rate with bursts of one
//! packet, and a queue in which a newer motor position, LED, V-Pot
//! ring, timecode digit or a unit's strip colours replace the queued
//! ones they supersede, a newer meter message goes last in place of
//! it, and a scribble strip
//! write drops the queued ones it overwrites whole.

use std::collections::VecDeque;

use super::payload;

/// The most MIDI list octets in one packet, and the burst the rate
/// allows: an X-Touch drops some of the messages of larger packets. A
/// scribble strip line, 64 octets as SysEx, still fits in one.
pub const MAX_PAYLOAD: usize = 64;

/// What makes a queued message stale once a newer one comes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    /// A motor fader: pitch bend per channel.
    Fader(u8),
    /// A key LED: note per channel.
    Led(u8, u8),
    /// A V-Pot ring: control change 48 to 55 per channel.
    Ring(u8, u8),
    /// A timecode display digit: control change 0x40 to 0x49.
    Timecode(u8),
    /// A meter level: channel pressure carries the strip in its high
    /// nibble. The overload (14 and 15 in the low nibble) is a key of its
    /// own, so a level sent after it never takes its place.
    Meter(u8, u8, bool),
    /// A scribble strip write: device, first and last position.
    Lcd(u8, usize, usize),
    /// An X-Touch's scribble strip colours: device.
    Colours(u8),
}

fn key(m: &[u8]) -> Option<Key> {
    match m {
        [s, _, _] if s & 0xf0 == 0xe0 => Some(Key::Fader(*s)),
        [s, note, _] if s & 0xf0 == 0x90 => Some(Key::Led(*s, *note)),
        [s, cc @ 48..=55, _] if s & 0xf0 == 0xb0 => Some(Key::Ring(*s, *cc)),
        [0xb0, cc @ 0x40..=0x49, _] => Some(Key::Timecode(*cc)),
        [s, level] if s & 0xf0 == 0xd0 => Some(Key::Meter(*s, level >> 4, level & 0x0f >= 14)),
        [0xf0, 0, 0, 0x66, device, 0x12, offset, text @ .., 0xf7] if !text.is_empty() => {
            let first = usize::from(*offset);
            Some(Key::Lcd(*device, first, first + text.len() - 1))
        }
        [0xf0, 0, 0, 0x66, device, 0x72, .., 0xf7] => Some(Key::Colours(*device)),
        _ => None,
    }
}

pub struct Pacer {
    queue: VecDeque<(Option<Key>, Vec<u8>)>,
    /// Octets per microsecond.
    rate: f64,
    tokens: f64,
    at: u64,
}

impl Pacer {
    pub fn new(bytes_per_second: u32) -> Pacer {
        Pacer {
            queue: VecDeque::new(),
            rate: f64::from(bytes_per_second) / 1e6,
            tokens: MAX_PAYLOAD as f64,
            at: 0,
        }
    }

    /// Queues one MIDI message; a SysEx longer than a packet goes in
    /// segments.
    pub fn push(&mut self, message: Vec<u8>) {
        let k = key(&message);
        // A scribble strip write goes last, after the writes it overlaps
        // in part, and those it covers whole go.
        if let Some(Key::Lcd(device, first, last)) = k {
            self.queue.retain(|(q, _)| {
                !matches!(q, Some(Key::Lcd(d, f, l)) if *d == device && *f >= first && *l <= last)
            });
            if message.len() <= MAX_PAYLOAD {
                self.queue.push_back((k, message));
            } else {
                self.push_segments(&message);
            }
            return;
        }
        // A meter message goes last, so a level never ends up before the
        // overload it follows: an X-Touch reads 15, the overload off, as a
        // level until the next one.
        if let Some(Key::Meter(..)) = k {
            self.queue.retain(|(q, _)| q != &k);
            self.queue.push_back((k, message));
            return;
        }
        if let Some(k) = &k
            && let Some(queued) = self.queue.iter_mut().find(|(q, _)| q.as_ref() == Some(k))
        {
            queued.1 = message;
            return;
        }
        if message.first() == Some(&0xf0) && message.len() > MAX_PAYLOAD {
            self.push_segments(&message);
        } else {
            self.queue.push_back((k, message));
        }
    }

    /// Queues a SysEx longer than a packet in segments, none of which a
    /// newer message replaces.
    fn push_segments(&mut self, sysex: &[u8]) {
        self.queue.extend(
            payload::segments(sysex, MAX_PAYLOAD)
                .into_iter()
                .map(|s| (None, s)),
        );
    }

    /// Drops everything queued: the surface gets the whole state again
    /// once a session starts.
    pub fn clear(&mut self) {
        self.queue.clear();
    }

    fn refill(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.at) as f64;
        self.tokens = (self.tokens + elapsed * self.rate).min(MAX_PAYLOAD as f64);
        self.at = now;
    }

    /// The messages of the next packet, if the rate allows one now.
    pub fn take(&mut self, now: u64) -> Option<Vec<Vec<u8>>> {
        self.refill(now);
        let mut size = 0;
        let mut n = 0;
        for (_, m) in &self.queue {
            // A delta time octet before every message but the first.
            let cost = m.len() + usize::from(n > 0);
            if (size + cost) as f64 > self.tokens || size + cost > MAX_PAYLOAD {
                break;
            }
            size += cost;
            n += 1;
        }
        if n == 0 {
            return None;
        }
        self.tokens -= size as f64;
        Some(self.queue.drain(..n).map(|(_, m)| m).collect())
    }

    /// When [`Pacer::take`] returns a packet next; `None` with nothing
    /// queued.
    pub fn ready_at(&self, now: u64) -> Option<u64> {
        let (_, first) = self.queue.front()?;
        let have = self.tokens + now.saturating_sub(self.at) as f64 * self.rate;
        let missing = first.len() as f64 - have;
        Some(if missing <= 0.0 {
            now
        } else {
            now + (missing / self.rate).ceil() as u64
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcu::{self, Device, Key as McuKey, Led};

    /// Every packet, the rate allowing, in order.
    fn packets(p: &mut Pacer) -> Vec<Vec<Vec<u8>>> {
        let mut t = p.at;
        std::iter::from_fn(|| {
            t = p.ready_at(t)?;
            p.take(t)
        })
        .collect()
    }

    /// Every message, the rate allowing, in order.
    fn all(p: &mut Pacer) -> Vec<Vec<u8>> {
        packets(p).concat()
    }

    /// What a receiver makes of the packets, in order.
    fn decode(packets: &[Vec<Vec<u8>>]) -> Vec<Vec<u8>> {
        let mut d = payload::Decoder::default();
        let mut got = Vec::new();
        for (i, packet) in packets.iter().enumerate() {
            let h = payload::Header {
                seq: i as u16,
                timestamp: 0,
                ssrc: 1,
            };
            got.extend(d.decode(&payload::encode(h, packet)).unwrap().1);
        }
        got
    }

    #[test]
    fn newer_faders_leds_rings_meters_cells_and_colours_replace_queued_ones() {
        let mut p = Pacer::new(1_000_000);
        p.push(mcu::fader(0, 100));
        p.push(mcu::led(McuKey::Mute(0), Led::On));
        p.push(vec![0xd0, 0x05]);
        p.push(mcu::lcd(Device::Main, 0, "Chan 1 "));
        p.push(vec![0xb0, 0x10, 0x41]);
        p.push(mcu::ring(2, 0x16));
        p.push(mcu::colours(Device::Main, [1; 8]));
        p.push(mcu::colours(Device::Extender, [2; 8]));
        p.push(mcu::fader(1, 7));
        p.push(mcu::fader(0, 200));
        p.push(mcu::led(McuKey::Mute(0), Led::Off));
        p.push(vec![0xd0, 0x0c]);
        p.push(vec![0xd0, 0x13]);
        p.push(mcu::lcd(Device::Main, 0, "Chan 9 "));
        p.push(mcu::lcd(Device::Main, 7, "Chan 2 "));
        p.push(vec![0xb0, 0x10, 0x41]);
        p.push(mcu::ring(2, 0x18));
        p.push(mcu::colours(Device::Main, [3; 8]));
        assert_eq!(
            all(&mut p),
            vec![
                mcu::fader(0, 200),
                mcu::led(McuKey::Mute(0), Led::Off),
                vec![0xb0, 0x10, 0x41],
                mcu::ring(2, 0x18),
                mcu::colours(Device::Main, [3; 8]),
                mcu::colours(Device::Extender, [2; 8]),
                mcu::fader(1, 7),
                // A newer meter message goes last instead.
                vec![0xd0, 0x0c],
                vec![0xd0, 0x13],
                mcu::lcd(Device::Main, 0, "Chan 9 "),
                mcu::lcd(Device::Main, 7, "Chan 2 "),
                vec![0xb0, 0x10, 0x41],
            ]
        );
    }

    #[test]
    fn a_newer_timecode_digit_replaces_the_queued_one() {
        let mut p = Pacer::new(1_000_000);
        p.push(mcu::timecode_digit(0, b'7', false));
        p.push(mcu::timecode_digit(1, b'0', false));
        p.push(mcu::timecode_digit(0, b'8', false));
        assert_eq!(
            p.take(0).unwrap(),
            vec![
                mcu::timecode_digit(0, b'8', false),
                mcu::timecode_digit(1, b'0', false),
            ]
        );
    }

    #[test]
    fn a_meter_level_never_takes_the_place_of_the_overload_before_it() {
        let mut p = Pacer::new(1_000_000);
        p.push(mcu::meter(2, 11));
        p.push(mcu::meter_clip(2, true));
        p.push(mcu::meter_clip(2, false));
        p.push(mcu::meter(2, 5));
        assert_eq!(
            p.take(0).unwrap(),
            vec![mcu::meter_clip(2, false), mcu::meter(2, 5)]
        );
        p.push(mcu::meter_clip(2, true));
        p.push(mcu::meter(2, 14));
        p.push(mcu::meter_clip(2, false));
        p.push(mcu::meter(2, 5));
        assert_eq!(
            p.take(0).unwrap(),
            vec![mcu::meter_clip(2, false), mcu::meter(2, 5)]
        );
    }

    #[test]
    fn overlapping_scribble_strip_writes_keep_their_order() {
        let mut p = Pacer::new(1_000_000);
        // A line of 56 characters, 64 octets, still fits one packet.
        let line = |c: &str| mcu::lcd(Device::Main, 0, &c.repeat(56));
        assert_eq!(line("x").len(), MAX_PAYLOAD);
        p.push(mcu::lcd(Device::Main, 0, "Chan 1 "));
        p.push(line("x"));
        p.push(mcu::lcd(Device::Main, 0, "Chan 2 "));
        // Another device's cell is not covered.
        p.push(mcu::lcd(Device::Extender, 0, "Ext 1  "));
        p.push(line("y"));
        assert_eq!(
            all(&mut p),
            vec![mcu::lcd(Device::Extender, 0, "Ext 1  "), line("y")]
        );
        // The cell after the line still shows over it.
        p.push(line("z"));
        p.push(mcu::lcd(Device::Main, 7, "Chan 3 "));
        assert_eq!(
            all(&mut p),
            vec![line("z"), mcu::lcd(Device::Main, 7, "Chan 3 ")]
        );
        // One character more, 65 octets, goes in two segments.
        let long = mcu::lcd(Device::Main, 0, &"v".repeat(57));
        p.push(long.clone());
        let sent = packets(&mut p);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0][0].len(), MAX_PAYLOAD);
        assert_eq!(sent[1], vec![vec![0xf7, 0x76, 0xf7]]);
        assert_eq!(decode(&sent), vec![long]);
        // Both lines in one write, 120 octets, go in segments, after the
        // cells they cover are dropped.
        let both = mcu::lcd(Device::Main, 0, &"w".repeat(112));
        p.push(mcu::lcd(Device::Main, 0, "Chan 4 "));
        p.push(both.clone());
        let sent = packets(&mut p);
        assert_eq!(sent.len(), 2);
        assert_eq!(decode(&sent), vec![both]);
    }

    #[test]
    fn every_message_the_bridge_builds_queues_in_items_that_fit_a_packet() {
        // A queue head longer than a packet would never go, and nothing
        // queued behind it either.
        for message in [
            mcu::lcd(Device::Main, 0, &"a".repeat(56)),
            mcu::lcd(Device::Main, 0, &"a".repeat(57)),
            mcu::lcd(Device::Main, 0, &"a".repeat(112)),
            mcu::lcd(Device::Extender, 7, &"a".repeat(105)),
            mcu::meter(3, 11),
            mcu::meter_clip(3, true),
            mcu::fader(8, 0x3fff),
            mcu::ring(7, 0x16),
            mcu::led(McuKey::Mute(7), Led::On),
            mcu::colours(Device::Main, [mcu::WHITE; 8]),
            mcu::timecode_digit(9, b'2', false),
        ] {
            let mut p = Pacer::new(1_000_000);
            p.push(message.clone());
            assert!(
                p.queue.iter().all(|(_, m)| m.len() <= MAX_PAYLOAD),
                "{message:?}"
            );
        }
    }

    #[test]
    fn the_rate_spaces_packets_and_a_packet_holds_at_most_max_payload() {
        // 10 kB/s: 10 octets per millisecond.
        let mut p = Pacer::new(10_000);
        for i in 0..200 {
            p.push(vec![0xa0, i as u8 & 0x7f, 0]);
        }
        // The first packet is a full burst: 16 messages of 3 octets and 15
        // delta times make 63.
        let first = p.take(0).unwrap();
        assert_eq!(first.len(), 16);
        assert_eq!(p.take(0), None);
        // One octet left, two missing at 10 per millisecond.
        assert_eq!(p.ready_at(0), Some(200));
        // After 10 ms, 100 octets more, but never more than a packet.
        assert_eq!(p.take(10_000).unwrap().len(), 16);
        // The rest goes out as the rate allows: 600 octets in about 60 ms.
        let mut sent = 32;
        let mut t = 10_000;
        while let Some(at) = p.ready_at(t) {
            t = at;
            sent += p.take(t).map_or(0, |packet| packet.len());
        }
        assert_eq!(sent, 200);
        assert!((50_000..80_000).contains(&t), "{t}");
    }

    #[test]
    fn a_long_sysex_goes_in_segments_one_per_packet() {
        let mut p = Pacer::new(1_000_000);
        let mut long = vec![0xf0];
        long.extend(std::iter::repeat_n(0x41, 1200));
        long.push(0xf7);
        p.push(long.clone());
        let mut packets = Vec::new();
        let mut t = 0;
        while let Some(packet) = p.take(t) {
            assert_eq!(packet.len(), 1);
            assert!(packet[0].len() <= MAX_PAYLOAD);
            packets.push(packet);
            t += 1_000;
        }
        // 1200 data octets, 62 per segment.
        assert_eq!(packets.len(), 20);
        assert_eq!(decode(&packets), vec![long]);
    }
}
