//! The mapping between MCU surfaces and the faderz state: one or more
//! units side by side (an X-Touch and its Extenders) over the channels,
//! banked together; faders write while touched, motors follow the state
//! while not; mute, solo and select keys; V-Pots on the pan; duck keys;
//! names and values on the scribble strips. Every unit is a console of
//! its own on the server.

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
/// A meter frame older than this, 3 meter periods of 40 ms, no longer
/// lights a duck key or a strip's meter.
pub const FRAME_STALE_MS: u64 = 120;
/// A peak above this lights CLIP: the top 1 dB of the digital scale,
/// close to clipping.
pub const CLIP_DBFS: f64 = -1.0;
/// The pan one V-Pot detent moves: 100 detents from full left to full
/// right.
pub const POT_STEP: f64 = 0.02;
/// In this long every unit is sent all it shows again, a few messages
/// at a time in `REFRESH_STEPS` even steps, so a surface that lost or
/// garbled messages, such as right after it invited the bridge again,
/// shows the state again without a burst it would drop part of.
pub const REFRESH_CYCLE_MS: u64 = 5000;
/// Per strip its fader, its key LEDs, its ring and meter, and each of its
/// two cells; then the master fader with the duck key LEDs and the strip
/// colours; then the timecode display. Each step fits one packet.
const STRIP_PARTS: usize = 5;
pub const REFRESH_STEPS: usize = mcu::STRIPS as usize * STRIP_PARTS + 2;
pub const REFRESH_STEP_MS: u64 = REFRESH_CYCLE_MS / REFRESH_STEPS as u64;
/// After a unit's session starts, or the unit is heard again after a
/// silence, it is sent all it shows again this long after the first full
/// send: an X-Touch that just came back drops part of what comes while it
/// settles, different parts each time.
pub const RESEND_AFTER_MS: [u64; 3] = [300, 1000, 3000];

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
    /// Pressed on a bypassed group: the hold starts once the key has
    /// been down for `LATCH_MS`, so that a tap sends nothing.
    waiting: bool,
}

impl Press {
    /// Whether the press holds its group's `hold`: a hold key's press
    /// the server did not refuse, once its hold has started.
    fn holds(&self) -> bool {
        self.action != KeyAction::Latch && self.refused.is_none() && !self.waiting
    }

    /// Takes the hold unless another session has it.
    fn start(&mut self, engine: &mut Engine, now: u64) {
        self.waiting = false;
        let path = hold_path(&self.duck);
        self.refused = engine
            .owner(&path)
            .filter(|o| !engine.mine(o))
            .map(short_name);
        if self.refused.is_none() {
            engine.touch(&path, now);
            engine.set(&path, json!(true), now);
        }
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
    rings: [Option<u8>; 8],
    meters: [Option<u8>; 8],
    clips: [Option<bool>; 8],
    /// The meter frame the levels were last sent for.
    meter_frame: Option<u64>,
    /// 16 cells: 8 per line.
    cells: [Option<String>; 16],
    colours: Option<[u8; 8]>,
    timecode: [Option<(u8, bool)>; mcu::TIMECODE_DIGITS],
}

impl Shown {
    /// Forgets refresh step `step` of what the unit shows, so the next
    /// render sends that part again; `ducks` are the unit's duck keys.
    fn forget(&mut self, step: usize, ducks: &[u8]) {
        let strips = usize::from(mcu::STRIPS);
        if step < strips * STRIP_PARTS {
            let strip = step / STRIP_PARTS;
            let s = strip as u8;
            match step % STRIP_PARTS {
                0 => self.faders[strip] = None,
                1 => {
                    for key in [Key::Rec(s), Key::Solo(s), Key::Mute(s), Key::Select(s)] {
                        self.leds.remove(&key.note());
                    }
                }
                2 => {
                    self.rings[strip] = None;
                    self.meters[strip] = None;
                    self.clips[strip] = None;
                }
                3 => self.cells[strip] = None,
                _ => self.cells[strips + strip] = None,
            }
            return;
        }
        if step > strips * STRIP_PARTS {
            self.timecode = [None; mcu::TIMECODE_DIGITS];
            return;
        }
        self.faders[usize::from(mcu::MASTER)] = None;
        self.colours = None;
        for note in ducks {
            self.leds.remove(note);
        }
    }
}

struct Unit {
    spec: UnitSpec,
    /// Index of its first strip among all strips, left to right.
    offset: usize,
    touches: [Option<Touch>; 9],
    /// Duck keys held down, by note.
    presses: BTreeMap<u8, Press>,
    /// Whether ALT is down: a V-Pot pushed with it centres the pan.
    alt: bool,
    /// When the unit is sent all it shows again while it settles.
    resends: Vec<u64>,
    shown: Shown,
}

pub struct Surface {
    units: Vec<Unit>,
    /// Index of the channel on the leftmost strip.
    bank: usize,
    /// The master fader's control; else the program bus's fader.
    master: Option<String>,
    selected: Option<String>,
    /// What each screen of this bridge's principal has selected, by
    /// handle, as the last `selections` said.
    screens: BTreeMap<String, String>,
    /// Duck keys of their own; else F1 to F8.
    keys: Option<DuckKeys>,
    /// The sign-in's two lines, shown on the first unit's scribble strip
    /// instead of the strips while a sign-in waits for the user.
    sign_in: Option<[String; 2]>,
    /// The next refresh step, and when.
    refresh_step: usize,
    refresh_at: u64,
    /// Minutes east of UTC of the time of day on the timecode display,
    /// which stays dark without it.
    clock_offset: Option<i32>,
    /// The wall clock, Unix seconds, once the driver has set it.
    time: Option<u64>,
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
                alt: false,
                resends: Vec::new(),
                shown: Shown::default(),
            })
            .collect();
        Surface {
            units,
            bank: 0,
            master,
            selected: None,
            screens: BTreeMap::new(),
            keys: None,
            sign_in: None,
            refresh_step: 0,
            refresh_at: 0,
            clock_offset: None,
            time: None,
        }
    }

    /// Shows the time of day `offset` minutes east of UTC on the X-Touch's
    /// timecode display; `None` leaves it dark.
    pub fn with_clock(mut self, offset: Option<i32>) -> Surface {
        self.clock_offset = offset;
        self
    }

    /// The wall clock, Unix seconds, for the timecode display.
    pub fn set_time(&mut self, unix: u64) {
        self.time = Some(unix);
    }

    /// Shows `lines` on the first unit's scribble strip in place of the
    /// strips, until `None`.
    pub fn set_sign_in(&mut self, lines: Option<[String; 2]>) {
        self.sign_in = lines;
    }

    /// Without the rolling refresh: a fixture checks that nothing more is
    /// sent once the state is shown.
    #[cfg(test)]
    pub(crate) fn without_refresh(mut self) -> Surface {
        self.refresh_at = u64::MAX;
        self
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

    /// The pan of the channel on strip `strip` of unit `unit`.
    fn pan_path(&self, engine: &Engine, unit: usize, strip: u8) -> Option<String> {
        self.channel(engine, unit, strip)
            .map(|c| format!("/channels/{c}/pan/lr"))
    }

    /// Sets the pan of strip `strip` to what `to` makes of it, rounded to
    /// 0.01 within -1 to 1, unless another session holds it.
    fn pan(
        &self,
        unit: usize,
        strip: u8,
        to: impl FnOnce(f64) -> f64,
        engine: &mut Engine,
        now: u64,
    ) {
        let Some(path) = self.pan_path(engine, unit, strip) else {
            return;
        };
        if engine.owner(&path).is_some_and(|o| !engine.mine(o)) {
            return;
        }
        let pan = engine.display(&path).and_then(Value::as_f64).unwrap_or(0.0);
        let next = (to(pan).clamp(-1.0, 1.0) * 100.0).round() / 100.0;
        if next != pan {
            engine.set(&path, json!(next), now);
        }
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
        self.units[unit].alt = false;
        self.units[unit].resends = RESEND_AFTER_MS.iter().map(|d| now + d).collect();
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
        // The surface lost what it was sent, or was cut off and is heard
        // again: everything again now and while it settles.
        if is.resync != was.resync {
            self.units[unit].resends = RESEND_AFTER_MS.iter().map(|d| now + d).collect();
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
            Input::Pot { strip, ticks } => self.pan(
                unit,
                strip,
                |p| p + f64::from(ticks) * POT_STEP,
                engine,
                now,
            ),
            Input::Key { key, down: false } => {
                if key == Key::Alt {
                    self.units[unit].alt = false;
                }
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
        if key.action == KeyAction::Latch {
            Self::toggle_mode(&key.duck, engine, now);
        }
        // A bypassed group is not ducked by a tap: the hold of a key
        // that may still turn out to be one waits for the tap time.
        let bypassed = engine.display(&format!("/ducks/{}/mode", key.duck)) == Some(&json!("off"));
        let mut press = Press {
            duck: key.duck,
            action: key.action,
            since: now,
            refused: None,
            suspect: None,
            waiting: key.action == KeyAction::HoldOrLatch && bypassed,
        };
        if key.action != KeyAction::Latch && !press.waiting {
            press.start(engine, now);
        }
        self.units[unit].presses.insert(note, press);
    }

    /// A duck key went up: a tap toggles the mode, whoever holds the
    /// hold, as the mode has no owner; then the hold is let go.
    fn duck_up(&mut self, mut p: Press, engine: &mut Engine, now: u64) {
        if p.waiting {
            if now < p.since + LATCH_MS {
                // A tap on a bypassed group: nothing was sent.
                return;
            }
            p.start(engine, now);
        }
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
            // Selects the strip's channel for the screens to follow; again on
            // the channel it published, it lets that go.
            Key::Select(s) => {
                if let Some(c) = strip_channel(s) {
                    self.selected = (engine.selection() != Some(c.as_str())).then_some(c);
                    engine.select(self.selected.as_deref());
                }
            }
            // Only with ALT, as Alt+0 on a screen: a push alone, easily
            // made while turning, would move a pan on air.
            Key::Pot(s) if self.units[unit].alt => self.pan(unit, s, |_| 0.0, engine, now),
            Key::Pot(_) => {}
            Key::Alt => self.units[unit].alt = true,
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
            Event::Selections(list) => self.follow(list, engine),
            Event::Error(e) => tracing::warn!("{e}"),
        }
    }

    /// Shows on the SELECT LEDs what a screen signed in as this bridge's
    /// principal selected last, dark once that screen's selection is gone,
    /// its panel closed or the screen left.
    fn follow(&mut self, list: &Value, engine: &mut Engine) {
        let Some(me) = engine.principal() else { return };
        let screens: BTreeMap<String, String> = (list.as_array().into_iter().flatten())
            .filter(|s| s["session"]["kind"] == "ui" && s["session"]["principal"] == me)
            .filter_map(|s| {
                Some((
                    s["session"]["handle"].as_str()?.to_owned(),
                    s["element"].as_str()?.to_owned(),
                ))
            })
            .collect();
        // A screen that changed to what the LEDs show followed a SELECT
        // press; one that moved elsewhere in the same message is taken.
        let changed = (screens.keys().chain(self.screens.keys())).find_map(|h| {
            let now = screens.get(h).cloned();
            (now != self.screens.get(h).cloned() && now != self.selected).then_some(now)
        });
        self.screens = screens;
        if let Some(element) = changed {
            self.selected = element;
            // The LEDs follow; the surface publishes a selection of its own
            // only on a SELECT press, so no screen drives another through
            // it, and one it published before no longer stands.
            if engine.selection().is_some() {
                engine.select(None);
            }
        }
    }

    /// Asks for refused controls held for a second, and notices a touched
    /// control that went to another session.
    pub fn tick(&mut self, engine: &mut Engine, now: u64) {
        for u in &mut self.units {
            if u.resends.iter().any(|&at| now >= at) {
                u.resends.retain(|&at| now < at);
                u.shown = Shown::default();
            }
        }
        if now >= self.refresh_at {
            for i in 0..self.units.len() {
                let ducks = self.duck_notes(i);
                self.units[i].shown.forget(self.refresh_step, &ducks);
            }
            self.refresh_step = (self.refresh_step + 1) % REFRESH_STEPS;
            // Steps keep their pace when a tick is late, and a long pause
            // starts it anew.
            self.refresh_at += REFRESH_STEP_MS;
            if self.refresh_at <= now {
                self.refresh_at = now + REFRESH_STEP_MS;
            }
        }
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
            if p.waiting && now >= p.since + LATCH_MS {
                p.start(engine, now);
            }
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
                .map(|s| self.strip_view(engine, i, s, now))
                .collect();
            let duck_leds: Vec<(u8, Led)> = (self.duck_notes(i).into_iter())
                .map(|note| {
                    let led = self
                        .duck_key(engine, i, note)
                        .map_or(Led::Off, |k| duck_led(engine, &k.duck, now));
                    (note, led)
                })
                .collect();
            let notice = self.notice(engine, i);
            let sign_in = self.sign_in.as_ref().filter(|_| i == 0);
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
                // The overload goes out on crossing CLIP_DBFS either way,
                // before the level; a level goes out on every new meter
                // frame, as an X-Touch lets it fall, silence once.
                if u.shown.clips[s] != Some(v.clip) {
                    out.push((i, mcu::meter_clip(s8, v.clip)));
                    u.shown.clips[s] = Some(v.clip);
                }
                let frame = engine.peaks().frames;
                if u.shown.meters[s] != Some(v.meter)
                    || (u.shown.meter_frame != Some(frame) && v.meter > 0)
                {
                    out.push((i, mcu::meter(s8, v.meter)));
                    u.shown.meters[s] = Some(v.meter);
                }
                if u.shown.rings[s] != Some(v.ring) {
                    out.push((i, mcu::ring(s8, v.ring)));
                    u.shown.rings[s] = Some(v.ring);
                }
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
                // The cell of strip `s` in a line of text over the strips.
                let cut = |line: &str| {
                    mcu::cell(&line.chars().skip(s * mcu::STRIP_CHARS).collect::<String>())
                };
                let value = match &notice {
                    Some(n) => cut(n),
                    None => v.value.clone(),
                };
                let (name, value) = match sign_in {
                    Some([top, bottom]) => (cut(top), cut(bottom)),
                    None => (v.name.clone(), value),
                };
                for (line, text) in [(0, &name), (1, &value)] {
                    let cell = line * 8 + s;
                    if u.shown.cells[cell].as_ref() != Some(text) {
                        let offset = line * mcu::LINE_CHARS + s * mcu::STRIP_CHARS;
                        out.push((i, mcu::lcd(u.spec.device, offset, text)));
                        u.shown.cells[cell] = Some(text.clone());
                    }
                }
            }
            u.shown.meter_frame = Some(engine.peaks().frames);
            // A sign-in shows on white, never on the last session's colours.
            let colours = match sign_in {
                Some(_) => [mcu::WHITE; 8],
                None => std::array::from_fn(|s| view[s].colour),
            };
            if u.shown.colours != Some(colours) {
                out.push((i, mcu::colours(u.spec.device, colours)));
                u.shown.colours = Some(colours);
            }
            if u.spec.device == Device::Main
                && let Some(unix) = self.time
                && let Some(offset) = self.clock_offset
            {
                let digits = mcu::clock_digits(unix, offset);
                for (p, &(ch, dot)) in digits.iter().enumerate() {
                    if u.shown.timecode[p] != Some((ch, dot)) {
                        out.push((i, mcu::timecode_digit(p as u8, ch, dot)));
                        u.shown.timecode[p] = Some((ch, dot));
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

    fn strip_view(&self, engine: &Engine, unit: usize, strip: u8, now: u64) -> StripView {
        let blank = StripView {
            fader: 0,
            mute: Led::Off,
            solo: Led::Off,
            select: Led::Off,
            colour: mcu::WHITE,
            ring: 0,
            meter: 0,
            clip: false,
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
        // The peak of a frame no older than 3 meter periods: its height on
        // the console's meter scale, CLIP close to clipping.
        let peaks = engine.peaks();
        let index = self.bank + self.units[unit].offset + usize::from(strip);
        let peak = (now < peaks.at + FRAME_STALE_MS)
            .then(|| peaks.dbfs.get(index).copied())
            .flatten();
        let clip = peak.is_some_and(|p| p > CLIP_DBFS);
        StripView {
            meter: peak.map_or(0, |p| mcu::meter_level(meter_position(p), clip)),
            clip,
            fader: taper::db_to_fader(db),
            mute: on(at("mute") == Some(json!(true))),
            solo: on(at("solo").is_some_and(|s| s != "off")),
            select: if touch.is_some_and(|t| t.refused.is_some()) {
                Led::Blink
            } else {
                on(self.selected.as_deref() == Some(channel.as_str()))
            },
            ring: mcu::ring_value(Some(at("pan/lr").and_then(|p| p.as_f64()).unwrap_or(0.0))),
            colour: mcu::colour(
                at("color")
                    .as_ref()
                    .and_then(Value::as_str)
                    .unwrap_or("none"),
            ),
            name: mcu::cell(&Self::channel_name(engine, &channel)),
            value: mcu::cell(&holder.unwrap_or_else(|| taper::format_db(db))),
        }
    }
}

/// The console's meter scale (web/src/lib/meters/ballistics.ts
/// `METER_SCALE`) as dBFS and height pairs: finest between -20 dBFS and
/// full scale.
const METER_SCALE: [(f64, f64); 8] = [
    (-60.0, 0.0),
    (-50.0, 0.07),
    (-40.0, 0.16),
    (-30.0, 0.3),
    (-20.0, 0.48),
    (-10.0, 0.7),
    (-6.0, 0.8),
    (0.0, 1.0),
];

/// The height 0 to 1 of `db` on the console's meter scale.
fn meter_position(db: f64) -> f64 {
    let pairs = METER_SCALE.windows(2);
    if db <= METER_SCALE[0].0 {
        return 0.0;
    }
    for w in pairs {
        let ((a_db, a), (b_db, b)) = (w[0], w[1]);
        if db <= b_db {
            return a + (db - a_db) / (b_db - a_db) * (b - a);
        }
    }
    1.0
}

/// A duck key's LED: on while the group is held or latched in, blinking
/// while it is bypassed (`mode` `off`) and not held, on while it ducks its
/// targets by a meter frame younger than `FRAME_STALE_MS`, else off.
fn duck_led(engine: &Engine, duck: &str, now: u64) -> Led {
    let at = |leaf: &str| engine.display(&format!("/ducks/{duck}/{leaf}")).cloned();
    let ducking = engine
        .duck(duck)
        .is_some_and(|d| d.envelope > 0.0 && now < d.at + FRAME_STALE_MS);
    if at("hold") == Some(json!(true)) || at("mode") == Some(json!("on")) {
        Led::On
    } else if at("mode") == Some(json!("off")) {
        Led::Blink
    } else if ducking {
        Led::On
    } else {
        Led::Off
    }
}

struct StripView {
    /// The scribble strip's [`mcu::colour`]; white past the last channel.
    colour: u8,
    fader: u16,
    mute: Led,
    solo: Led,
    select: Led,
    /// The V-Pot ring's [`mcu::ring_value`].
    ring: u8,
    /// The meter's [`mcu::meter_level`], and whether the peak is above
    /// [`CLIP_DBFS`].
    meter: u8,
    clip: bool,
    name: String,
    value: String,
}

#[cfg(test)]
mod tests;
