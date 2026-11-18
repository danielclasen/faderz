//! The mapping between MCU surfaces and the faderz state: one or more
//! units side by side (an X-Touch and its Extenders) over the channels,
//! banked together; faders write while touched, motors follow the state
//! while not; mute, solo and select keys; duck keys; names and values on
//! the scribble strips. Every unit is a console of its own on the server.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::engine::{Engine, Event};
use crate::mcu::{self, Device, Input, Key, Led};
use crate::midi::Link;
use crate::taper;

/// How often a motor fader is moved at most.
pub const MOTOR_INTERVAL_MS: u64 = 20;
/// A refused touch held this long asks the holder to give the control up.
pub const REQUEST_AFTER_MS: u64 = 1000;
/// After messages from a network surface went missing, a touched fader
/// that does not move for this long is taken as let go: its release may
/// have been among them.
pub const STALE_TOUCH_MS: u64 = 1500;
/// A duck key let go sooner than this after its press was a tap, as the
/// web UI's talk key tells a tap from a hold.
pub const LATCH_MS: u64 = 300;

/// What a duck key does with its group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyAction {
    /// Pressed, it holds the group's `hold`; a tap then toggles its
    /// `mode` between `auto` and `on`, and a longer press lets the hold
    /// go when the key does.
    #[default]
    HoldOrLatch,
    /// Holds the group's `hold` while the key is down.
    Hold,
    /// Every press toggles the group's `mode` between `auto` and `on`.
    Latch,
}

/// A key bound to a duck group.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DuckKey {
    /// The group's id in the shape.
    pub duck: String,
    #[serde(default)]
    pub action: KeyAction,
}

/// The duck keys of an X-Touch by note. Without a map of its own the
/// surface has F1 to F8 on the shape's duck groups in order.
pub type DuckKeys = BTreeMap<u8, DuckKey>;

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
    /// Since when the touch may have been let go unseen.
    suspect: Option<u64>,
}

/// A duck key held down.
#[derive(Debug, Clone)]
struct Press {
    duck: String,
    action: KeyAction,
    since: u64,
    /// The holder's short name once the server refused the hold or the
    /// hold went to someone else.
    refused: Option<String>,
    /// Since when the key may have been let go unseen.
    suspect: Option<u64>,
}

impl Press {
    /// Whether the press holds its group's `hold`: a hold key's press
    /// the server did not refuse.
    fn holds(&self) -> bool {
        self.action != KeyAction::Latch && self.refused.is_none()
    }
}

fn hold_path(duck: &str) -> String {
    format!("/ducks/{duck}/hold")
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
    /// Duck keys held down, by note.
    presses: BTreeMap<u8, Press>,
    shown: Shown,
}

pub struct Surface {
    units: Vec<Unit>,
    /// Index of the channel on the leftmost strip.
    bank: usize,
    /// The master fader's control; else the program bus's fader.
    master: Option<String>,
    selected: Option<String>,
    /// Duck keys of their own; else F1 to F8.
    keys: Option<DuckKeys>,
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
                presses: BTreeMap::new(),
                shown: Shown::default(),
            })
            .collect();
        Surface {
            units,
            bank: 0,
            master,
            selected: None,
            keys: None,
        }
    }

    /// Binds the X-Touch's duck keys by `keys` instead of F1 to F8.
    pub fn with_keys(mut self, keys: Option<DuckKeys>) -> Surface {
        self.keys = keys;
        self
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

    /// The duck groups of the shape, id and name, in order.
    fn ducks(engine: &Engine) -> Vec<(String, String)> {
        engine.mirror().shape["ducks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| {
                let id = d["id"].as_str()?;
                Some((id.to_owned(), d["name"].as_str().unwrap_or(id).to_owned()))
            })
            .collect()
    }

    /// The notes of unit `unit`'s duck keys: an X-Touch's, none on an
    /// Extender, which has no function keys.
    fn duck_notes(&self, unit: usize) -> Vec<u8> {
        if self.units[unit].spec.device != Device::Main {
            return Vec::new();
        }
        match &self.keys {
            Some(keys) => keys.keys().copied().collect(),
            None => (0..8).map(|f| Key::Function(f).note()).collect(),
        }
    }

    /// The duck group key `note` of unit `unit` is bound to, when the
    /// shape has it.
    fn duck_key(&self, engine: &Engine, unit: usize, note: u8) -> Option<DuckKey> {
        if !self.duck_notes(unit).contains(&note) {
            return None;
        }
        let ducks = Self::ducks(engine);
        match &self.keys {
            Some(keys) => keys
                .get(&note)
                .filter(|k| ducks.iter().any(|(id, _)| *id == k.duck))
                .cloned(),
            None => match Key::from_note(note) {
                Key::Function(f) => {
                    ducks
                        .into_iter()
                        .nth(usize::from(f))
                        .map(|(duck, _)| DuckKey {
                            duck,
                            action: KeyAction::default(),
                        })
                }
                _ => None,
            },
        }
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

    /// Unit `unit` lost what it showed (a new session, or messages lost
    /// on the way): the next render sends it everything again.
    pub fn resend(&mut self, unit: usize) {
        self.units[unit].shown = Shown::default();
    }

    /// Unit `unit` went away or came back as a new session: what its
    /// faders held is let go, and it is shown everything again.
    pub fn reconnected(&mut self, unit: usize, engine: &mut Engine, now: u64) {
        for t in self.units[unit].touches.iter_mut() {
            if let Some(t) = t.take()
                && t.refused.is_none()
            {
                engine.release(&t.path, now);
            }
        }
        // One at a time, so that two keys on one group release it once.
        while let Some((_, p)) = self.units[unit].presses.pop_first() {
            self.let_go(p, engine, now);
        }
        self.resend(unit);
    }

    /// Messages from unit `unit` went missing: its touched faders that do
    /// not move within [`STALE_TOUCH_MS`] are let go.
    pub fn missed(&mut self, unit: usize, now: u64) {
        for t in self.units[unit].touches.iter_mut().flatten() {
            t.suspect.get_or_insert(now);
        }
        for p in self.units[unit].presses.values_mut() {
            p.suspect.get_or_insert(now);
        }
    }

    /// Unit `unit`'s connection changed from `was` to `is`.
    pub fn link_changed(
        &mut self,
        unit: usize,
        was: Link,
        is: Link,
        engine: &mut Engine,
        now: u64,
    ) {
        if is.session != was.session || !is.connected {
            self.reconnected(unit, engine, now);
            return;
        }
        if is.missed != was.missed {
            self.missed(unit, now);
        }
        if is.resync != was.resync {
            self.resend(unit);
        }
    }

    /// The controls held by a touched fader or a duck key held down.
    pub fn touching(&self) -> HashSet<String> {
        let faders = (self.units.iter())
            .flat_map(|u| u.touches.iter().flatten())
            .filter(|t| t.refused.is_none())
            .map(|t| t.path.clone());
        let keys = (self.units.iter())
            .flat_map(|u| u.presses.values())
            .filter(|p| p.holds())
            .map(|p| hold_path(&p.duck));
        faders.chain(keys).collect()
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
                    suspect: None,
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
                if let Some(t) = &mut self.units[unit].touches[usize::from(strip)] {
                    t.suspect = None;
                    if t.refused.is_none() {
                        engine.set(&t.path, json!(taper::fader_to_db(value)), now);
                    }
                }
            }
            Input::Key { key, down: true } => match self.duck_key(engine, unit, key.note()) {
                Some(k) => self.duck_down(unit, key.note(), k, engine, now),
                None => self.key(unit, key, engine, now),
            },
            Input::Key { key, down: false } => {
                if let Some(p) = self.units[unit].presses.remove(&key.note()) {
                    self.duck_up(p, engine, now);
                }
            }
        }
    }

    /// A duck key went down: a latch toggles the mode, a hold is taken
    /// unless another session has it.
    fn duck_down(&mut self, unit: usize, note: u8, key: DuckKey, engine: &mut Engine, now: u64) {
        // A note on again without a note off between: the same press.
        if self.units[unit].presses.contains_key(&note) {
            return;
        }
        let path = hold_path(&key.duck);
        let mut refused = None;
        if key.action == KeyAction::Latch {
            Self::toggle_mode(&key.duck, engine, now);
        } else {
            refused = engine
                .owner(&path)
                .filter(|o| !engine.mine(o))
                .map(short_name);
            if refused.is_none() {
                engine.touch(&path, now);
                engine.set(&path, json!(true), now);
            }
        }
        let press = Press {
            duck: key.duck,
            action: key.action,
            since: now,
            refused,
            suspect: None,
        };
        self.units[unit].presses.insert(note, press);
    }

    /// A duck key went up: a tap toggles the mode, whoever holds the
    /// hold, as the mode has no owner; then the hold is let go.
    fn duck_up(&mut self, p: Press, engine: &mut Engine, now: u64) {
        if p.action == KeyAction::HoldOrLatch && now < p.since + LATCH_MS {
            Self::toggle_mode(&p.duck, engine, now);
        }
        self.let_go(p, engine, now);
    }

    /// Releases the hold `p` took, unless another key still holds it.
    fn let_go(&self, p: Press, engine: &mut Engine, now: u64) {
        if p.holds() && !self.touching().contains(&hold_path(&p.duck)) {
            engine.release(&hold_path(&p.duck), now);
        }
    }

    /// Sets duck group `duck`'s `mode` to `auto` when it is `on`, and to
    /// `on` when it is `auto`. A bypass (`off`) is left as it is: only a
    /// screen or the API sets and ends it.
    fn toggle_mode(duck: &str, engine: &mut Engine, now: u64) {
        let path = format!("/ducks/{duck}/mode");
        match engine.display(&path).and_then(Value::as_str) {
            Some("on") => engine.set(&path, json!("auto"), now),
            Some("off") => {}
            _ => engine.set(&path, json!("on"), now),
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
            // A function key without a duck group, or any other note.
            Key::Function(_) | Key::Other(_) => {
                tracing::debug!(note = key.note(), "key not mapped")
            }
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
                // A hold that lapsed with the old connection goes back in
                // while its key is down.
                for path in touching.iter().filter(|p| p.starts_with("/ducks/")) {
                    if engine.display(path) != Some(&json!(true)) {
                        engine.set(path, json!(true), now);
                    }
                }
                for warning in self.unbound_keys(engine) {
                    tracing::warn!("{warning}");
                }
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
                for p in self.units.iter_mut().flat_map(|u| u.presses.values_mut()) {
                    if hold_path(&p.duck) == *path && p.holds() {
                        p.refused = Some(short_name(owner));
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
        for u in &mut self.units {
            for (strip, slot) in u.touches.iter_mut().enumerate() {
                if slot
                    .as_ref()
                    .is_some_and(|t| t.suspect.is_some_and(|at| now >= at + STALE_TOUCH_MS))
                {
                    let t = slot.take().expect("checked");
                    tracing::info!(
                        path = t.path,
                        "a touch that may have been let go unseen released"
                    );
                    if t.refused.is_none() {
                        engine.release(&t.path, now);
                    }
                    u.shown.faders[strip] = None;
                    u.shown.moved_at[strip] = None;
                }
            }
        }
        // A key cannot show it is still down: after missed messages its
        // hold ends.
        for unit in 0..self.units.len() {
            let stale: Vec<u8> = (self.units[unit].presses.iter())
                .filter(|(_, p)| p.suspect.is_some_and(|at| now >= at + STALE_TOUCH_MS))
                .map(|(note, _)| *note)
                .collect();
            for note in stale {
                let p = self.units[unit].presses.remove(&note).expect("listed");
                tracing::info!(
                    duck = p.duck,
                    "a duck key that may have been let go unseen released"
                );
                self.let_go(p, engine, now);
            }
        }
        for p in self.units.iter_mut().flat_map(|u| u.presses.values_mut()) {
            let path = hold_path(&p.duck);
            if p.holds()
                && !engine.touch_pending(&path)
                && let Some(owner) = engine.owner(&path).filter(|o| !engine.mine(o))
            {
                p.refused = Some(short_name(owner));
            }
        }
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
            let duck_leds: Vec<(u8, Led)> = (self.duck_notes(i).into_iter())
                .map(|note| {
                    let led = self
                        .duck_key(engine, i, note)
                        .map_or(Led::Off, |k| duck_led(engine, &k.duck));
                    (note, led)
                })
                .collect();
            let notice = self.notice(engine, i);
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
                    // A REC key bound to a duck shows the duck.
                    if duck_leds.iter().any(|(n, _)| *n == key.note()) {
                        continue;
                    }
                    if u.shown.leds.get(&key.note()) != Some(&led) {
                        out.push((i, mcu::led(key, led)));
                        u.shown.leds.insert(key.note(), led);
                    }
                }
                let value = match &notice {
                    Some(n) => mcu::cell(&n.chars().skip(s * mcu::STRIP_CHARS).collect::<String>()),
                    None => v.value.clone(),
                };
                for (line, text) in [(0, &v.name), (1, &value)] {
                    let cell = line * 8 + s;
                    if u.shown.cells[cell].as_ref() != Some(text) {
                        let offset = line * mcu::LINE_CHARS + s * mcu::STRIP_CHARS;
                        out.push((i, mcu::lcd(u.spec.device, offset, text)));
                        u.shown.cells[cell] = Some(text.clone());
                    }
                }
            }
            for (note, led) in duck_leds {
                if u.shown.leds.get(&note) != Some(&led) {
                    out.push((i, mcu::led(Key::from_note(note), led)));
                    u.shown.leds.insert(note, led);
                }
            }
        }
        out
    }

    /// While a duck key of unit `unit` is down, line 2 of its scribble
    /// strip says who holds the group when its hold was refused, or that
    /// the group is bypassed.
    fn notice(&self, engine: &Engine, unit: usize) -> Option<String> {
        let name = |duck: &str| {
            (Self::ducks(engine).into_iter())
                .find(|(id, _)| id == duck)
                .map_or_else(|| duck.to_owned(), |(_, name)| name)
        };
        let presses = self.units[unit].presses.values();
        let refused = presses.clone().find_map(|p| {
            p.refused
                .as_ref()
                .map(|h| format!("{} held by {h}", name(&p.duck)))
        });
        refused.or_else(|| {
            presses
                .clone()
                .find(|p| engine.display(&format!("/ducks/{}/mode", p.duck)) == Some(&json!("off")))
                .map(|p| format!("{} bypassed", name(&p.duck)))
        })
    }

    /// What is wrong with the duck keys of their own on the shape: a key
    /// bound to a group the shape lacks does nothing.
    pub fn unbound_keys(&self, engine: &Engine) -> Vec<String> {
        let ducks = Self::ducks(engine);
        (self.keys.iter().flatten())
            .filter(|(_, k)| !ducks.iter().any(|(id, _)| *id == k.duck))
            .map(|(note, k)| format!("keys.{note}: the shape has no duck group {}", k.duck))
            .collect()
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

/// A duck key's LED: on while the group is held or latched in, blinking
/// while it is bypassed (`mode` `off`), else off.
fn duck_led(engine: &Engine, duck: &str) -> Led {
    let at = |leaf: &str| engine.display(&format!("/ducks/{duck}/{leaf}")).cloned();
    if at("hold") == Some(json!(true)) || at("mode") == Some(json!("on")) {
        Led::On
    } else if at("mode") == Some(json!("off")) {
        Led::Blink
    } else {
        Led::Off
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
    use crate::engine::tests::{engine, live, sent};

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
    fn a_new_session_lets_go_of_held_faders_and_gets_everything_again() {
        let mut e = live(1);
        let mut s = Surface::new(units(2), None);
        let first = s.render(&e, 0);
        // Faders, LEDs and both lines of every strip per unit, and the
        // X-Touch's function key LEDs.
        let on = |out: &[(usize, Vec<u8>)], unit| out.iter().filter(|(u, _)| *u == unit).count();
        assert_eq!(on(&first, 0), 9 + 8 * 4 + 16 + 8);
        assert_eq!(on(&first, 1), 8 + 8 * 4 + 16);
        assert!(s.render(&e, 100).is_empty(), "only what changed");

        s.resend(1);
        let again = s.render(&e, 200);
        assert_eq!(on(&again, 0), 0);
        assert_eq!(on(&again, 1), on(&first, 1));

        s.input(0, &[0x90, 104, 0x7f], &mut e, 300);
        e.take_actions();
        s.reconnected(0, &mut e, 400);
        assert_eq!(
            sent_types(&e.take_actions()),
            vec![("release".into(), "/channels/ch1/fader".into())]
        );
        assert!(s.touching().is_empty());
        assert_eq!(on(&s.render(&e, 500), 0), on(&first, 0));
    }

    #[test]
    fn after_missed_messages_a_touch_that_does_not_move_is_let_go() {
        let mut e = live(1);
        let mut s = Surface::new(units(1), None);
        let held = || HashSet::from(["/channels/ch1/fader".to_owned()]);
        s.render(&e, 0);
        s.input(0, &[0x90, 104, 0x7f], &mut e, 0);
        e.take_actions();
        let link = Link {
            connected: true,
            session: 1,
            ..Link::default()
        };
        let lost = |n: u64| Link {
            resync: n,
            missed: n,
            ..link
        };
        // The fader moves after the loss: still held.
        s.link_changed(0, link, lost(1), &mut e, 1000);
        s.input(0, &[0xe0, 0, 0x40], &mut e, 2000);
        s.tick(&mut e, 5000);
        assert_eq!(s.touching(), held());
        e.take_actions();
        // It does not move after the next one: let go.
        s.link_changed(0, lost(1), lost(2), &mut e, 6000);
        s.tick(&mut e, 7499);
        assert_eq!(s.touching(), held());
        s.tick(&mut e, 7500);
        assert_eq!(
            sent_types(&e.take_actions()),
            vec![("release".into(), "/channels/ch1/fader".into())]
        );
        assert!(s.touching().is_empty());

        // A loss of what went to the surface alone leaves touches alone,
        // and sends it everything again.
        s.input(0, &[0x90, 104, 0x7f], &mut e, 8000);
        s.render(&e, 8000);
        let resend = Link {
            resync: 3,
            ..lost(2)
        };
        s.link_changed(0, lost(2), resend, &mut e, 8000);
        s.tick(&mut e, 20_000);
        assert_eq!(s.touching(), held());
        assert_eq!(s.render(&e, 20_000).len(), 9 + 8 * 4 + 16 + 8 - 1);
        // The session ending lets go of everything.
        let down = Link {
            connected: false,
            ..resend
        };
        s.link_changed(0, resend, down, &mut e, 21_000);
        assert!(s.touching().is_empty());
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

    const F1: u8 = 54;
    const F2: u8 = 55;
    const HOLD: &str = "/ducks/comm/hold";
    const MODE: &str = "/ducks/comm/mode";

    /// A live engine whose shape has the duck groups `comm` ("Commentary
    /// A") and `comm-b`.
    fn ducking() -> Engine {
        let duck = json!({ "mode": "auto", "hold": false });
        let snapshot = json!({ "type": "snapshot", "replica": { "id": "r1", "role": "leader" },
            "you": {}, "epoch": "1", "revision": 5,
            "shape": { "channels": [{ "id": "ch1", "name": "Mic 1" }], "buses": [],
                "ducks": [{ "id": "comm", "name": "Commentary A" }, { "id": "comm-b", "name": "Commentary B" }] },
            "state": { "channels": { "ch1": { "fader": -90, "mute": false, "solo": "off" } },
                "ducks": { "comm": duck, "comm-b": duck }, "consoles": [] },
            "owners": {} });
        let mut e = engine(1);
        e.start(0);
        e.opened(1, 0);
        e.received(1, &snapshot.to_string(), 0);
        e.take_actions();
        e.take_events();
        e
    }

    fn delta(e: &mut Engine, revision: u64, changes: Value, owners: Value) {
        let d = json!({ "type": "delta", "replica": { "id": "r1", "role": "leader" },
            "revision": revision, "changes": changes, "owners": owners });
        e.received(1, &d.to_string(), 0);
    }

    fn key(s: &mut Surface, e: &mut Engine, note: u8, down: bool, now: u64) {
        s.input(0, &[0x90, note, if down { 0x7f } else { 0 }], e, now);
    }

    fn values(actions: &[Action]) -> Vec<(String, String, Value)> {
        sent(actions, 1)
            .into_iter()
            .filter(|m| m["type"] != "ping")
            .map(|m| {
                (
                    m["type"].as_str().unwrap().to_owned(),
                    m["path"].as_str().unwrap_or_default().to_owned(),
                    m["value"].clone(),
                )
            })
            .collect()
    }

    fn cmd(kind: &str, path: &str, value: Value) -> (String, String, Value) {
        (kind.into(), path.into(), value)
    }

    #[test]
    fn a_held_function_key_touches_sets_and_releases_its_ducks_hold() {
        let mut e = ducking();
        let mut s = Surface::new(units(1), None);
        key(&mut s, &mut e, F1, true, 0);
        assert_eq!(
            values(&e.take_actions()),
            vec![
                cmd("touch", HOLD, Value::Null),
                cmd("set", HOLD, json!(true)),
            ]
        );
        assert!(s.touching().contains(HOLD));
        key(&mut s, &mut e, F1, false, LATCH_MS);
        assert_eq!(
            values(&e.take_actions()),
            vec![cmd("release", HOLD, Value::Null)],
            "held past a tap: the mode stays"
        );
        assert!(s.touching().is_empty());
        // F2 is the second group; F3 has none.
        key(&mut s, &mut e, F2, true, 1000);
        key(&mut s, &mut e, F2 + 1, true, 1000);
        assert_eq!(
            values(&e.take_actions()),
            vec![
                cmd("touch", "/ducks/comm-b/hold", Value::Null),
                cmd("set", "/ducks/comm-b/hold", json!(true)),
            ]
        );
    }

    #[test]
    fn a_tapped_function_key_toggles_its_ducks_mode_between_auto_and_on() {
        let mut e = ducking();
        let mut s = Surface::new(units(1), None);
        key(&mut s, &mut e, F1, true, 0);
        key(&mut s, &mut e, F1, false, LATCH_MS - 1);
        assert_eq!(
            values(&e.take_actions()),
            vec![
                cmd("touch", HOLD, Value::Null),
                cmd("set", HOLD, json!(true)),
                cmd("set", MODE, json!("on")),
                cmd("release", HOLD, Value::Null),
            ]
        );
        key(&mut s, &mut e, F1, true, 1000);
        key(&mut s, &mut e, F1, false, 1100);
        let out = values(&e.take_actions());
        assert_eq!(out[2], cmd("set", MODE, json!("auto")));
        // A bypass is left as it is, and said while the key is down.
        let mut e = ducking();
        delta(
            &mut e,
            6,
            json!([{ "path": MODE, "value": "off" }]),
            json!([]),
        );
        s.render(&e, 1999);
        key(&mut s, &mut e, F1, true, 2000);
        let out = s.render(&e, 2000);
        for (strip, text) in [(0, "Comment"), (1, "ary A b"), (2, "ypassed")] {
            let cell = mcu::lcd(Device::Main, 56 + strip * 7, text);
            assert!(out.contains(&(0, cell)), "{strip}: {out:02x?}");
        }
        key(&mut s, &mut e, F1, false, 2001);
        assert_eq!(
            values(&e.take_actions()),
            vec![
                cmd("touch", HOLD, Value::Null),
                cmd("set", HOLD, json!(true)),
                cmd("release", HOLD, Value::Null),
            ]
        );
        let out = s.render(&e, 2002);
        assert!(out.contains(&(0, mcu::lcd(Device::Main, 56 + 14, "       "))));
    }

    #[test]
    fn a_refused_duck_key_shows_the_holder_and_only_a_tap_latches() {
        let ana = json!({ "principal": "ana", "handle": "aaaaaaaaaaaaaaaaaaaaaa", "kind": "ui", "connection": "c" });
        // Held elsewhere before the press: nothing goes out.
        let mut e = ducking();
        delta(
            &mut e,
            6,
            json!([{ "path": HOLD, "value": true }]),
            json!([{ "path": HOLD, "owner": ana }]),
        );
        let mut s = Surface::new(units(1), None);
        s.render(&e, 0);
        key(&mut s, &mut e, F1, true, 10);
        assert!(values(&e.take_actions()).is_empty());
        let out = s.render(&e, 20);
        for (strip, text) in [
            (0, "Comment"),
            (1, "ary A h"),
            (2, "eld by "),
            (3, "ana    "),
        ] {
            let cell = mcu::lcd(Device::Main, 56 + strip * 7, text);
            assert!(out.contains(&(0, cell)), "{strip}: {out:02x?}");
        }
        key(&mut s, &mut e, F1, false, 10 + LATCH_MS);
        assert!(values(&e.take_actions()).is_empty(), "nothing to release");
        let out = s.render(&e, 400);
        let value = mcu::cell(&taper::format_db(-90.0));
        assert!(
            out.contains(&(0, mcu::lcd(Device::Main, 56, &value))),
            "back to the fader: {out:02x?}"
        );
        assert!(out.contains(&(0, mcu::lcd(Device::Main, 56 + 21, "       "))));
        // The mode has no owner: a tap latches while another holds.
        key(&mut s, &mut e, F1, true, 500);
        key(&mut s, &mut e, F1, false, 501);
        assert_eq!(
            values(&e.take_actions()),
            vec![cmd("set", MODE, json!("on"))]
        );

        // Refused by the server after the press: shown, and let go
        // without a release.
        let mut e = ducking();
        let mut s = Surface::new(units(1), None);
        key(&mut s, &mut e, F1, true, 0);
        e.take_actions();
        s.event(
            &Event::Refused {
                path: HOLD.into(),
                owner: ana,
            },
            &mut e,
            5,
        );
        assert!(s.touching().is_empty());
        let out = s.render(&e, 6);
        assert!(out.contains(&(0, mcu::lcd(Device::Main, 56 + 21, "ana    "))));
        key(&mut s, &mut e, F1, false, 10);
        assert_eq!(
            values(&e.take_actions()),
            vec![cmd("set", MODE, json!("on"))]
        );

        // Answered, the hold went to another session: noticed on a tick.
        let mut e = ducking();
        let mut s = Surface::new(units(1), None);
        key(&mut s, &mut e, F1, true, 0);
        let touch = sent(&e.take_actions(), 1).remove(0);
        let me =
            json!({ "principal": "p", "handle": e.handle(), "kind": "console", "connection": "c" });
        let d = json!({ "type": "delta", "replica": { "id": "r1", "role": "leader" }, "revision": 6,
            "owners": [{ "path": HOLD, "owner": { "principal": "bo", "handle": "bbbbbbbbbbbbbbbbbbbbbb", "kind": "ui" } }],
            "origins": [{ "owner": me, "commandId": touch["id"], "seq": touch["seq"] }] });
        e.received(1, &d.to_string(), 5);
        s.tick(&mut e, 6);
        assert!(s.touching().is_empty());
        let out = s.render(&e, 7);
        assert!(
            out.contains(&(0, mcu::lcd(Device::Main, 56 + 21, "bo     "))),
            "{out:02x?}"
        );
    }

    #[test]
    fn function_key_leds_show_hold_latch_and_bypass() {
        let mut e = ducking();
        let mut s = Surface::new(units(2), None);
        let first = s.render(&e, 0);
        assert!(first.contains(&(0, mcu::led(Key::Function(0), Led::Off))));
        assert!(first.contains(&(0, mcu::led(Key::Function(7), Led::Off))));
        assert!(
            !first
                .iter()
                .any(|(u, m)| *u == 1 && (54..=61).contains(&m[1]) && m[0] == 0x90),
            "an Extender has no function keys"
        );
        let led = |s: &mut Surface, e: &Engine, now| {
            s.render(e, now)
                .into_iter()
                .filter(|(u, m)| *u == 0 && m[..2] == [0x90, F1])
                .map(|(_, m)| m[2])
                .collect::<Vec<u8>>()
        };
        delta(
            &mut e,
            6,
            json!([{ "path": HOLD, "value": true }]),
            json!([]),
        );
        assert_eq!(led(&mut s, &e, 1), vec![0x7f], "held");
        delta(
            &mut e,
            7,
            json!([{ "path": HOLD, "value": false }, { "path": MODE, "value": "on" }]),
            json!([]),
        );
        assert_eq!(led(&mut s, &e, 2), Vec::<u8>::new(), "latched: still on");
        delta(
            &mut e,
            8,
            json!([{ "path": MODE, "value": "off" }]),
            json!([]),
        );
        assert_eq!(led(&mut s, &e, 3), vec![1], "bypassed: blinks");
        delta(
            &mut e,
            9,
            json!([{ "path": HOLD, "value": true }]),
            json!([]),
        );
        assert_eq!(led(&mut s, &e, 4), vec![0x7f], "a hold beats the bypass");
        delta(
            &mut e,
            10,
            json!([{ "path": HOLD, "value": false }, { "path": MODE, "value": "auto" }]),
            json!([]),
        );
        assert_eq!(led(&mut s, &e, 5), vec![0], "auto");
    }

    #[test]
    fn duck_keys_of_its_own_replace_the_function_keys() {
        let mut e = ducking();
        let keys = DuckKeys::from([
            (
                0,
                DuckKey {
                    duck: "comm-b".into(),
                    action: KeyAction::Hold,
                },
            ),
            (
                F2,
                DuckKey {
                    duck: "comm".into(),
                    action: KeyAction::Latch,
                },
            ),
            (
                2,
                DuckKey {
                    duck: "gone".into(),
                    action: KeyAction::Hold,
                },
            ),
        ]);
        let mut s = Surface::new(units(1), None).with_keys(Some(keys));
        key(&mut s, &mut e, F1, true, 0);
        key(&mut s, &mut e, F1, false, 1);
        key(&mut s, &mut e, 2, true, 2);
        assert!(
            values(&e.take_actions()).is_empty(),
            "F1 and a missing group do nothing"
        );
        // A hold-only key: a tap holds and lets go.
        key(&mut s, &mut e, 0, true, 10);
        key(&mut s, &mut e, 0, false, 20);
        assert_eq!(
            values(&e.take_actions()),
            vec![
                cmd("touch", "/ducks/comm-b/hold", Value::Null),
                cmd("set", "/ducks/comm-b/hold", json!(true)),
                cmd("release", "/ducks/comm-b/hold", Value::Null),
            ]
        );
        // A latch key toggles on the press and holds nothing.
        key(&mut s, &mut e, F2, true, 30);
        assert!(s.touching().is_empty());
        key(&mut s, &mut e, F2, false, 1000);
        assert_eq!(
            values(&e.take_actions()),
            vec![cmd("set", MODE, json!("on"))]
        );
        // REC 1 shows its duck, not a dark REC LED.
        let out = s.render(&e, 1001);
        assert!(
            out.contains(&(0, mcu::led(Key::Rec(0), Led::On))),
            "{out:02x?}"
        );
        assert!(out.contains(&(0, mcu::led(Key::Function(1), Led::On))));
        assert!(!out.contains(&(0, mcu::led(Key::Rec(0), Led::Off))));
        assert!(
            !out.iter().any(|(_, m)| m[..2] == [0x90, F1]),
            "F1 is not a duck key"
        );
    }

    #[test]
    fn a_held_duck_key_is_let_go_with_its_session_or_after_missed_messages() {
        let mut e = ducking();
        let mut s = Surface::new(units(1), None);
        key(&mut s, &mut e, F1, true, 0);
        let me =
            json!({ "principal": "p", "handle": e.handle(), "kind": "console", "connection": "c" });
        let origins: Vec<Value> = sent(&e.take_actions(), 1)
            .iter()
            .map(|m| json!({ "owner": me, "commandId": m["id"], "seq": m["seq"] }))
            .collect();
        delta(
            &mut e,
            6,
            json!([{ "path": HOLD, "value": true }]),
            json!([{ "path": HOLD, "owner": me }]),
        );
        let answer = json!({ "type": "delta", "replica": { "id": "r1", "role": "leader" },
            "revision": 7, "origins": origins });
        e.received(1, &answer.to_string(), 50);
        assert_eq!(e.display(HOLD), Some(&json!(true)));
        // A new connection whose snapshot lost the hold with the lease:
        // taken and set again.
        let snapshot = json!({ "type": "snapshot", "replica": { "id": "r1", "role": "leader" },
            "you": {}, "epoch": "1", "revision": 9, "shape": e.mirror().shape,
            "state": { "ducks": { "comm": { "mode": "auto", "hold": false } }, "consoles": [] },
            "owners": {} });
        e.received(1, &snapshot.to_string(), 100);
        for ev in e.take_events() {
            s.event(&ev, &mut e, 100);
        }
        assert_eq!(
            values(&e.take_actions()),
            vec![
                cmd("touch", HOLD, Value::Null),
                cmd("set", HOLD, json!(true))
            ]
        );
        // Messages went missing: a key cannot show it is still down.
        let link = Link {
            connected: true,
            session: 1,
            ..Link::default()
        };
        let lost = Link { missed: 1, ..link };
        s.link_changed(0, link, lost, &mut e, 1000);
        s.tick(&mut e, 1000 + STALE_TOUCH_MS - 1);
        assert!(values(&e.take_actions()).iter().all(|c| c.0 != "release"));
        s.tick(&mut e, 1000 + STALE_TOUCH_MS);
        assert_eq!(
            values(&e.take_actions()),
            vec![cmd("release", HOLD, Value::Null)]
        );
        key(&mut s, &mut e, F1, false, 3000);
        assert!(values(&e.take_actions()).is_empty());
        // The session ending lets go at once.
        key(&mut s, &mut e, F1, true, 4000);
        e.take_actions();
        s.link_changed(
            0,
            lost,
            Link {
                connected: false,
                ..lost
            },
            &mut e,
            4100,
        );
        assert_eq!(
            values(&e.take_actions()),
            vec![cmd("release", HOLD, Value::Null)]
        );
        assert!(s.touching().is_empty());
    }

    #[test]
    fn a_duck_held_on_two_x_touches_is_let_go_with_the_last_key() {
        let mut e = ducking();
        let two = vec![
            UnitSpec {
                id: "a".into(),
                name: "A".into(),
                device: Device::Main,
            },
            UnitSpec {
                id: "b".into(),
                name: "B".into(),
                device: Device::Main,
            },
        ];
        let mut s = Surface::new(two, None);
        s.input(0, &[0x90, F1, 0x7f], &mut e, 0);
        s.input(1, &[0x90, F1, 0x7f], &mut e, 10);
        e.take_actions();
        s.input(0, &[0x90, F1, 0], &mut e, 1000);
        assert!(values(&e.take_actions()).is_empty(), "still held on b");
        s.input(1, &[0x90, F1, 0], &mut e, 2000);
        assert_eq!(
            values(&e.take_actions()),
            vec![cmd("release", HOLD, Value::Null)]
        );
    }

    #[test]
    fn an_empty_keys_map_leaves_the_x_touch_without_duck_keys() {
        let mut e = ducking();
        let mut s = Surface::new(units(1), None).with_keys(Some(DuckKeys::new()));
        let out = s.render(&e, 0);
        key(&mut s, &mut e, F1, true, 1);
        key(&mut s, &mut e, F1, false, 2);
        assert!(values(&e.take_actions()).is_empty());
        assert!(
            !out.iter()
                .any(|(_, m)| m[0] == 0x90 && (54..=61).contains(&m[1])),
            "{out:02x?}"
        );
    }

    #[test]
    fn two_keys_on_one_group_release_it_once_when_their_session_ends() {
        let mut e = ducking();
        let keys = DuckKeys::from([
            (
                0,
                DuckKey {
                    duck: "comm".into(),
                    action: KeyAction::Hold,
                },
            ),
            (
                1,
                DuckKey {
                    duck: "comm".into(),
                    action: KeyAction::Hold,
                },
            ),
        ]);
        let mut s = Surface::new(units(1), None).with_keys(Some(keys));
        key(&mut s, &mut e, 0, true, 0);
        key(&mut s, &mut e, 1, true, 0);
        e.take_actions();
        s.reconnected(0, &mut e, 100);
        assert_eq!(
            values(&e.take_actions()),
            vec![cmd("release", HOLD, Value::Null)]
        );
    }

    #[test]
    fn a_latch_key_toggles_once_per_press() {
        let mut e = ducking();
        let keys = DuckKeys::from([(
            F1,
            DuckKey {
                duck: "comm".into(),
                action: KeyAction::Latch,
            },
        )]);
        let mut s = Surface::new(units(1), None).with_keys(Some(keys));
        // A note on repeated without a note off is the same press.
        key(&mut s, &mut e, F1, true, 0);
        key(&mut s, &mut e, F1, true, 5);
        key(&mut s, &mut e, F1, false, 10);
        assert_eq!(
            values(&e.take_actions()),
            vec![cmd("set", MODE, json!("on"))]
        );
        assert!(s.touching().is_empty());
    }

    #[test]
    fn keys_bound_to_groups_the_shape_lacks_are_named() {
        let e = ducking();
        let keys = DuckKeys::from([
            (
                0,
                DuckKey {
                    duck: "comm".into(),
                    action: KeyAction::Hold,
                },
            ),
            (
                1,
                DuckKey {
                    duck: "gone".into(),
                    action: KeyAction::Hold,
                },
            ),
        ]);
        let s = Surface::new(units(1), None).with_keys(Some(keys));
        assert_eq!(
            s.unbound_keys(&e),
            vec!["keys.1: the shape has no duck group gone"]
        );
        assert!(Surface::new(units(1), None).unbound_keys(&e).is_empty());
    }
}
