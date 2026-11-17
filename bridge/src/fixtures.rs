//! Replays the MIDI fixtures in tests/fixtures through the surface
//! mapping and a live engine, checking what goes to faderz and back to the
//! surface. The fixtures follow the published MCU message formats; none
//! was captured from a unit.

use serde_json::{Value, json};

use crate::engine::tests::{engine, sent};
use crate::engine::{Action, Engine};
use crate::mcu::Device;
use crate::surface::{Surface, UnitSpec};

fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).expect("a hex byte"))
        .collect()
}

/// A live engine on a shape of 20 channels named "Chan 1" to "Chan 20"
/// and a program bus.
fn twenty() -> Engine {
    let channels: Vec<Value> = (1..=20)
        .map(|i| json!({ "id": format!("ch{i}"), "name": format!("Chan {i}") }))
        .collect();
    let state: serde_json::Map<String, Value> = (1..=20)
        .map(|i| {
            (
                format!("ch{i}"),
                json!({ "fader": -90, "mute": false, "solo": "off" }),
            )
        })
        .collect();
    let snapshot = json!({
        "type": "snapshot", "replica": { "id": "r1", "role": "leader" },
        "you": {}, "epoch": "1", "revision": 1,
        "shape": { "channels": channels, "buses": [{ "id": "pgm", "kind": "program" }] },
        "state": { "channels": state, "buses": { "pgm": { "fader": 0 } }, "consoles": [] },
        "owners": {},
    });
    let mut e = engine(1);
    e.start(0);
    e.opened(1, 0);
    e.received(1, &snapshot.to_string(), 0);
    e
}

struct Replay {
    engine: Engine,
    surface: Surface,
    now: u64,
    revision: u64,
    /// Bridge to surface since the last `<` check.
    midi: Vec<Vec<u8>>,
    /// Bridge to faderz since the last `!` check.
    commands: Vec<Value>,
}

impl Replay {
    fn new() -> Replay {
        let mut engine = twenty();
        let unit = UnitSpec {
            id: "xt".into(),
            name: "X-Touch".into(),
            device: Device::Main,
        };
        let surface = Surface::new(vec![unit], None);
        surface.register(&mut engine, 0);
        engine.take_actions();
        engine.take_events();
        Replay {
            engine,
            surface,
            now: 0,
            revision: 1,
            midi: Vec::new(),
            commands: Vec::new(),
        }
    }

    /// Carries the engine's events and actions over, and renders.
    fn settle(&mut self) {
        for event in self.engine.take_events() {
            self.surface.event(&event, &mut self.engine, self.now);
        }
        let actions: Vec<Action> = self.engine.take_actions();
        self.commands.extend(
            sent(&actions, 1)
                .into_iter()
                .filter(|m| !matches!(m["type"].as_str(), Some("ping" | "pong"))),
        );
        self.midi.extend(
            self.surface
                .render(&self.engine, self.now)
                .into_iter()
                .map(|(_, m)| m),
        );
    }

    /// Time passes in steps of 5 ms; faderz keeps beating.
    fn pass(&mut self, ms: u64) {
        let end = self.now + ms;
        while self.now < end {
            self.now = (self.now + 5).min(end);
            if self.now.is_multiple_of(100) {
                let ping = json!({ "type": "ping", "replica": { "id": "r1", "role": "leader" }, "t": self.now });
                self.engine.received(1, &ping.to_string(), self.now);
            }
            self.engine.tick(self.now);
            self.surface.tick(&mut self.engine, self.now);
            self.settle();
        }
    }

    fn command(m: &Value) -> String {
        let t = m["type"].as_str().unwrap_or_default();
        match t {
            "select" => format!("select {}", m["element"].as_str().unwrap_or("null")),
            "console" => format!("console {} {}", m["id"].as_str().unwrap(), m["bankOffset"]),
            "set" => {
                let v = &m["value"];
                let v = match v.as_f64() {
                    Some(f) if v.is_number() => format!("{f}"),
                    _ => v.to_string(),
                };
                format!("set {} {v}", m["path"].as_str().unwrap())
            }
            _ => format!("{t} {}", m["path"].as_str().unwrap_or_default()),
        }
    }

    fn line(&mut self, name: &str, n: usize, line: &str) {
        let at = format!("{name}:{n}: {line}");
        let (op, rest) = line.split_at(1);
        let rest = rest.trim();
        match op {
            ">" => {
                self.surface
                    .input(0, &hex(rest), &mut self.engine, self.now);
                self.settle();
            }
            "+" => self.pass(rest.parse().expect("milliseconds")),
            "@" => {
                let mut delta: Value = serde_json::from_str(rest).expect("a delta");
                self.revision += 1;
                delta["type"] = json!("delta");
                delta["replica"] = json!({ "id": "r1", "role": "leader" });
                delta["revision"] = json!(self.revision);
                self.engine.received(1, &delta.to_string(), self.now);
                self.settle();
            }
            "!" => {
                let got: Vec<String> = self
                    .commands
                    .drain(..)
                    .map(|m| Replay::command(&m))
                    .collect();
                let want: Vec<String> = if rest == "none" {
                    vec![]
                } else {
                    vec![rest.to_owned()]
                };
                assert_eq!(got, want, "{at}");
            }
            "<" => {
                let midi = std::mem::take(&mut self.midi);
                if let Some(status) = rest.strip_prefix("none-of ") {
                    let status = hex(status)[0];
                    assert!(
                        !midi.iter().any(|m| m[0] == status),
                        "{at}: sent {midi:02x?}"
                    );
                } else {
                    for want in rest.split(',').map(hex) {
                        assert!(midi.contains(&want), "{at}: sent {midi:02x?}");
                    }
                }
            }
            _ => panic!("{at}: unknown line"),
        }
    }
}

fn replay(name: &str, text: &str) {
    let mut r = Replay::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        r.line(name, i + 1, line);
    }
}

#[test]
fn a_fader_ride() {
    replay(
        "fader-ride.mcu",
        include_str!("../tests/fixtures/fader-ride.mcu"),
    );
}

#[test]
fn strip_keys_and_their_leds() {
    replay("keys.mcu", include_str!("../tests/fixtures/keys.mcu"));
}

#[test]
fn banking_and_the_scribble_strip() {
    replay("bank.mcu", include_str!("../tests/fixtures/bank.mcu"));
}
