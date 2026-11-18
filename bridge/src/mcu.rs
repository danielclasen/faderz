//! The Mackie Control (MCU) messages the bridge reads from and writes to a
//! surface: 14-bit faders as pitch bend, fader touch, the strip and bank
//! keys as notes, their LEDs, and the scribble strip text as SysEx.
//!
//! The values follow the published reverse-engineered MCU reference; they
//! are not checked against a unit here.

/// Strips of an X-Touch or an X-Touch Extender, not counting a master.
pub const STRIPS: u8 = 8;
/// The master fader's index on an X-Touch: pitch bend channel 8, touch
/// note 112.
pub const MASTER: u8 = 8;
/// Characters per strip on one line of the scribble strips.
pub const STRIP_CHARS: usize = 7;
/// Characters per line of the scribble strip buffer.
pub const LINE_CHARS: usize = 56;

/// The SysEx device id of a unit: the main unit or an extender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Main,
    Extender,
}

impl Device {
    fn id(self) -> u8 {
        match self {
            Device::Main => 0x14,
            Device::Extender => 0x15,
        }
    }
}

/// A key of the surface, by what the bridge does with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Rec(u8),
    Solo(u8),
    Mute(u8),
    Select(u8),
    BankLeft,
    BankRight,
    ChannelLeft,
    ChannelRight,
    /// F1 to F8 as 0 to 7.
    Function(u8),
    /// Any other note: not mapped.
    Other(u8),
}

impl Key {
    pub fn from_note(note: u8) -> Key {
        match note {
            0..=7 => Key::Rec(note),
            8..=15 => Key::Solo(note - 8),
            16..=23 => Key::Mute(note - 16),
            24..=31 => Key::Select(note - 24),
            46 => Key::BankLeft,
            47 => Key::BankRight,
            48 => Key::ChannelLeft,
            49 => Key::ChannelRight,
            54..=61 => Key::Function(note - 54),
            n => Key::Other(n),
        }
    }

    /// The note of the key, which also addresses its LED.
    pub fn note(self) -> u8 {
        match self {
            Key::Rec(s) => s,
            Key::Solo(s) => 8 + s,
            Key::Mute(s) => 16 + s,
            Key::Select(s) => 24 + s,
            Key::BankLeft => 46,
            Key::BankRight => 47,
            Key::ChannelLeft => 48,
            Key::ChannelRight => 49,
            Key::Function(f) => 54 + f,
            Key::Other(n) => n,
        }
    }
}

/// What a surface sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A fader moved; `strip` 8 is the master.
    Fader {
        strip: u8,
        value: u16,
    },
    /// A fader was touched (`down`) or let go.
    Touch {
        strip: u8,
        down: bool,
    },
    Key {
        key: Key,
        down: bool,
    },
}

/// Reads one MIDI message; anything the bridge does not map is `None`.
pub fn parse(message: &[u8]) -> Option<Input> {
    let (&status, data) = message.split_first()?;
    match (status & 0xf0, data) {
        (0xe0, &[lsb, msb]) => {
            let strip = status & 0x0f;
            (strip <= MASTER).then_some(Input::Fader {
                strip,
                value: u16::from(lsb & 0x7f) | u16::from(msb & 0x7f) << 7,
            })
        }
        // Note off, or note on with velocity 0, is a release.
        (0x90 | 0x80, &[note, velocity]) => {
            let down = status & 0xf0 == 0x90 && velocity > 0;
            match note {
                104..=112 => Some(Input::Touch {
                    strip: note - 104,
                    down,
                }),
                _ => Some(Input::Key {
                    key: Key::from_note(note),
                    down,
                }),
            }
        }
        _ => None,
    }
}

/// Moves a motor fader to a 14-bit value.
pub fn fader(strip: u8, value: u16) -> Vec<u8> {
    let value = value.min(crate::taper::FADER_MAX);
    vec![
        0xe0 | (strip & 0x0f),
        (value & 0x7f) as u8,
        (value >> 7) as u8,
    ]
}

/// The state of a key LED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Led {
    Off,
    Blink,
    On,
}

/// Sets a key LED.
pub fn led(key: Key, state: Led) -> Vec<u8> {
    let velocity = match state {
        Led::Off => 0,
        Led::Blink => 1,
        Led::On => 0x7f,
    };
    vec![0x90, key.note(), velocity]
}

/// Writes `text` into the scribble strip buffer at `offset` (0..112; the
/// second line starts at 56). Characters outside printable ASCII are
/// shown as `?`.
pub fn lcd(device: Device, offset: usize, text: &str) -> Vec<u8> {
    let mut out = vec![0xf0, 0x00, 0x00, 0x66, device.id(), 0x12, offset as u8];
    out.extend(text.chars().map(|c| {
        if c.is_ascii_graphic() || c == ' ' {
            c as u8
        } else {
            b'?'
        }
    }));
    out.push(0xf7);
    out
}

/// What [`lcd`] wrote, read back: device, offset and text.
pub fn parse_lcd(message: &[u8]) -> Option<(Device, usize, String)> {
    let body = message.strip_prefix(&[0xf0, 0x00, 0x00, 0x66])?;
    let device = match body.first()? {
        0x14 => Device::Main,
        0x15 => Device::Extender,
        _ => return None,
    };
    let rest = body[1..].strip_prefix(&[0x12])?;
    let (&offset, text) = rest.split_first()?;
    let text = text.strip_suffix(&[0xf7])?;
    Some((
        device,
        usize::from(offset),
        String::from_utf8_lossy(text).into(),
    ))
}

/// `text` cut or padded with spaces to one strip's width.
pub fn cell(text: &str) -> String {
    let text: String = text.chars().take(STRIP_CHARS).collect();
    format!("{text:<width$}", width = STRIP_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faders_are_14_bit_pitch_bend_with_the_master_on_channel_8() {
        assert_eq!(
            parse(&[0xe3, 0x7f, 0x5f]),
            Some(Input::Fader {
                strip: 3,
                value: 0x2fff
            })
        );
        assert_eq!(
            parse(&[0xe8, 0x00, 0x7f]),
            Some(Input::Fader {
                strip: 8,
                value: 0x3f80
            })
        );
        assert_eq!(parse(&[0xe9, 0, 0]), None);
        assert_eq!(fader(3, 0x2fff), vec![0xe3, 0x7f, 0x5f]);
        assert_eq!(fader(8, 0xffff), vec![0xe8, 0x7f, 0x7f]);
    }

    #[test]
    fn touch_notes_and_keys_are_read_with_note_off_as_release() {
        assert_eq!(
            parse(&[0x90, 104, 0x7f]),
            Some(Input::Touch {
                strip: 0,
                down: true
            })
        );
        assert_eq!(
            parse(&[0x90, 112, 0]),
            Some(Input::Touch {
                strip: 8,
                down: false
            })
        );
        assert_eq!(
            parse(&[0x80, 107, 0x40]),
            Some(Input::Touch {
                strip: 3,
                down: false
            })
        );
        let key = |note| match parse(&[0x90, note, 0x7f]) {
            Some(Input::Key { key, down: true }) => key,
            other => panic!("{other:?}"),
        };
        assert_eq!(key(2), Key::Rec(2));
        assert_eq!(key(9), Key::Solo(1));
        assert_eq!(key(23), Key::Mute(7));
        assert_eq!(key(24), Key::Select(0));
        assert_eq!(key(46), Key::BankLeft);
        assert_eq!(key(49), Key::ChannelRight);
        assert_eq!(key(50), Key::Other(50));
        assert_eq!(key(53), Key::Other(53));
        for (f, note) in (54..=61).enumerate() {
            assert_eq!(key(note), Key::Function(f as u8));
            assert_eq!(
                parse(&[0x80, note, 0x40]),
                Some(Input::Key {
                    key: Key::Function(f as u8),
                    down: false
                })
            );
        }
        assert_eq!(key(62), Key::Other(62));
        for note in 0..=103 {
            assert_eq!(Key::from_note(note).note(), note);
        }
    }

    #[test]
    fn leds_and_the_scribble_strip_are_written_as_notes_and_sysex() {
        assert_eq!(led(Key::Mute(2), Led::On), vec![0x90, 18, 0x7f]);
        assert_eq!(led(Key::Select(0), Led::Blink), vec![0x90, 24, 1]);
        assert_eq!(led(Key::Solo(7), Led::Off), vec![0x90, 15, 0]);
        assert_eq!(led(Key::Function(7), Led::Blink), vec![0x90, 61, 1]);
        let m = lcd(Device::Extender, 56 + 7, "Vox  \u{e9}");
        assert_eq!(
            m,
            vec![
                0xf0, 0, 0, 0x66, 0x15, 0x12, 63, b'V', b'o', b'x', b' ', b' ', b'?', 0xf7
            ]
        );
        assert_eq!(parse_lcd(&m), Some((Device::Extender, 63, "Vox  ?".into())));
        assert_eq!(cell("Presenter 1"), "Present");
        assert_eq!(cell("Mic"), "Mic    ");
    }
}
