//! The Mackie Control (MCU) messages the bridge reads from and writes to a
//! surface: 14-bit faders as pitch bend, fader touch, the strip and bank
//! keys as notes, their LEDs, the V-Pots and their LED rings as control
//! changes, and the scribble strip text as SysEx.
//!
//! The values follow the published reverse-engineered MCU reference; they
//! are not checked against a unit here. The function keys F1 to F8 at
//! notes 54 to 61 (0x36 to 0x3d) are the notes Ardour's Mackie Control
//! surface (`libs/surfaces/mackie/device_info.cc`) and REAPER's
//! (`csurf_mcu.cpp` in the REAPER SDK) read.

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
    /// A V-Pot pushed.
    Pot(u8),
    BankLeft,
    BankRight,
    ChannelLeft,
    ChannelRight,
    /// ALT, a modifier.
    Alt,
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
            32..=39 => Key::Pot(note - 32),
            46 => Key::BankLeft,
            47 => Key::BankRight,
            48 => Key::ChannelLeft,
            49 => Key::ChannelRight,
            54..=61 => Key::Function(note - 54),
            73 => Key::Alt,
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
            Key::Pot(s) => 32 + s,
            Key::BankLeft => 46,
            Key::BankRight => 47,
            Key::ChannelLeft => 48,
            Key::ChannelRight => 49,
            Key::Function(f) => 54 + f,
            Key::Alt => 73,
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
    /// A V-Pot turned by `ticks` detents, clockwise above 0.
    Pot {
        strip: u8,
        ticks: i8,
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
        // A V-Pot turn: bit 6 sets anticlockwise, the low six bits count.
        // A turn of no detent says nothing.
        (0xb0, &[cc @ 16..=23, value]) if value & 0x3f != 0 => {
            let ticks = (value & 0x3f) as i8;
            Some(Input::Pot {
                strip: cc - 16,
                ticks: if value & 0x40 != 0 { -ticks } else { ticks },
            })
        }
        _ => None,
    }
}

/// The V-Pot ring value for a pan of -1 to 1 from the centre out: mode 1
/// (boost/cut) and LED 1 to 11, as far left as right of the centre.
/// LED 6, the centre alone, is the exact centre only, so an operator
/// can tell a pan just off it. `None` is a dark ring.
pub fn ring_value(pan: Option<f64>) -> u8 {
    pan.map_or(0, |p| {
        let steps = (p.clamp(-1.0, 1.0).abs() * 5.0).round().max(1.0) as i8;
        let led = match p {
            0.0 => 6,
            p if p > 0.0 => 6 + steps,
            _ => 6 - steps,
        };
        0x10 | led as u8
    })
}

/// Sets V-Pot ring `strip` to a [`ring_value`].
pub fn ring(strip: u8, value: u8) -> Vec<u8> {
    vec![0xb0, 0x30 + strip, value]
}

/// The meter level lighting 0 to 7 of an X-Touch's 7 meter LEDs, which
/// lights one LED per two of the levels 0 to 12.
const LED_LEVELS: [u8; 8] = [0, 1, 3, 5, 7, 9, 11, 12];

/// The meter level of a strip for a height of 0 to 1 on the console's
/// meter scale, lighting as many of 7 LEDs; 14, the overload, while
/// `over`.
pub fn meter_level(height: f64, over: bool) -> u8 {
    if over {
        return 14;
    }
    LED_LEVELS[(height * 7.0).floor().clamp(0.0, 7.0) as usize]
}

/// Sets meter `strip` to a [`meter_level`]: channel pressure, the strip
/// in the high nibble. An X-Touch lets the LEDs fall by themselves, so a
/// level is sent again while the signal lasts.
pub fn meter(strip: u8, level: u8) -> Vec<u8> {
    vec![0xd0, strip << 4 | level]
}

/// Sets the overload LED (CLIP) of meter `strip`: level 14 on, 15 off,
/// as the MCU reference has it. An X-Touch lights CLIP at 14 and 15
/// alike and lets it fall by itself; it takes 15 as a level and shows the
/// level sent right after it in its place.
pub fn meter_clip(strip: u8, on: bool) -> Vec<u8> {
    meter(strip, if on { 14 } else { 15 })
}

/// Digits of the timecode display, position 0 the rightmost.
pub const TIMECODE_DIGITS: usize = 10;

/// Sets timecode display digit `position` (0 the rightmost) to `ch`, an
/// ASCII digit or space, with its dot lit when `dot`: control change
/// 0x40 + position, the character's low six bits, 0x40 for the dot.
pub fn timecode_digit(position: u8, ch: u8, dot: bool) -> Vec<u8> {
    vec![
        0xb0,
        0x40 + position,
        ch & 0x3f | if dot { 0x40 } else { 0 },
    ]
}

/// The timecode display showing the time of day of `unix` seconds at
/// `offset` minutes east of UTC, `HH.MM.SS` on the hours, minutes and
/// seconds digits: the digit and dot of each position, rightmost first.
pub fn clock_digits(unix: u64, offset: i32) -> [(u8, bool); TIMECODE_DIGITS] {
    let day = (unix as i64 + i64::from(offset) * 60).rem_euclid(86_400);
    let (h, m, s) = (day / 3600, day / 60 % 60, day % 60);
    let d = |n: i64| b'0' + n as u8;
    let blank = (b' ', false);
    [
        blank,
        blank,
        blank,
        (d(s % 10), false),
        (d(s / 10), false),
        (d(m % 10), true),
        (d(m / 10), false),
        (d(h % 10), true),
        (d(h / 10), false),
        blank,
    ]
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

/// The X-Touch display colour of a channel's `ScribbleColor`: bit 0 red,
/// bit 1 green, bit 2 blue. An X-Touch has 7; orange shows as yellow, its
/// nearest, and `none` or an unknown colour as [`WHITE`].
pub fn colour(name: &str) -> u8 {
    match name {
        "red" => 1,
        "green" => 2,
        "orange" | "yellow" => 3,
        "blue" => 4,
        "purple" => 5,
        "cyan" => 6,
        _ => WHITE,
    }
}

/// The [`colour`] of a strip without a channel.
pub const WHITE: u8 = 7;

/// Sets the display colours of a unit's 8 scribble strips: an X-Touch
/// SysEx outside the Mackie Control protocol, one [`colour`] per strip.
pub fn colours(device: Device, codes: [u8; 8]) -> Vec<u8> {
    let mut out = vec![0xf0, 0x00, 0x00, 0x66, device.id(), 0x72];
    out.extend_from_slice(&codes);
    out.push(0xf7);
    out
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
        assert_eq!(key(32), Key::Pot(0));
        assert_eq!(key(39), Key::Pot(7));
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
        assert_eq!(key(73), Key::Alt);
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

    #[test]
    fn v_pots_turn_as_relative_control_changes_and_light_their_rings() {
        assert_eq!(
            parse(&[0xb0, 16, 1]),
            Some(Input::Pot { strip: 0, ticks: 1 })
        );
        assert_eq!(
            parse(&[0xb0, 23, 0x43]),
            Some(Input::Pot {
                strip: 7,
                ticks: -3
            })
        );
        assert_eq!(parse(&[0xb0, 24, 1]), None, "CC 24 is no V-Pot");
        assert_eq!(
            parse(&[0xb0, 48, 1]),
            None,
            "the ring is written, never read"
        );
        assert_eq!(parse(&[0xb0, 16, 0x00]), None, "no detent");
        assert_eq!(parse(&[0xb0, 16, 0x40]), None, "no detent");
        assert_eq!(ring_value(Some(-1.0)), 0x11);
        assert_eq!(ring_value(Some(0.0)), 0x16);
        assert_eq!(ring_value(Some(1.0)), 0x1b);
        assert_eq!(ring_value(Some(2.0)), 0x1b, "clamped");
        assert_eq!(ring_value(None), 0);
        // The same distance from the centre lights the same step either
        // way.
        assert_eq!(ring_value(Some(0.1)), 0x17);
        assert_eq!(ring_value(Some(-0.1)), 0x15);
        // Only the exact centre lights the centre alone.
        assert_eq!(ring_value(Some(0.01)), 0x17);
        assert_eq!(ring_value(Some(-0.01)), 0x15);
        assert_eq!(ring_value(Some(0.3)), 0x18);
        assert_eq!(ring(0, 0x11), vec![0xb0, 48, 0x11]);
        assert_eq!(ring(7, 0), vec![0xb0, 55, 0]);
    }

    #[test]
    fn the_timecode_display_shows_the_time_of_day_at_an_offset() {
        // 2026-11-26 09:05:07 UTC.
        let unix = 1_795_683_907;
        let shown = |d: [(u8, bool); TIMECODE_DIGITS]| -> String {
            d.iter()
                .rev()
                .map(|&(c, dot)| format!("{}{}", c as char, if dot { "." } else { "" }))
                .collect()
        };
        assert_eq!(shown(clock_digits(unix, 0)), " 09.05.07   ");
        assert_eq!(shown(clock_digits(unix, 60)), " 10.05.07   ");
        assert_eq!(
            shown(clock_digits(unix, -600)),
            " 23.05.07   ",
            "the day before"
        );
        assert_eq!(timecode_digit(7, b'9', true), vec![0xb0, 0x47, 0x79]);
        assert_eq!(timecode_digit(9, b' ', false), vec![0xb0, 0x49, 0x20]);
    }

    #[test]
    fn meters_light_one_x_touch_led_per_two_levels_and_14_is_the_overload() {
        // As tried on an X-Touch: 0 to 7 LEDs at levels 0, 1, 3, 5, 7, 9,
        // 11 and 12.
        assert_eq!(meter_level(0.0, false), 0);
        assert_eq!(meter_level(0.14, false), 0);
        assert_eq!(meter_level(1.0 / 7.0, false), 1);
        assert_eq!(meter_level(0.5, false), 5);
        assert_eq!(meter_level(0.99, false), 11);
        assert_eq!(meter_level(1.0, false), 12);
        assert_eq!(meter_level(0.5, true), 14);
        assert_eq!(meter(3, 9), vec![0xd0, 0x39]);
        assert_eq!(meter_clip(3, true), vec![0xd0, 0x3e]);
        assert_eq!(meter_clip(7, false), vec![0xd0, 0x7f]);
    }

    #[test]
    fn scribble_strip_colours_are_rgb_bits_in_one_x_touch_sysex() {
        let names = [
            "none", "red", "orange", "yellow", "green", "cyan", "blue", "purple",
        ];
        let codes: Vec<u8> = names.iter().map(|n| colour(n)).collect();
        assert_eq!(codes, vec![7, 1, 3, 3, 2, 6, 4, 5]);
        assert_eq!(
            colour("pink"),
            7,
            "a colour faderz may add later shows white"
        );
        assert_eq!(
            colours(Device::Extender, [1, 2, 3, 4, 5, 6, 7, 7]),
            vec![0xf0, 0, 0, 0x66, 0x15, 0x72, 1, 2, 3, 4, 5, 6, 7, 7, 0xf7]
        );
    }
}
