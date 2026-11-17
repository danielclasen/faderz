//! A software MCU surface on the far end of an in-process [`midi::pair`]:
//! it sends what a hand on an X-Touch sends (touch, fader moves, key
//! presses) and keeps what the bridge sets (motor positions, LEDs, the
//! scribble strip), for tests without hardware.
//!
//! [`midi::pair`]: crate::midi::pair

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::mcu::{self, Device, Key, Led};
use crate::midi::{self, Port};

pub struct Emulator {
    pub device: Device,
    to_bridge: UnboundedSender<Vec<u8>>,
    from_bridge: UnboundedReceiver<Vec<u8>>,
    /// Motor positions, 14-bit; 8 is the master.
    pub faders: [u16; 9],
    pub leds: HashMap<u8, Led>,
    /// The two lines of the scribble strip.
    pub lcd: [u8; 2 * mcu::LINE_CHARS],
    /// Every message the bridge sent, in order.
    pub received: Vec<Vec<u8>>,
}

impl Emulator {
    /// A surface and the bridge's port to it.
    pub fn new(device: Device) -> (Emulator, Port) {
        let (port, to_bridge, from_bridge) = midi::pair();
        let emulator = Emulator {
            device,
            to_bridge,
            from_bridge,
            faders: [0; 9],
            leds: HashMap::new(),
            lcd: [b' '; 2 * mcu::LINE_CHARS],
            received: Vec::new(),
        };
        (emulator, port)
    }

    /// Sends one raw message, as recorded from a surface.
    pub fn send(&self, message: &[u8]) {
        let _ = self.to_bridge.send(message.to_vec());
    }

    pub fn touch(&self, strip: u8) {
        self.send(&[0x90, 104 + strip, 0x7f]);
    }

    pub fn untouch(&self, strip: u8) {
        self.send(&[0x90, 104 + strip, 0x00]);
    }

    /// A hand moves a fader: the surface reports the position.
    pub fn move_fader(&mut self, strip: u8, value: u16) {
        self.faders[usize::from(strip)] = value;
        self.send(&mcu::fader(strip, value));
    }

    pub fn press(&self, key: Key) {
        self.send(&[0x90, key.note(), 0x7f]);
        self.send(&[0x90, key.note(), 0x00]);
    }

    /// Takes in what the bridge sent so far; returns how many messages.
    pub fn pump(&mut self) -> usize {
        let mut n = 0;
        while let Ok(message) = self.from_bridge.try_recv() {
            self.take(&message);
            self.received.push(message);
            n += 1;
        }
        n
    }

    fn take(&mut self, message: &[u8]) {
        match message {
            [status, lsb, msb] if status & 0xf0 == 0xe0 => {
                let strip = usize::from(status & 0x0f);
                if strip < self.faders.len() {
                    self.faders[strip] = u16::from(*lsb) | u16::from(*msb) << 7;
                }
            }
            [0x90, note, velocity] => {
                let led = match velocity {
                    0 => Led::Off,
                    1 => Led::Blink,
                    _ => Led::On,
                };
                self.leds.insert(*note, led);
            }
            _ => {
                if let Some((device, offset, text)) = mcu::parse_lcd(message)
                    && device == self.device
                {
                    for (i, b) in text.bytes().enumerate() {
                        if let Some(c) = self.lcd.get_mut(offset + i) {
                            *c = b;
                        }
                    }
                }
            }
        }
    }

    /// The text of one strip on one line of the scribble strip.
    pub fn cell(&self, line: usize, strip: usize) -> String {
        let at = line * mcu::LINE_CHARS + strip * mcu::STRIP_CHARS;
        String::from_utf8_lossy(&self.lcd[at..at + mcu::STRIP_CHARS]).into_owned()
    }

    pub fn led(&self, key: Key) -> Led {
        self.leds.get(&key.note()).copied().unwrap_or(Led::Off)
    }

    /// Takes in what the bridge sends until `done` holds, for at most
    /// `within`.
    pub async fn until(
        &mut self,
        within: Duration,
        mut done: impl FnMut(&Emulator) -> bool,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            self.pump();
            if done(self) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
