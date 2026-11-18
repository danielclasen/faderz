//! Paces what goes to a network surface: a byte rate with bursts of one
//! packet, and a queue in which a newer motor position, LED or meter
//! level replaces the queued one it supersedes, and a scribble strip
//! write drops the queued ones it overwrites whole.

use std::collections::VecDeque;

use super::payload;

/// The most MIDI list octets in one packet.
pub const MAX_PAYLOAD: usize = 512;

/// What makes a queued message stale once a newer one comes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    /// A motor fader: pitch bend per channel.
    Fader(u8),
    /// A key LED: note per channel.
    Led(u8, u8),
    /// A meter level: channel pressure carries the strip in its high
    /// nibble.
    Meter(u8, u8),
    /// A scribble strip write: device, first and last position.
    Lcd(u8, usize, usize),
}

fn key(m: &[u8]) -> Option<Key> {
    match m {
        [s, _, _] if s & 0xf0 == 0xe0 => Some(Key::Fader(*s)),
        [s, note, _] if s & 0xf0 == 0x90 => Some(Key::Led(*s, *note)),
        [s, level] if s & 0xf0 == 0xd0 => Some(Key::Meter(*s, level >> 4)),
        [0xf0, 0, 0, 0x66, device, 0x12, offset, text @ .., 0xf7] if !text.is_empty() => {
            let first = usize::from(*offset);
            Some(Key::Lcd(*device, first, first + text.len() - 1))
        }
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
            self.queue.extend(
                payload::segments(&message, MAX_PAYLOAD)
                    .into_iter()
                    .map(|s| (None, s)),
            );
        } else {
            self.queue.push_back((k, message));
        }
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

    #[test]
    fn newer_faders_leds_meters_and_cells_replace_queued_ones_in_place() {
        let mut p = Pacer::new(1_000_000);
        p.push(mcu::fader(0, 100));
        p.push(mcu::led(McuKey::Mute(0), Led::On));
        p.push(vec![0xd0, 0x05]);
        p.push(mcu::lcd(Device::Main, 0, "Chan 1 "));
        p.push(vec![0xb0, 0x10, 0x41]);
        p.push(mcu::fader(1, 7));
        p.push(mcu::fader(0, 200));
        p.push(mcu::led(McuKey::Mute(0), Led::Off));
        p.push(vec![0xd0, 0x0c]);
        p.push(vec![0xd0, 0x13]);
        p.push(mcu::lcd(Device::Main, 0, "Chan 9 "));
        p.push(mcu::lcd(Device::Main, 7, "Chan 2 "));
        p.push(vec![0xb0, 0x10, 0x41]);
        let packet = p.take(0).unwrap();
        assert_eq!(
            packet,
            vec![
                mcu::fader(0, 200),
                mcu::led(McuKey::Mute(0), Led::Off),
                vec![0xd0, 0x0c],
                vec![0xb0, 0x10, 0x41],
                mcu::fader(1, 7),
                vec![0xd0, 0x13],
                mcu::lcd(Device::Main, 0, "Chan 9 "),
                mcu::lcd(Device::Main, 7, "Chan 2 "),
                vec![0xb0, 0x10, 0x41],
            ]
        );
        assert_eq!(p.take(0), None);
    }

    #[test]
    fn overlapping_scribble_strip_writes_keep_their_order() {
        let mut p = Pacer::new(1_000_000);
        let line = |c: &str| mcu::lcd(Device::Main, 0, &c.repeat(56));
        p.push(mcu::lcd(Device::Main, 0, "Chan 1 "));
        p.push(line("x"));
        p.push(mcu::lcd(Device::Main, 0, "Chan 2 "));
        // Another device's cell is not covered.
        p.push(mcu::lcd(Device::Extender, 0, "Ext 1  "));
        p.push(line("y"));
        assert_eq!(
            p.take(0).unwrap(),
            vec![mcu::lcd(Device::Extender, 0, "Ext 1  "), line("y")]
        );
        // The cell after the line still shows over it.
        p.push(line("z"));
        p.push(mcu::lcd(Device::Main, 7, "Chan 3 "));
        assert_eq!(
            p.take(0).unwrap(),
            vec![line("z"), mcu::lcd(Device::Main, 7, "Chan 3 ")]
        );
    }

    #[test]
    fn the_rate_spaces_packets_and_a_packet_holds_at_most_max_payload() {
        // 10 kB/s: 10 octets per millisecond.
        let mut p = Pacer::new(10_000);
        for i in 0..200 {
            p.push(vec![0xb0, i as u8 & 0x7f, 0]);
        }
        // The first packet is a full burst: 128 messages of 3 octets and
        // 127 delta times make 511.
        let first = p.take(0).unwrap();
        assert_eq!(first.len(), 128);
        assert_eq!(p.take(0), None);
        assert_eq!(p.ready_at(0), Some(200));
        // After 10 ms, 100 octets: 25 messages (25 * 3 + 24 = 99).
        assert_eq!(p.take(10_000).unwrap().len(), 25);
        assert_eq!(p.take(1_000_000).unwrap().len(), 47);
        assert_eq!(p.ready_at(1_000_000), None);
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
        assert_eq!(packets.len(), 3);
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
        assert_eq!(got, vec![long]);
    }
}
