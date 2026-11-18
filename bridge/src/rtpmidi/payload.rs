//! The RTP-MIDI payload (RFC 6295): an RTP header and a MIDI command
//! section. No recovery journal is sent; a received one is skipped.

/// The payload type AppleMIDI peers use.
pub const PAYLOAD_TYPE: u8 = 0x61;
/// The longest MIDI list a command section's long header describes.
pub const MAX_LIST: usize = 0x0fff;
/// The longest SysEx kept while its segments arrive.
const MAX_SYSEX: usize = 0x10000;

/// Why a packet was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

/// What an RTP header names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

/// A packet carrying `commands`, each a complete MIDI message or a SysEx
/// segment ([`segments`]), the first without a delta time and the others
/// at delta time 0. The MIDI list must fit [`MAX_LIST`].
pub fn encode(header: Header, commands: &[Vec<u8>]) -> Vec<u8> {
    let mut list = Vec::new();
    for (i, c) in commands.iter().enumerate() {
        if i > 0 {
            list.push(0);
        }
        list.extend_from_slice(c);
    }
    debug_assert!(list.len() <= MAX_LIST);
    let mut p = Vec::with_capacity(14 + list.len());
    p.extend_from_slice(&[0x80, PAYLOAD_TYPE]);
    p.extend_from_slice(&header.seq.to_be_bytes());
    p.extend_from_slice(&header.timestamp.to_be_bytes());
    p.extend_from_slice(&header.ssrc.to_be_bytes());
    if list.len() <= 0x0f {
        p.push(list.len() as u8);
    } else {
        p.push(0x80 | (list.len() >> 8) as u8 & 0x0f);
        p.push(list.len() as u8);
    }
    p.extend_from_slice(&list);
    p
}

/// A SysEx message (`F0 ... F7`) cut into segments of at most `max`
/// octets each (RFC 6295 section 3.2): `F0 ... F0`, then `F7 ... F0`,
/// last `F7 ... F7`. One segment when it fits.
pub fn segments(sysex: &[u8], max: usize) -> Vec<Vec<u8>> {
    if sysex.len() <= max || max < 3 {
        return vec![sysex.to_vec()];
    }
    let data = &sysex[1..sysex.len() - 1];
    let chunks: Vec<&[u8]> = data.chunks(max - 2).collect();
    let last = chunks.len() - 1;
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let mut s = Vec::with_capacity(chunk.len() + 2);
            s.push(if i == 0 { 0xf0 } else { 0xf7 });
            s.extend_from_slice(chunk);
            s.push(if i == last { 0xf7 } else { 0xf0 });
            s
        })
        .collect()
}

/// The number of data octets after a status octet; `None` for a status
/// that cannot start a command.
fn data_len(status: u8) -> Option<usize> {
    match status {
        0x80..=0xbf | 0xe0..=0xef => Some(2),
        0xc0..=0xdf => Some(1),
        0xf1 | 0xf3 => Some(1),
        0xf2 => Some(2),
        0xf6 | 0xf8..=0xff => Some(0),
        _ => None,
    }
}

/// Reads packets of one stream, keeping a SysEx whose segments span
/// packets.
#[derive(Debug, Default)]
pub struct Decoder {
    sysex: Option<Vec<u8>>,
}

impl Decoder {
    /// Drops a SysEx still waiting for segments, as after a lost packet.
    pub fn reset(&mut self) {
        self.sysex = None;
    }

    /// The header and the complete MIDI messages of one packet. A
    /// malformed packet is refused whole and changes nothing.
    pub fn decode(&mut self, packet: &[u8]) -> Result<(Header, Vec<Vec<u8>>), Malformed> {
        let (header, section) = rtp(packet)?;
        let mut sysex = self.sysex.clone();
        let messages = commands(section, &mut sysex)?;
        self.sysex = sysex;
        Ok((header, messages))
    }
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// The RTP header and what follows it, padding removed.
fn rtp(p: &[u8]) -> Result<(Header, &[u8]), Malformed> {
    if p.len() < 12 {
        return Err(Malformed("shorter than an RTP header"));
    }
    if p[0] >> 6 != 2 {
        return Err(Malformed("not RTP version 2"));
    }
    let header = Header {
        seq: be16(&p[2..]),
        timestamp: u32::from_be_bytes([p[4], p[5], p[6], p[7]]),
        ssrc: u32::from_be_bytes([p[8], p[9], p[10], p[11]]),
    };
    let mut at = 12 + 4 * usize::from(p[0] & 0x0f);
    if p[0] & 0x10 != 0 {
        let ext = p
            .get(at + 2..at + 4)
            .ok_or(Malformed("truncated header extension"))?;
        at += 4 + 4 * usize::from(be16(ext));
    }
    let mut end = p.len();
    if p[0] & 0x20 != 0 {
        let pad = usize::from(p[end - 1]);
        if pad == 0 || pad > end.saturating_sub(at) {
            return Err(Malformed("bad padding"));
        }
        end -= pad;
    }
    if at >= end {
        return Err(Malformed("no command section"));
    }
    Ok((header, &p[at..end]))
}

/// The MIDI list of a command section, decoded; the journal after it is
/// skipped.
fn commands(s: &[u8], sysex: &mut Option<Vec<u8>>) -> Result<Vec<Vec<u8>>, Malformed> {
    let h = s[0];
    let (len, at) = if h & 0x80 != 0 {
        let low = *s.get(1).ok_or(Malformed("truncated long header"))?;
        (usize::from(h & 0x0f) << 8 | usize::from(low), 2)
    } else {
        (usize::from(h & 0x0f), 1)
    };
    let list = s
        .get(at..at + len)
        .ok_or(Malformed("MIDI list longer than the packet"))?;
    let first_has_delta = h & 0x20 != 0;
    let mut out = Vec::new();
    let mut running: Option<u8> = None;
    let mut i = 0;
    let mut first = true;
    while i < list.len() {
        if !first || first_has_delta {
            let mut octets = 0;
            loop {
                let b = *list.get(i).ok_or(Malformed("truncated delta time"))?;
                i += 1;
                octets += 1;
                if b & 0x80 == 0 {
                    break;
                }
                if octets == 4 {
                    return Err(Malformed("delta time longer than 4 octets"));
                }
            }
        }
        first = false;
        let b = *list
            .get(i)
            .ok_or(Malformed("delta time without a command"))?;
        if b == 0xf0 || b == 0xf7 {
            i = segment(list, i, sysex, &mut out)?;
            running = None;
            continue;
        }
        let status = if b & 0x80 != 0 {
            i += 1;
            b
        } else {
            running.ok_or(Malformed("running status without a status"))?
        };
        let n = data_len(status).ok_or(Malformed("undefined status"))?;
        let data = list.get(i..i + n).ok_or(Malformed("truncated command"))?;
        if data.iter().any(|d| d & 0x80 != 0) {
            return Err(Malformed("status octet among data"));
        }
        i += n;
        match status {
            0x80..=0xef => running = Some(status),
            0xf0..=0xf7 => running = None,
            _ => {}
        }
        let mut m = Vec::with_capacity(n + 1);
        m.push(status);
        m.extend_from_slice(data);
        out.push(m);
    }
    Ok(out)
}

/// One SysEx command or segment starting at `list[i]`; returns where the
/// next delta time starts. Real-time messages inside it come out on
/// their own.
fn segment(
    list: &[u8],
    mut i: usize,
    sysex: &mut Option<Vec<u8>>,
    out: &mut Vec<Vec<u8>>,
) -> Result<usize, Malformed> {
    let start = list[i];
    i += 1;
    let mut data = Vec::new();
    let end = loop {
        let b = *list.get(i).ok_or(Malformed("unterminated SysEx"))?;
        i += 1;
        match b {
            0x00..=0x7f => data.push(b),
            0xf0 | 0xf4 | 0xf7 => break b,
            0xf8..=0xff => out.push(vec![b]),
            _ => return Err(Malformed("status octet inside SysEx")),
        }
    };
    match (start, end) {
        // Cancelled.
        (_, 0xf4) => *sysex = None,
        // Complete, or the first of several segments.
        (0xf0, _) => {
            let mut m = vec![0xf0];
            m.extend_from_slice(&data);
            if end == 0xf7 {
                m.push(0xf7);
                out.push(m);
                *sysex = None;
            } else {
                *sysex = Some(m);
            }
        }
        // A later segment; without its start (lost), it is dropped.
        _ => {
            if let Some(m) = sysex.as_mut() {
                m.extend_from_slice(&data);
                if m.len() > MAX_SYSEX {
                    *sysex = None;
                } else if end == 0xf7 {
                    let mut m = sysex.take().expect("checked");
                    m.push(0xf7);
                    out.push(m);
                }
            }
        }
    }
    Ok(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: Header = Header {
        seq: 0x1234,
        timestamp: 0xdead_beef,
        ssrc: 0x0102_0304,
    };

    /// An RTP header around `section`.
    fn packet(section: &[u8]) -> Vec<u8> {
        let mut p = vec![0x80, 0x61, 0x12, 0x34, 0xde, 0xad, 0xbe, 0xef, 1, 2, 3, 4];
        p.extend_from_slice(section);
        p
    }

    fn decode(section: &[u8]) -> Result<Vec<Vec<u8>>, Malformed> {
        Decoder::default().decode(&packet(section)).map(|(_, m)| m)
    }

    #[test]
    fn a_short_list_takes_the_short_header_and_a_long_one_the_long() {
        let p = encode(H, &[vec![0x90, 18, 0x7f], vec![0xe0, 0x10, 0x20]]);
        assert_eq!(&p[..12], &packet(&[])[..]);
        // 3 + delta 0 + 3 = 7 octets: B=0, LEN=7.
        assert_eq!(&p[12..], &[7, 0x90, 18, 0x7f, 0, 0xe0, 0x10, 0x20]);
        let lcd: Vec<u8> = [0xf0, 0, 0, 0x66, 0x14, 0x12, 0]
            .into_iter()
            .chain([b'x'; 56])
            .chain([0xf7])
            .collect();
        let p = encode(H, std::slice::from_ref(&lcd));
        assert_eq!(&p[12..14], &[0x80, 64]);
        let (h, m) = Decoder::default().decode(&p).unwrap();
        assert_eq!(h, H);
        assert_eq!(m, vec![lcd]);
    }

    #[test]
    fn delta_times_of_one_to_four_octets_are_skipped() {
        // Z=1: the first command has a delta time too.
        let m = decode(&[
            0x20 | 12,
            0x81,
            0x00,
            0x90,
            1,
            2,
            0xff,
            0xff,
            0xff,
            0x7f,
            0x80,
            3,
            4,
        ])
        .unwrap();
        assert_eq!(m, vec![vec![0x90, 1, 2], vec![0x80, 3, 4]]);
        // A fifth octet is refused.
        let e = decode(&[8, 0x90, 1, 2, 0x81, 0x81, 0x81, 0x81, 0x00]).unwrap_err();
        assert_eq!(e.0, "delta time longer than 4 octets");
    }

    #[test]
    fn running_status_repeats_the_last_channel_status_within_a_packet() {
        let m = decode(&[11, 0xe1, 0x10, 0x20, 0, 0x11, 0x21, 0, 0xd0, 0x15, 0, 0x16]).unwrap();
        assert_eq!(
            m,
            vec![
                vec![0xe1, 0x10, 0x20],
                vec![0xe1, 0x11, 0x21],
                vec![0xd0, 0x15],
                vec![0xd0, 0x16]
            ]
        );
        // Real-time messages leave it alone; system common ones end it.
        let m = decode(&[8, 0x90, 1, 2, 0, 0xf8, 0, 3, 4]).unwrap();
        assert_eq!(m, vec![vec![0x90, 1, 2], vec![0xf8], vec![0x90, 3, 4]]);
        let e = decode(&[8, 0x90, 1, 2, 0, 0xf6, 0, 3, 4]).unwrap_err();
        assert_eq!(e.0, "running status without a status");
        // A packet cannot start on running status.
        assert_eq!(
            decode(&[2, 1, 2]).unwrap_err().0,
            "running status without a status"
        );
    }

    #[test]
    fn a_sysex_in_segments_is_put_together_across_packets() {
        let whole: Vec<u8> = [0xf0, 0, 0, 0x66, 0x14, 0x12, 0]
            .into_iter()
            .chain(b"Chan 1 Chan 2 Chan 3".iter().copied())
            .chain([0xf7])
            .collect();
        let parts = segments(&whole, 10);
        assert!(parts.len() > 2);
        assert_eq!(parts[0][0], 0xf0);
        assert_eq!(*parts[0].last().unwrap(), 0xf0);
        assert_eq!(parts[1][0], 0xf7);
        assert_eq!(*parts[1].last().unwrap(), 0xf0);
        assert_eq!(*parts.last().unwrap().last().unwrap(), 0xf7);
        assert!(parts.iter().all(|p| p.len() <= 10));
        assert_eq!(segments(&whole, 100), vec![whole.clone()]);

        let mut d = Decoder::default();
        let mut got = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            let p = encode(
                Header { seq: i as u16, ..H },
                &[part.clone(), vec![0x90, 1, 2]],
            );
            let (_, m) = d.decode(&p).unwrap();
            got.extend(m);
        }
        let n = parts.len();
        // The note after every segment comes through; the SysEx once, whole.
        assert_eq!(got.iter().filter(|m| **m == [0x90, 1, 2]).count(), n);
        assert_eq!(
            got.iter().filter(|m| m[0] == 0xf0).collect::<Vec<_>>(),
            vec![&whole]
        );
        assert_eq!(got[n - 1], whole);
    }

    #[test]
    fn a_cancelled_or_orphaned_segment_is_dropped() {
        let mut d = Decoder::default();
        let p = |s: &[u8]| {
            let mut v = vec![s.len() as u8];
            v.extend_from_slice(s);
            packet(&v)
        };
        assert_eq!(
            d.decode(&p(&[0xf0, 1, 2, 0xf0])).unwrap().1,
            Vec::<Vec<u8>>::new()
        );
        assert_eq!(
            d.decode(&p(&[0xf7, 3, 0xf4])).unwrap().1,
            Vec::<Vec<u8>>::new()
        );
        // Cancelled: the end has nothing to finish.
        assert_eq!(
            d.decode(&p(&[0xf7, 4, 0xf7])).unwrap().1,
            Vec::<Vec<u8>>::new()
        );
        // After a reset (a lost packet) a later segment is dropped too.
        d.decode(&p(&[0xf0, 1, 0xf0])).unwrap();
        d.reset();
        assert_eq!(
            d.decode(&p(&[0xf7, 2, 0xf7])).unwrap().1,
            Vec::<Vec<u8>>::new()
        );
        // A real-time message inside a SysEx comes out on its own.
        assert_eq!(
            d.decode(&p(&[0xf0, 1, 0xfe, 2, 0xf7])).unwrap().1,
            vec![vec![0xfe], vec![0xf0, 1, 2, 0xf7]]
        );
    }

    #[test]
    fn a_journal_after_the_list_is_skipped() {
        // J=1, LEN=3, then journal octets that would not parse as MIDI.
        let m = decode(&[0x40 | 3, 0x90, 5, 6, 0x80, 0x00, 0xff, 0x13]).unwrap();
        assert_eq!(m, vec![vec![0x90, 5, 6]]);
    }

    #[test]
    fn csrcs_extensions_and_padding_are_skipped() {
        let mut p = vec![0xb1, 0x61, 0, 1, 0, 0, 0, 0, 0, 0, 0, 9];
        p.extend_from_slice(&[0xaa; 4]); // one CSRC
        p.extend_from_slice(&[0, 0, 0, 1, 0xbb, 0xbb, 0xbb, 0xbb]); // extension
        p.extend_from_slice(&[3, 0x90, 7, 8]);
        p.extend_from_slice(&[0, 0, 3]); // padding
        let (h, m) = Decoder::default().decode(&p).unwrap();
        assert_eq!(h.ssrc, 9);
        assert_eq!(m, vec![vec![0x90, 7, 8]]);
    }

    #[test]
    fn malformed_packets_are_refused_and_change_nothing() {
        let mut d = Decoder::default();
        d.decode(&packet(&[3, 0xf0, 1, 0xf0])).unwrap();
        for (bad, why) in [
            (vec![0x80, 0x61, 0, 1], "shorter than an RTP header"),
            (
                [0x40]
                    .into_iter()
                    .chain(packet(&[1, 0xf8]).into_iter().skip(1))
                    .collect(),
                "not RTP version 2",
            ),
            (packet(&[]), "no command section"),
            (packet(&[0x85]), "truncated long header"),
            (packet(&[5, 0x90, 1]), "MIDI list longer than the packet"),
            (packet(&[2, 0x90, 1]), "truncated command"),
            (packet(&[3, 0x90, 0x80, 1]), "status octet among data"),
            (packet(&[2, 0xf0, 1]), "unterminated SysEx"),
            (packet(&[3, 0xf0, 0x90, 0xf7]), "status octet inside SysEx"),
            (packet(&[1, 0xf4]), "undefined status"),
            (packet(&[4, 0x90, 1, 2, 0x80]), "truncated delta time"),
            (
                packet(&[4, 0x90, 1, 2, 0x00]),
                "delta time without a command",
            ),
        ] {
            assert_eq!(d.decode(&bad).unwrap_err().0, why, "{bad:02x?}");
        }
        let mut padded = packet(&[1, 0xf8]);
        padded[0] |= 0x20;
        padded.push(0);
        assert_eq!(d.decode(&padded).unwrap_err().0, "bad padding");
        // The SysEx begun before the refused packets is still pending.
        assert_eq!(
            d.decode(&packet(&[3, 0xf7, 2, 0xf7])).unwrap().1,
            vec![vec![0xf0, 1, 2, 0xf7]]
        );
    }

    #[test]
    fn no_input_panics_the_decoder() {
        // A fixed pseudo-random sequence of packets, with valid headers so
        // the command section parser sees most of them.
        let mut x: u32 = 0x9e37_79b9;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let mut d = Decoder::default();
        for _ in 0..20_000 {
            let len = (next() % 40) as usize;
            let mut p: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if next() % 2 == 0 && p.len() > 1 {
                p[0] = 0x80 | p[0] & 0x3f;
            }
            if next() % 2 == 0 {
                p = packet(&p);
            }
            let _ = d.decode(&p);
        }
    }
}
