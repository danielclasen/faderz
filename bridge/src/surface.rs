//! The mapping between MCU surfaces and the faderz state: one or more
//! units side by side (an X-Touch and its Extenders) over the channels,
//! banked together; faders write while touched, motors follow the state
//! while not; mute, solo and select keys; names and values on the
//! scribble strips. Every unit is a console of its own on the server.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::engine::{Engine, Event};
use crate::mcu::{self, Device, Input, Key, Led};
use crate::taper;

/// How often a motor fader is moved at most.
pub const MOTOR_INTERVAL_MS: u64 = 20;
/// A refused touch held this long asks the holder to give the control up.
pub const REQUEST_AFTER_MS: u64 = 1000;

/// One physical unit.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitSpec {
    /// The console's id on the server (model `Id`).
    pub id: String,
    pub name: String,
    /// The main unit has the master fader; an extender has none.
    pub device: Device,
}

#[derive(Debug, Clone)]
struct Touch {
    path: String,
    /// The holder's short name once the server refused the touch or the
    /// control went to someone else.
    refused: Option<String>,
    since: u64,
    requested: bool,
}

#[derive(Debug, Default)]
struct Shown {
    faders: [Option<u16>; 9],
    moved_at: [Option<u64>; 9],
    leds: HashMap<u8, Led>,
    /// 16 cells: 8 per line.
    cells: [Option<String>; 16],
}

struct Unit {
    spec: UnitSpec,
    /// Index of its first strip among all strips, left to right.
    offset: usize,
    touches: [Option<Touch>; 9],
    shown: Shown,
}

pub struct Surface {
    units: Vec<Unit>,
    /// Index of the channel on the leftmost strip.
    bank: usize,
    /// The master fader's control; else the program bus's fader.
    master: Option<String>,
    selected: Option<String>,
}

/// A short name for the holder of a control: its principal, or its kind
/// while every session is `anonymous`.
pub fn short_name(owner: &Value) -> String {
    match owner["principal"].as_str() {
        Some(p) if p != "anonymous" => p.to_owned(),
        _ => match owner["kind"].as_str() {
            Some("ui") => "screen".to_owned(),
            Some(kind) => kind.to_owned(),
            None => "taken".to_owned(),
        },
    }
}

impl Surface {
    pub fn new(units: Vec<UnitSpec>, master: Option<String>) -> Surface {
        let units = units
            .into_iter()
            .enumerate()
            .map(|(i, spec)| Unit {
                spec,
                offset: i * usize::from(mcu::STRIPS),
                touches: Default::default(),
                shown: Shown::default(),
            })
            .collect();
        Surface {
            units,
            bank: 0,
            master,
            selected: None,
        }
    }

    pub fn bank(&self) -> usize {
        self.bank
    }

    fn strips(&self) -> usize {
        self.units.len() * usize::from(mcu::STRIPS)
    }

    /// The channel ids of the shape, in order.
    fn channels(engine: &Engine) -> Vec<String> {
        engine.mirror().shape["channels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["id"].as_str().map(str::to_owned))
            .collect()
    }

    fn channel_name(engine: &Engine, id: &str) -> String {
        engine.mirror().shape["channels"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| c["id"] == id)
            .and_then(|c| c["name"].as_str())
            .unwrap_or(id)
            .to_owned()
    }

    /// The channel on strip `strip` of unit `unit` at the current bank.
    fn channel(&self, engine: &Engine, unit: usize, strip: u8) -> Option<String> {
        let index = self.bank + self.units[unit].offset + usize::from(strip);
        Self::channels(engine).into_iter().nth(index)
    }

    /// The master fader's control: the configured one, else the fader of
    /// the shape's program bus.
    pub fn master_path(&self, engine: &Engine) -> Option<String> {
        if let Some(m) = &self.master {
            return Some(m.clone());
        }
        engine.mirror().shape["buses"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|b| b["kind"] == "program")
            .and_then(|b| b["id"].as_str())
            .map(|id| format!("/buses/{id}/fader"))
    }

    fn fader_path(&self, engine: &Engine, unit: usize, strip: u8) -> Option<String> {
        if strip == mcu::MASTER {
            return (self.units[unit].spec.device == Device::Main)
                .then(|| self.master_path(engine))
                .flatten();
        }
        self.channel(engine, unit, strip)
            .map(|c| format!("/channels/{c}/fader"))
    }

    /// The `console` message of every unit.
    pub fn consoles(&self) -> Vec<Value> {
        self.units
            .iter()
            .map(|u| {
                json!({
                    "type": "console",
                    "id": u.spec.id,
                    "name": u.spec.name,
                    "protocol": "mcu",
                    "transport": "bridge",
                    "strips": mcu::STRIPS,
                    "bankOffset": self.bank + u.offset,
                })
            })
            .collect()
    }

    /// Registers every unit as a console with the engine.
    pub fn register(&self, engine: &mut Engine, now: u64) {
        for c in self.consoles() {
            engine.console(c, now);
        }
    }

    /// The controls held by a touched fader.
    pub fn touching(&self) -> HashSet<String> {
        self.units
            .iter()
            .flat_map(|u| u.touches.iter().flatten())
            .filter(|t| t.refused.is_none())
            .map(|t| t.path.clone())
            .collect()
    }

    /// One MIDI message from unit `unit`.
    pub fn input(&mut self, unit: usize, message: &[u8], engine: &mut Engine, now: u64) {
        let Some(input) = mcu::parse(message) else {
            return;
        };
        match input {
            Input::Touch { strip, down: true } => {
                let Some(path) = self.fader_path(engine, unit, strip) else {
                    return;
                };
                engine.touch(&path, now);
                self.units[unit].touches[usize::from(strip)] = Some(Touch {
                    path,
                    refused: None,
                    since: now,
                    requested: false,
                });
            }
            Input::Touch { strip, down: false } => {
                let u = &mut self.units[unit];
                if let Some(t) = u.touches[usize::from(strip)].take()
                    && t.refused.is_none()
                {
                    engine.release(&t.path, now);
                }
                // The motor goes to the value shown once let go.
                u.shown.faders[usize::from(strip)] = None;
                u.shown.moved_at[usize::from(strip)] = None;
            }
            // A fader that moves without a touch is a motor's echo or a
            // surface without touch sense: never written.
            Input::Fader { strip, value } => {
                if let Some(t) = &self.units[unit].touches[usize::from(strip)]
                    && t.refused.is_none()
                {
                    engine.set(&t.path, json!(taper::fader_to_db(value)), now);
                }
            }
            Input::Key { key, down: true } => self.key(unit, key, engine, now),
            Input::Key { down: false, .. } => {}
        }
    }

    fn key(&mut self, unit: usize, key: Key, engine: &mut Engine, now: u64) {
        let strip_channel = |s: u8| self.channel(engine, unit, s);
        match key {
            Key::Mute(s) => {
                if let Some(c) = strip_channel(s) {
                    let path = format!("/channels/{c}/mute");
                    let on = engine.display(&path).and_then(Value::as_bool) == Some(true);
                    engine.set(&path, json!(!on), now);
                }
            }
            Key::Solo(s) => {
                if let Some(c) = strip_channel(s) {
                    let path = format!("/channels/{c}/solo");
                    let off = engine.display(&path).and_then(Value::as_str) == Some("off");
                    engine.set(&path, json!(if off { "pfl" } else { "off" }), now);
                }
            }
            Key::Select(s) => {
                if let Some(c) = strip_channel(s) {
                    self.selected = (self.selected.as_deref() != Some(c.as_str())).then_some(c);
                    engine.select(self.selected.as_deref());
                }
            }
            // Mapped, and left without an action until recording exists.
            Key::Rec(_) => {}
            Key::BankLeft => self.move_bank(-(self.strips() as isize), engine, now),
            Key::BankRight => self.move_bank(self.strips() as isize, engine, now),
            Key::ChannelLeft => self.move_bank(-1, engine, now),
            Key::ChannelRight => self.move_bank(1, engine, now),
            Key::Other(note) => tracing::debug!(note, "key not mapped"),
        }
    }

    fn move_bank(&mut self, by: isize, engine: &mut Engine, now: u64) {
        let last = Self::channels(engine).len().saturating_sub(1);
        let bank = self.bank.saturating_add_signed(by).min(last);
        if bank != self.bank {
            self.bank = bank;
            self.register(engine, now);
        }
    }

    /// What the engine reported.
    pub fn event(&mut self, event: &Event, engine: &mut Engine, now: u64) {
        match event {
            Event::Synced => {
                let touching = self.touching();
                engine.reconcile(&touching, now);
            }
            Event::Refused { path, owner } => {
                for t in self
                    .units
                    .iter_mut()
                    .flat_map(|u| u.touches.iter_mut().flatten())
                {
                    if t.path == *path && t.refused.is_none() {
                        t.refused = Some(short_name(owner));
                    }
                }
            }
            Event::Requested { path, from } => {
                tracing::info!(path, from = %short_name(from), "asked to give up a control");
            }
            Event::Error(e) => tracing::warn!("{e}"),
        }
    }

    /// Asks for refused controls held for a second, and notices a touched
    /// control that went to another session.
    pub fn tick(&mut self, engine: &mut Engine, now: u64) {
        for t in self
            .units
            .iter_mut()
            .flat_map(|u| u.touches.iter_mut().flatten())
        {
            if t.refused.is_none()
                && !engine.touch_pending(&t.path)
                && let Some(owner) = engine.owner(&t.path).filter(|o| !engine.mine(o))
            {
                t.refused = Some(short_name(owner));
            }
            if t.refused.is_some() && !t.requested && now >= t.since + REQUEST_AFTER_MS {
                t.requested = true;
                engine.request(&t.path);
            }
        }
    }

    /// The MIDI messages that bring every unit to what the state and the
    /// touches show, per unit; only what changed since the last call.
    pub fn render(&mut self, engine: &Engine, now: u64) -> Vec<(usize, Vec<u8>)> {
        let mut out = Vec::new();
        for i in 0..self.units.len() {
            let view: Vec<StripView> = (0..=mcu::MASTER)
                .map(|s| self.strip_view(engine, i, s))
                .collect();
            let u = &mut self.units[i];
            for (s, v) in view.iter().enumerate() {
                if s == usize::from(mcu::MASTER) && u.spec.device != Device::Main {
                    continue;
                }
                if u.touches[s].is_none()
                    && u.shown.faders[s] != Some(v.fader)
                    && u.shown.moved_at[s].is_none_or(|t| now >= t + MOTOR_INTERVAL_MS)
                {
                    out.push((i, mcu::fader(s as u8, v.fader)));
                    u.shown.faders[s] = Some(v.fader);
                    u.shown.moved_at[s] = Some(now);
                }
                if s == usize::from(mcu::MASTER) {
                    continue;
                }
                let s8 = s as u8;
                for (key, led) in [
                    (Key::Rec(s8), Led::Off),
                    (Key::Solo(s8), v.solo),
                    (Key::Mute(s8), v.mute),
                    (Key::Select(s8), v.select),
                ] {
                    if u.shown.leds.get(&key.note()) != Some(&led) {
                        out.push((i, mcu::led(key, led)));
                        u.shown.leds.insert(key.note(), led);
                    }
                }
                for (line, text) in [(0, &v.name), (1, &v.value)] {
                    let cell = line * 8 + s;
                    if u.shown.cells[cell].as_ref() != Some(text) {
                        let offset = line * mcu::LINE_CHARS + s * mcu::STRIP_CHARS;
                        out.push((i, mcu::lcd(u.spec.device, offset, text)));
                        u.shown.cells[cell] = Some(text.clone());
                    }
                }
            }
        }
        out
    }

    fn strip_view(&self, engine: &Engine, unit: usize, strip: u8) -> StripView {
        let blank = StripView {
            fader: 0,
            mute: Led::Off,
            solo: Led::Off,
            select: Led::Off,
            name: mcu::cell(""),
            value: mcu::cell(""),
        };
        let Some(fader) = self.fader_path(engine, unit, strip) else {
            return blank;
        };
        let db = engine
            .display(&fader)
            .and_then(Value::as_f64)
            .unwrap_or(taper::OFF_DB);
        let touch = self.units[unit].touches[usize::from(strip)].as_ref();
        let holder = match touch.and_then(|t| t.refused.clone()) {
            Some(name) => Some(name),
            None => engine
                .owner(&fader)
                .filter(|o| !engine.mine(o))
                .map(short_name),
        };
        if strip == mcu::MASTER {
            return StripView {
                fader: taper::db_to_fader(db),
                ..blank
            };
        }
        let channel = self.channel(engine, unit, strip).expect("a fader path");
        let at = |leaf: &str| {
            engine
                .display(&format!("/channels/{channel}/{leaf}"))
                .cloned()
        };
        let on = |b: bool| if b { Led::On } else { Led::Off };
        StripView {
            fader: taper::db_to_fader(db),
            mute: on(at("mute") == Some(json!(true))),
            solo: on(at("solo").is_some_and(|s| s != "off")),
            select: if touch.is_some_and(|t| t.refused.is_some()) {
                Led::Blink
            } else {
                on(self.selected.as_deref() == Some(channel.as_str()))
            },
            name: mcu::cell(&Self::channel_name(engine, &channel)),
            value: mcu::cell(&holder.unwrap_or_else(|| taper::format_db(db))),
        }
    }
}

struct StripView {
    fader: u16,
    mute: Led,
    solo: Led,
    select: Led,
    name: String,
    value: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Action;
    use crate::engine::tests::{live, sent};

    fn units(n: usize) -> Vec<UnitSpec> {
        (0..n)
            .map(|i| UnitSpec {
                id: format!("u{i}"),
                name: format!("Unit {i}"),
                device: if i == 0 {
                    Device::Main
                } else {
                    Device::Extender
                },
            })
            .collect()
    }

    fn sent_types(actions: &[Action]) -> Vec<(String, String)> {
        sent(actions, 1)
            .into_iter()
            .filter(|m| m["type"] != "ping")
            .map(|m| {
                (
                    m["type"].as_str().unwrap().to_owned(),
                    m["path"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn a_touched_fader_writes_and_a_released_one_moves_to_the_state() {
        let mut e = live(1);
        let mut s = Surface::new(units(1), None);
        s.render(&e, 0);
        s.input(0, &[0xe0, 0, 0x60], &mut e, 1);
        assert!(
            sent_types(&e.take_actions()).is_empty(),
            "no write without a touch"
        );
        s.input(0, &[0x90, 104, 0x7f], &mut e, 2);
        s.input(0, &[0xe0, 0x7f, 0x5f], &mut e, 3);
        let out = s.render(&e, 4);
        assert!(
            !out.iter().any(|(_, m)| m[0] == 0xe0),
            "no motor move while touched"
        );
        s.input(0, &[0x90, 104, 0], &mut e, 30);
        assert_eq!(
            sent_types(&e.take_actions()),
            vec![
                ("touch".into(), "/channels/ch1/fader".into()),
                ("set".into(), "/channels/ch1/fader".into()),
                ("release".into(), "/channels/ch1/fader".into()),
            ]
        );
        // The value written is shown until answered: the motor goes there.
        let out = s.render(&e, 31);
        let db = e
            .display("/channels/ch1/fader")
            .and_then(Value::as_f64)
            .unwrap();
        assert!(out.contains(&(0, mcu::fader(0, taper::db_to_fader(db)))));
    }

    #[test]
    fn a_refused_touch_shows_the_holder_writes_nothing_and_asks_after_a_second() {
        let mut e = live(1);
        let mut s = Surface::new(units(1), None);
        s.input(0, &[0x90, 104, 0x7f], &mut e, 0);
        let owner = json!({ "principal": "ana", "handle": "aaaaaaaaaaaaaaaaaaaaaa", "kind": "ui", "connection": "c" });
        s.event(
            &Event::Refused {
                path: "/channels/ch1/fader".into(),
                owner,
            },
            &mut e,
            5,
        );
        e.take_actions();
        s.input(0, &[0xe0, 0, 0x40], &mut e, 6);
        assert!(sent_types(&e.take_actions()).is_empty());
        let out = s.render(&e, 7);
        assert!(out.contains(&(0, mcu::lcd(Device::Main, 56, "ana    "))));
        assert!(out.contains(&(0, mcu::led(Key::Select(0), Led::Blink))));
        s.tick(&mut e, 999);
        assert!(sent_types(&e.take_actions()).is_empty());
        s.tick(&mut e, 1000);
        assert_eq!(
            sent_types(&e.take_actions()),
            vec![("request".into(), "/channels/ch1/fader".into())]
        );
        s.input(0, &[0x90, 104, 0], &mut e, 1100);
        assert!(
            sent_types(&e.take_actions()).is_empty(),
            "nothing to release"
        );
    }

    #[test]
    fn keys_toggle_mute_and_solo_and_select_and_rec_does_nothing() {
        let mut e = live(1);
        let mut s = Surface::new(units(1), None);
        s.input(0, &[0x90, 16, 0x7f], &mut e, 0);
        s.input(0, &[0x90, 8, 0x7f], &mut e, 1);
        s.input(0, &[0x90, 0, 0x7f], &mut e, 2);
        s.input(0, &[0x90, 24, 0x7f], &mut e, 3);
        let out: Vec<Value> = sent(&e.take_actions(), 1);
        assert_eq!(out[0]["path"], "/channels/ch1/mute");
        assert_eq!(out[0]["value"], true);
        assert_eq!(out[1]["path"], "/channels/ch1/solo");
        assert_eq!(out[1]["value"], "pfl");
        assert_eq!(out[2], json!({ "type": "select", "element": "ch1" }));
        assert_eq!(out.len(), 3);
        let leds = s.render(&e, 4);
        for led in [
            mcu::led(Key::Mute(0), Led::On),
            mcu::led(Key::Solo(0), Led::On),
            mcu::led(Key::Select(0), Led::On),
            mcu::led(Key::Rec(0), Led::Off),
        ] {
            assert!(leds.contains(&(0, led.clone())), "{led:?}");
        }
    }

    #[test]
    fn banks_move_by_all_strips_and_clamp_and_strips_past_the_end_are_dark() {
        let mut e = live(1);
        let mut s = Surface::new(units(2), None);
        s.register(&mut e, 0);
        s.input(1, &[0x90, 47, 0x7f], &mut e, 1);
        assert_eq!(s.bank(), 0, "one channel: the last bank is 0");
        let out = s.render(&e, 2);
        // Unit 0 strip 0 shows the channel, the extender is blank.
        assert!(out.contains(&(0, mcu::lcd(Device::Main, 0, "Mic 1  "))));
        assert!(out.contains(&(1, mcu::lcd(Device::Extender, 0, "       "))));
        assert!(out.contains(&(1, mcu::fader(0, 0))));
        assert!(
            !out.iter().any(|(u, m)| *u == 1 && m[0] == 0xe8),
            "an extender has no master"
        );
        let consoles: Vec<Value> = sent(&e.take_actions(), 1)
            .into_iter()
            .filter(|m| m["type"] == "console")
            .collect();
        assert_eq!(consoles.len(), 2);
        assert_eq!(consoles[1]["bankOffset"], 8);
        assert_eq!(consoles[1]["id"], "u1");
    }

    #[test]
    fn the_master_fader_follows_the_program_bus_or_the_configured_control() {
        let e = live(1);
        let s = Surface::new(units(1), None);
        assert_eq!(s.master_path(&e), None, "the test shape has no program bus");
        let s = Surface::new(units(1), Some("/monitor/level".into()));
        assert_eq!(s.master_path(&e).as_deref(), Some("/monitor/level"));
    }

    #[test]
    fn after_a_sync_touched_faders_are_touched_again_and_the_rest_released() {
        let mut e = live(1);
        let mut s = Surface::new(units(1), None);
        s.input(0, &[0x90, 104, 0x7f], &mut e, 0);
        let touch = sent(&e.take_actions(), 1).remove(0);
        let me =
            json!({ "principal": "p", "handle": e.handle(), "kind": "console", "connection": "c" });
        let delta = json!({ "type": "delta", "replica": { "id": "r1", "role": "leader" }, "revision": 6,
            "owners": [{ "path": "/channels/ch1/fader", "owner": me }],
            "origins": [{ "owner": me, "commandId": touch["id"], "seq": touch["seq"] }] });
        e.received(1, &delta.to_string(), 1);
        assert!(!e.touch_pending("/channels/ch1/fader"));
        // A new connection's snapshot: the fader's ownership lapsed, the
        // mute is still this session's.
        let snapshot = json!({ "type": "snapshot", "replica": { "id": "r1", "role": "leader" },
            "you": {}, "epoch": "1", "revision": 9,
            "shape": { "channels": [{ "id": "ch1", "name": "Mic 1" }], "buses": [] },
            "state": { "channels": { "ch1": { "fader": -90, "mute": false, "solo": "off" } }, "consoles": [] },
            "owners": { "/channels/ch1/mute": me } });
        e.received(1, &snapshot.to_string(), 2);
        for ev in e.take_events() {
            s.event(&ev, &mut e, 3);
        }
        assert_eq!(
            sent_types(&e.take_actions()),
            vec![
                ("release".into(), "/channels/ch1/mute".into()),
                ("touch".into(), "/channels/ch1/fader".into()),
            ]
        );
    }

    #[test]
    fn short_names_name_the_principal_or_the_kind() {
        assert_eq!(
            short_name(&json!({ "principal": "dana", "kind": "ui" })),
            "dana"
        );
        assert_eq!(
            short_name(&json!({ "principal": "anonymous", "kind": "ui" })),
            "screen"
        );
        assert_eq!(
            short_name(&json!({ "principal": "anonymous", "kind": "console" })),
            "console"
        );
    }
}
