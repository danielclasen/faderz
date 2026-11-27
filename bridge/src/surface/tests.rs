use super::*;
use crate::engine::Action;
use crate::engine::tests::{engine, live, sent};
use crate::meters::Layout;

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
    // Faders, LEDs, rings, meters with their overload and both lines of
    // every strip, the colours per unit, and the X-Touch's function key
    // LEDs.
    let on = |out: &[(usize, Vec<u8>)], unit| out.iter().filter(|(u, _)| *u == unit).count();
    assert_eq!(on(&first, 0), 9 + 8 * 7 + 16 + 1 + 8);
    assert_eq!(on(&first, 1), 8 + 8 * 7 + 16 + 1);
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
    assert_eq!(s.render(&e, 20_000).len(), 9 + 8 * 7 + 16 + 1 + 8 - 1);
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
        "shape": { "channels": [{ "id": "ch1", "name": "Mic 1", "width": 1 }], "buses": [],
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
    assert_eq!(values(&e.take_actions()), vec![]);
    let out = s.render(&e, 2002);
    assert!(out.contains(&(0, mcu::lcd(Device::Main, 56 + 14, "       "))));
}

/// A ducking engine whose group `comm` is bypassed.
fn bypassed() -> Engine {
    let mut e = ducking();
    delta(
        &mut e,
        6,
        json!([{ "path": MODE, "value": "off" }]),
        json!([]),
    );
    e
}

#[test]
fn a_tap_on_a_bypassed_group_sends_nothing_and_says_so() {
    let mut e = bypassed();
    let mut s = Surface::new(units(1), None);
    key(&mut s, &mut e, F1, true, 0);
    assert_eq!(values(&e.take_actions()), vec![]);
    assert!(s.touching().is_empty());
    let out = s.render(&e, 1);
    assert!(
        out.contains(&(0, mcu::lcd(Device::Main, 56 + 14, "ypassed"))),
        "{out:02x?}"
    );
    s.tick(&mut e, LATCH_MS - 1);
    key(&mut s, &mut e, F1, false, LATCH_MS - 1);
    assert_eq!(values(&e.take_actions()), vec![], "no touch, hold or mode");
    s.tick(&mut e, 2 * LATCH_MS);
    assert_eq!(values(&e.take_actions()), vec![], "nothing starts later");
}

#[test]
fn a_long_press_on_a_bypassed_group_holds_from_the_tap_time() {
    let mut e = bypassed();
    let mut s = Surface::new(units(1), None);
    key(&mut s, &mut e, F1, true, 0);
    s.tick(&mut e, LATCH_MS - 1);
    assert_eq!(values(&e.take_actions()), vec![]);
    s.tick(&mut e, LATCH_MS);
    assert_eq!(
        values(&e.take_actions()),
        vec![
            cmd("touch", HOLD, Value::Null),
            cmd("set", HOLD, json!(true)),
        ]
    );
    assert!(s.touching().contains(HOLD));
    s.tick(&mut e, LATCH_MS + 1);
    assert_eq!(values(&e.take_actions()), vec![], "started once");
    key(&mut s, &mut e, F1, false, 1000);
    assert_eq!(
        values(&e.take_actions()),
        vec![cmd("release", HOLD, Value::Null)]
    );
}

#[test]
fn a_bypassed_key_let_go_at_the_tap_time_holds_and_releases() {
    // Let go at 299 ms is a tap, at 300 ms a press, as for any group.
    let mut e = bypassed();
    let mut s = Surface::new(units(1), None);
    key(&mut s, &mut e, F1, true, 0);
    key(&mut s, &mut e, F1, false, LATCH_MS);
    assert_eq!(
        values(&e.take_actions()),
        vec![
            cmd("touch", HOLD, Value::Null),
            cmd("set", HOLD, json!(true)),
            cmd("release", HOLD, Value::Null),
        ]
    );
}

#[test]
fn a_bypass_set_while_a_key_is_down_does_not_drop_its_hold() {
    let mut e = ducking();
    let mut s = Surface::new(units(1), None);
    key(&mut s, &mut e, F1, true, 0);
    e.take_actions();
    delta(
        &mut e,
        6,
        json!([{ "path": MODE, "value": "off" }]),
        json!([]),
    );
    s.tick(&mut e, 1000);
    assert!(s.touching().contains(HOLD));
    key(&mut s, &mut e, F1, false, 1100);
    assert_eq!(
        values(&e.take_actions()),
        vec![cmd("release", HOLD, Value::Null)]
    );
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
fn function_key_leds_light_while_the_group_ducks_until_the_frames_go_stale() {
    let mut e = ducking();
    let layout = Layout::of(&e.mirror().shape).expect("a layout");
    let frame = |envelope: i16| layout.frame(&[[-2000, envelope, 1], [i16::MIN, 0, 0]]);
    let mut s = Surface::new(units(1), None);
    let led = |s: &mut Surface, e: &Engine, now| {
        s.render(e, now)
            .into_iter()
            .filter(|(u, m)| *u == 0 && m[..2] == [0x90, F1])
            .map(|(_, m)| m[2])
            .collect::<Vec<u8>>()
    };
    assert_eq!(led(&mut s, &e, 0), vec![0], "auto, no frames");
    e.binary(1, &frame(5000), 10);
    assert_eq!(led(&mut s, &e, 10), vec![0x7f], "ducking");
    e.binary(1, &frame(2500), 50);
    assert_eq!(led(&mut s, &e, 50), Vec::<u8>::new(), "sent once");
    e.binary(1, &frame(0), 90);
    assert_eq!(led(&mut s, &e, 90), vec![0], "engaged, not yet ducking");
    e.binary(1, &frame(10_000), 130);
    assert_eq!(led(&mut s, &e, 130), vec![0x7f]);
    assert_eq!(led(&mut s, &e, 130 + FRAME_STALE_MS - 1), Vec::<u8>::new());
    assert_eq!(
        led(&mut s, &e, 130 + FRAME_STALE_MS),
        vec![0],
        "stale: the state alone"
    );
    delta(
        &mut e,
        6,
        json!([{ "path": MODE, "value": "off" }]),
        json!([]),
    );
    e.binary(1, &frame(5000), 300);
    assert_eq!(led(&mut s, &e, 300), vec![1], "bypassed: blinks");
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

const PAN: &str = "/channels/ch1/pan/lr";

fn pot(s: &mut Surface, e: &mut Engine, value: u8, now: u64) {
    s.input(0, &[0xb0, 16, value], e, now);
}

#[test]
fn a_v_pot_turns_its_channels_pan_by_detents_within_full_left_and_right() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    pot(&mut s, &mut e, 2, 0);
    pot(&mut s, &mut e, 0x41, 100);
    pot(&mut s, &mut e, 0x7f, 200);
    pot(&mut s, &mut e, 0x41, 300);
    assert_eq!(
        values(&e.take_actions()),
        vec![
            cmd("set", PAN, json!(0.04)),
            cmd("set", PAN, json!(0.02)),
            cmd("set", PAN, json!(-1.0)),
        ],
        "clockwise is right, and a turn past full left writes nothing"
    );
    // Turns on a strip past the last channel do nothing.
    s.input(0, &[0xb0, 17, 1], &mut e, 400);
    assert!(values(&e.take_actions()).is_empty());
}

const ALT: u8 = 73;

#[test]
fn a_v_pot_pushed_with_alt_centres_the_pan_and_alone_does_nothing() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    pot(&mut s, &mut e, 5, 0);
    e.take_actions();
    key(&mut s, &mut e, 32, true, 100);
    key(&mut s, &mut e, 32, false, 150);
    key(&mut s, &mut e, 32, true, 200);
    assert!(values(&e.take_actions()).is_empty(), "pushes alone");
    key(&mut s, &mut e, ALT, true, 300);
    key(&mut s, &mut e, 32, true, 350);
    assert_eq!(values(&e.take_actions()), vec![cmd("set", PAN, json!(0.0))]);
    // ALT let go, a push is a push again.
    pot(&mut s, &mut e, 5, 400);
    key(&mut s, &mut e, ALT, false, 450);
    e.take_actions();
    key(&mut s, &mut e, 32, true, 500);
    assert!(values(&e.take_actions()).is_empty());
}

#[test]
fn a_v_pot_leaves_a_pan_another_session_holds() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    let other = json!({ "principal": "ana", "handle": "aaaaaaaaaaaaaaaaaaaaaa", "kind": "ui", "connection": "c" });
    delta(
        &mut e,
        6,
        json!([]),
        json!([{ "path": PAN, "owner": other }]),
    );
    pot(&mut s, &mut e, 1, 0);
    key(&mut s, &mut e, ALT, true, 1);
    key(&mut s, &mut e, 32, true, 2);
    assert!(values(&e.take_actions()).is_empty());
}

#[test]
fn the_ring_shows_the_pan_at_once_and_is_dark_past_the_last_channel() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    let out = s.render(&e, 0);
    assert!(out.contains(&(0, mcu::ring(0, 0x16))));
    assert!(out.contains(&(0, mcu::ring(1, 0))));
    // The value written is shown before the server answers, so a turn
    // never sees the ring jump back.
    pot(&mut s, &mut e, 0x45, 1);
    let rings: Vec<_> = (s.render(&e, 2).into_iter())
        .filter(|(_, m)| m[0] == 0xb0)
        .collect();
    assert_eq!(rings, vec![(0, mcu::ring(0, 0x15))]);
    // A move within one LED's step sends nothing.
    pot(&mut s, &mut e, 0x41, 100);
    assert!(!s.render(&e, 101).iter().any(|(_, m)| m[0] == 0xb0));
}

#[test]
fn detents_within_one_write_interval_add_up() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    pot(&mut s, &mut e, 1, 0);
    pot(&mut s, &mut e, 1, 5);
    pot(&mut s, &mut e, 1, 10);
    assert_eq!(
        values(&e.take_actions()),
        vec![cmd("set", PAN, json!(0.02))]
    );
    e.tick(20);
    assert_eq!(
        values(&e.take_actions()),
        vec![cmd("set", PAN, json!(0.06))]
    );
}

#[test]
fn alt_is_let_go_with_its_session() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    pot(&mut s, &mut e, 5, 0);
    key(&mut s, &mut e, ALT, true, 100);
    // Its release may be among what went missing.
    s.reconnected(0, &mut e, 150);
    e.take_actions();
    key(&mut s, &mut e, 32, true, 200);
    assert!(values(&e.take_actions()).is_empty());
}

/// A meter frame in which channel ch1 peaks at `dbfs`.
fn peak(e: &mut Engine, dbfs: f64, now: u64) {
    let layout = Layout::of(&e.mirror().shape).expect("a layout");
    let frame = layout.frame_with(&[(dbfs * 100.0) as i16], &[]);
    e.binary(1, &frame, now);
}

/// The meter messages of strip `strip` in `out`, in order.
fn meter_of(out: &[(usize, Vec<u8>)], strip: u8) -> Vec<u8> {
    (out.iter())
        .filter(|(_, m)| m[0] == 0xd0 && m[1] >> 4 == strip)
        .map(|(_, m)| m[1] & 0x0f)
        .collect()
}

#[test]
fn a_strip_meter_shows_the_peak_on_7_leds_again_on_every_frame() {
    let mut e = ducking();
    let mut s = Surface::new(units(1), None);
    // -20 dBFS is 0.48 of the console's scale: 3 of 7 LEDs, level 5.
    peak(&mut e, -20.0, 0);
    let out = s.render(&e, 0);
    assert_eq!(
        meter_of(&out, 0),
        vec![15, 5],
        "the overload off, then the level"
    );
    // Strips past the last channel: off once.
    assert_eq!(meter_of(&out, 1), vec![15, 0]);
    assert!(meter_of(&s.render(&e, 10), 0).is_empty(), "no new frame");
    peak(&mut e, -20.0, 40);
    assert_eq!(
        meter_of(&s.render(&e, 40), 0),
        vec![5],
        "the X-Touch lets it fall"
    );
    assert!(
        meter_of(&s.render(&e, 40), 1).is_empty(),
        "silence goes out once"
    );
    // A frame older than 3 meter periods: silence, once.
    assert_eq!(meter_of(&s.render(&e, 160), 0), vec![0]);
    assert!(meter_of(&s.render(&e, 200), 0).is_empty());
}

#[test]
fn the_overload_lights_in_the_top_db_only_and_goes_before_the_level() {
    let mut e = ducking();
    let mut s = Surface::new(units(1), None);
    peak(&mut e, -20.0, 0);
    s.render(&e, 0);
    // Loud, as high as the console's red zone, and not clipping: no CLIP.
    peak(&mut e, -1.0, 40);
    assert_eq!(meter_of(&s.render(&e, 40), 0), vec![11]);
    peak(&mut e, -0.5, 80);
    assert_eq!(meter_of(&s.render(&e, 80), 0), vec![14, 14]);
    peak(&mut e, -0.5, 120);
    assert_eq!(
        meter_of(&s.render(&e, 120), 0),
        vec![14],
        "the overload goes out once"
    );
    peak(&mut e, -20.0, 160);
    assert_eq!(meter_of(&s.render(&e, 160), 0), vec![15, 5]);
}

#[test]
fn the_meter_scale_is_the_consoles() {
    // web/src/lib/meters/ballistics.ts METER_SCALE, which the console
    // draws its meters on: a change there must reach the surface too.
    let ts = include_str!("../../../web/src/lib/meters/ballistics.ts");
    let body = &ts[ts.find("METER_SCALE").expect("METER_SCALE")..];
    let body = &body[..body.find("];").expect("its end")];
    let pairs: Vec<(f64, f64)> = (body.lines().map(str::trim))
        .filter_map(|l| l.strip_prefix('[')?.strip_suffix("],"))
        .map(|p| {
            let (db, at) = p.split_once(',').expect("a pair");
            (db.trim().parse().unwrap(), at.trim().parse().unwrap())
        })
        .collect();
    assert_eq!(pairs, METER_SCALE.to_vec());
}

/// A live engine on `n` mono channels ch1 to chN.
fn channels(n: usize) -> Engine {
    let ids: Vec<Value> = (1..=n)
        .map(|i| json!({ "id": format!("ch{i}"), "name": format!("Ch {i}"), "width": 1 }))
        .collect();
    let state: serde_json::Map<String, Value> = (1..=n)
        .map(|i| {
            (
                format!("ch{i}"),
                json!({ "fader": -90, "mute": false, "solo": "off" }),
            )
        })
        .collect();
    let snapshot = json!({ "type": "snapshot", "replica": { "id": "r1", "role": "leader" },
        "you": {}, "epoch": "1", "revision": 5,
        "shape": { "channels": ids, "buses": [] },
        "state": { "channels": state, "consoles": [] }, "owners": {} });
    let mut e = engine(1);
    e.start(0);
    e.opened(1, 0);
    e.received(1, &snapshot.to_string(), 0);
    e.take_actions();
    e.take_events();
    e
}

/// A meter frame with these channel peaks, dBFS, the first channels first.
fn peaks(e: &mut Engine, dbfs: &[f64], now: u64) {
    let layout = Layout::of(&e.mirror().shape).expect("a layout");
    let cents: Vec<i16> = dbfs.iter().map(|d| (d * 100.0) as i16).collect();
    e.binary(1, &layout.frame_with(&cents, &[]), now);
}

#[test]
fn strip_meters_follow_the_bank_and_an_extenders_offset() {
    let mut e = channels(10);
    let mut s = Surface::new(units(2), None);
    // ch1 -45, ch2 -20, ch9 -12, ch10 -7 dBFS.
    let frame = [
        -45.0, -20.0, -60.0, -60.0, -60.0, -60.0, -60.0, -60.0, -12.0, -7.0,
    ];
    peaks(&mut e, &frame, 0);
    let out = s.render(&e, 0);
    let on = |out: &[(usize, Vec<u8>)], unit: usize, strip: u8| -> Vec<u8> {
        meter_of(
            &out.iter()
                .filter(|(u, _)| *u == unit)
                .cloned()
                .collect::<Vec<_>>(),
            strip,
        )
    };
    assert_eq!(on(&out, 0, 0), vec![15, 0], "-45 dBFS lights none");
    assert_eq!(on(&out, 0, 1), vec![15, 5]);
    assert_eq!(
        on(&out, 1, 0),
        vec![15, 7],
        "the Extender's first strip is ch9"
    );
    assert_eq!(on(&out, 1, 1), vec![15, 9]);
    // CHANNEL right: every strip shows the next channel.
    key(&mut s, &mut e, 49, true, 10);
    peaks(&mut e, &frame, 40);
    let out = s.render(&e, 40);
    assert_eq!(on(&out, 0, 0), vec![5]);
    assert_eq!(on(&out, 1, 0), vec![9]);
    assert_eq!(on(&out, 1, 1), vec![0], "past the last channel");
}

#[test]
fn a_stale_frame_while_clipping_turns_the_overload_off_and_the_meter_dark() {
    let mut e = ducking();
    let mut s = Surface::new(units(1), None);
    peak(&mut e, -0.5, 0);
    assert_eq!(meter_of(&s.render(&e, 0), 0), vec![14, 14]);
    assert_eq!(meter_of(&s.render(&e, FRAME_STALE_MS), 0), vec![15, 0]);
}

/// A live engine answered as principal `ben`.
fn signed_in() -> Engine {
    let mut snapshot: Value =
        serde_json::from_str(&crate::engine::tests::snapshot("r1", 5)).unwrap();
    snapshot["you"] = json!({ "principal": "ben", "kind": "console", "connection": "c1" });
    let mut e = engine(1);
    e.start(0);
    e.opened(1, 0);
    e.received(1, &snapshot.to_string(), 0);
    e.take_actions();
    e.take_events();
    e
}

fn screen(principal: &str, handle: &str, kind: &str) -> Value {
    json!({ "principal": principal, "handle": handle, "kind": kind, "connection": "c9" })
}

/// The primary reports `selections`; the surface sees the event.
fn selections(s: &mut Surface, e: &mut Engine, list: Value) {
    let m = json!({ "type": "selections", "replica": { "id": "r1", "role": "leader" }, "selections": list });
    e.received(1, &m.to_string(), 0);
    for ev in e.take_events() {
        s.event(&ev, e, 0);
    }
}

fn selects(actions: &[Action]) -> Vec<Value> {
    sent(actions, 1)
        .into_iter()
        .filter(|m| m["type"] == "select")
        .map(|m| m["element"].clone())
        .collect()
}

fn select_led(s: &mut Surface, e: &Engine) -> Option<Led> {
    (s.render(e, 0).into_iter())
        .find(|(_, m)| m[0] == 0x90 && m[1] == Key::Select(0).note())
        .map(|(_, m)| if m[2] == 0 { Led::Off } else { Led::On })
}

#[test]
fn select_leds_follow_a_screen_of_the_same_principal_and_publish_nothing() {
    let mut e = signed_in();
    let mut s = Surface::new(units(1), None);
    assert_eq!(select_led(&mut s, &e), Some(Led::Off));
    let mine = screen("ben", "aaaaaaaaaaaaaaaaaaaaaa", "ui");
    selections(
        &mut s,
        &mut e,
        json!([{ "session": mine, "element": "ch1" }]),
    );
    assert_eq!(select_led(&mut s, &e), Some(Led::On));
    // Gone from the list, its panel closed or the screen left: dark.
    selections(&mut s, &mut e, json!([]));
    assert_eq!(select_led(&mut s, &e), Some(Led::Off));
    // A bus is on no strip: every SELECT LED stays dark.
    selections(
        &mut s,
        &mut e,
        json!([{ "session": mine, "element": "pgm" }]),
    );
    assert_eq!(
        select_led(&mut s, &e),
        None,
        "nothing changed on the surface"
    );
    // Following publishes nothing, so no other screen follows through it.
    assert!(selects(&e.take_actions()).is_empty());
    // Other principals and other surfaces are not followed.
    let list = json!([
        { "session": screen("ana", "bbbbbbbbbbbbbbbbbbbbbb", "ui"), "element": "ch1" },
        { "session": screen("ben", "cccccccccccccccccccccc", "console"), "element": "ch1" },
        { "session": mine, "element": "pgm" },
    ]);
    selections(&mut s, &mut e, list);
    assert_eq!(select_led(&mut s, &e), None);
}

#[test]
fn select_publishes_a_press_and_a_screen_following_it_changes_nothing() {
    let mut e = signed_in();
    let mut s = Surface::new(units(1), None);
    key(&mut s, &mut e, 24, true, 0);
    assert_eq!(selects(&e.take_actions()), vec![json!("ch1")]);
    let mine = screen("ben", "aaaaaaaaaaaaaaaaaaaaaa", "ui");
    selections(
        &mut s,
        &mut e,
        json!([{ "session": mine, "element": "ch1" }]),
    );
    assert!(selects(&e.take_actions()).is_empty());
    assert_eq!(select_led(&mut s, &e), Some(Led::On));
    // Again on the channel it published: it lets it go.
    key(&mut s, &mut e, 24, true, 1);
    assert_eq!(selects(&e.take_actions()), vec![Value::Null]);
    // A screen moves on: the LEDs follow, and the press it published
    // before no longer stands. A press on the strip the screen lit
    // publishes it rather than letting go.
    key(&mut s, &mut e, 24, true, 2);
    e.take_actions();
    selections(
        &mut s,
        &mut e,
        json!([{ "session": mine, "element": "pgm" }]),
    );
    assert_eq!(selects(&e.take_actions()), vec![Value::Null]);
    selections(
        &mut s,
        &mut e,
        json!([{ "session": mine, "element": "ch1" }]),
    );
    assert!(selects(&e.take_actions()).is_empty());
    key(&mut s, &mut e, 24, true, 3);
    assert_eq!(selects(&e.take_actions()), vec![json!("ch1")]);
}

#[test]
fn select_takes_the_screen_that_moved_not_one_that_follows_late() {
    let mut e = signed_in();
    let mut s = Surface::new(units(1), None);
    // b sorts first, so its change is met first.
    let a = screen("ben", "cccccccccccccccccccccc", "ui");
    let b = screen("ben", "aaaaaaaaaaaaaaaaaaaaaa", "ui");
    key(&mut s, &mut e, 24, true, 0);
    assert_eq!(select_led(&mut s, &e), Some(Led::On));
    // In one message: screen a moved to pgm, and screen b, before it had
    // seen that, followed the surface's press to ch1. The surface takes
    // a: pgm is on no strip, so strip 1 goes dark.
    selections(
        &mut s,
        &mut e,
        json!([{ "session": a, "element": "pgm" }, { "session": b, "element": "ch1" }]),
    );
    assert_eq!(select_led(&mut s, &e), Some(Led::Off));
    selections(
        &mut s,
        &mut e,
        json!([{ "session": a, "element": "pgm" }, { "session": b, "element": "pgm" }]),
    );
    assert_eq!(select_led(&mut s, &e), None, "b following changes nothing");
}

#[test]
fn select_waits_for_the_principal() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    assert_eq!(select_led(&mut s, &e), Some(Led::Off));
    let mine = screen("anonymous", "aaaaaaaaaaaaaaaaaaaaaa", "ui");
    selections(
        &mut s,
        &mut e,
        json!([{ "session": mine, "element": "ch1" }]),
    );
    assert_eq!(select_led(&mut s, &e), None, "the snapshot named nobody");
}

#[test]
fn everything_is_sent_again_a_little_at_a_time_so_a_surface_that_lost_it_recovers() {
    let mut e = live(1);
    // With the clock on and the time fixed, the digits are part of what
    // goes again.
    let mut s = Surface::new(units(1), None).with_clock(Some(60));
    s.set_time(1_795_683_907);
    let full = s.render(&e, 0);
    assert!(
        full.iter()
            .any(|(_, m)| m[0] == 0xb0 && (0x40..0x4a).contains(&m[1]))
    );
    assert!(s.render(&e, 1).is_empty(), "only what changed");
    // A step sends one part again: first strip 1's fader, nothing else.
    s.tick(&mut e, 100);
    assert_eq!(s.render(&e, 100), vec![(0, mcu::fader(0, 0))]);
    // The steps keep the pace they started with at 0: the next is due
    // at REFRESH_STEP_MS.
    s.tick(&mut e, REFRESH_STEP_MS - 1);
    assert!(s.render(&e, REFRESH_STEP_MS - 1).is_empty());
    // Over one cycle every message of the full state goes again, each
    // step small enough for one 64-octet packet.
    let mut again = Vec::new();
    for step in 1..REFRESH_STEPS as u64 {
        let now = step * REFRESH_STEP_MS;
        s.tick(&mut e, now);
        let out = s.render(&e, now);
        let octets: usize = out.iter().map(|(_, m)| m.len() + 1).sum();
        assert!(
            octets <= crate::rtpmidi::pacer::MAX_PAYLOAD,
            "step {step}: {octets} octets"
        );
        again.extend(out);
    }
    again.push((0, mcu::fader(0, 0)));
    for m in &full {
        assert!(again.contains(m), "{m:?} not sent again");
    }
    assert!(REFRESH_STEPS as u64 * REFRESH_STEP_MS <= REFRESH_CYCLE_MS);
}

#[test]
fn after_a_long_pause_the_refresh_starts_its_pace_anew() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    s.render(&e, 0);
    s.tick(&mut e, 100);
    s.render(&e, 100);
    // Ticks stop for a while, as when the loop was held up: one step
    // goes, not every step missed, and the next is a full step later.
    let late = 100 + 20 * REFRESH_STEP_MS;
    s.tick(&mut e, late);
    // Step 1: strip 1's four key LEDs.
    let out = s.render(&e, late);
    assert_eq!(out.len(), 4, "{out:?}");
    assert!(out.iter().all(|(_, m)| m[0] == 0x90));
    s.tick(&mut e, late + REFRESH_STEP_MS - 1);
    assert!(s.render(&e, late + REFRESH_STEP_MS - 1).is_empty());
    s.tick(&mut e, late + REFRESH_STEP_MS);
    assert!(!s.render(&e, late + REFRESH_STEP_MS).is_empty());
}

#[test]
fn a_sign_in_on_the_first_unit_stays_up_through_the_refresh() {
    let mut e = live(1);
    let mut s = Surface::new(units(2), None);
    let strips = s.render(&e, 0);
    let top = format!("{:<56}", "Sign in: https://idp.example/device");
    let bottom = format!("{:<56}", "Code: WXYZ-1234");
    s.set_sign_in(Some([top.clone(), bottom.clone()]));
    let shown = s.render(&e, 1);
    // Every cell of the code that differs from the blank strip it
    // replaces goes out.
    let cell = |line: &str, strip: usize| line[strip * 7..strip * 7 + 7].to_owned();
    for (line, text) in [(0, &top), (1, &bottom)] {
        for strip in 0..8 {
            let c = cell(text, strip);
            let offset = line * mcu::LINE_CHARS + strip * mcu::STRIP_CHARS;
            let sent = shown.contains(&(0, mcu::lcd(Device::Main, offset, &c)));
            assert!(sent || c.trim().is_empty(), "{c:?}");
        }
    }
    // A full refresh cycle and a resend send the code again, never the
    // strips, on the first unit; the second unit keeps its strips.
    s.resend(0);
    let mut cells = Vec::new();
    for step in 0..=REFRESH_STEPS as u64 {
        let now = 100 + step * REFRESH_STEP_MS;
        s.tick(&mut e, now);
        cells.extend(
            (s.render(&e, now).into_iter())
                .filter(|(u, m)| *u == 0 && m[0] == 0xf0 && m.get(5) == Some(&0x12)),
        );
    }
    assert!(!cells.is_empty());
    // The second unit shows its own strips, as a surface without the
    // sign-in does.
    let mut plain = Surface::new(units(2), None);
    plain.render(&e, 0);
    let mut second = Vec::new();
    let mut expected = Vec::new();
    s.resend(1);
    plain.resend(1);
    let lcd_of_unit_1 = |out: Vec<(usize, Vec<u8>)>| -> Vec<Vec<u8>> {
        (out.into_iter())
            .filter(|(u, m)| *u == 1 && m[0] == 0xf0 && m.get(5) == Some(&0x12))
            .map(|(_, m)| m)
            .collect()
    };
    second.extend(lcd_of_unit_1(s.render(&e, 20_000)));
    expected.extend(lcd_of_unit_1(plain.render(&e, 20_000)));
    assert!(!second.is_empty());
    assert_eq!(second, expected);
    for (_, m) in &cells {
        let (_, offset, text) = mcu::parse_lcd(m).unwrap();
        let line = if offset < mcu::LINE_CHARS {
            &top
        } else {
            &bottom
        };
        let at = offset % mcu::LINE_CHARS;
        assert_eq!(text, line[at..at + text.len()], "{text:?}");
    }
    // Signed in, the strips come back.
    s.set_sign_in(None);
    let back = s.render(&e, 10_000);
    let first_cell = |out: &[(usize, Vec<u8>)]| {
        out.iter()
            .find(|(u, m)| *u == 0 && mcu::parse_lcd(m).is_some_and(|(_, o, _)| o == 0))
            .cloned()
    };
    assert!(first_cell(&strips).is_some());
    assert_eq!(first_cell(&back), first_cell(&strips));
}

#[test]
fn a_refresh_never_moves_a_touched_fader() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    s.render(&e, 0);
    // Strip 1's fader is touched; strip 2's is not.
    s.input(0, &[0x90, 104, 0x7f], &mut e, 1);
    let mut faders = Vec::new();
    for step in 0..REFRESH_STEPS as u64 {
        let now = 100 + step * REFRESH_STEP_MS;
        s.tick(&mut e, now);
        faders.extend(
            s.render(&e, now)
                .into_iter()
                .filter(|(_, m)| m[0] & 0xf0 == 0xe0),
        );
    }
    assert!(faders.contains(&(0, mcu::fader(1, 0))), "{faders:?}");
    assert!(!faders.iter().any(|(_, m)| m[0] == 0xe0), "{faders:?}");
}

#[test]
fn a_new_session_is_sent_everything_again_while_the_surface_settles() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None).without_refresh();
    let full = s.render(&e, 0).len();
    let link = Link {
        connected: true,
        session: 1,
        ..Link::default()
    };
    s.link_changed(0, Link::default(), link, &mut e, 1000);
    e.take_actions();
    assert_eq!(s.render(&e, 1000).len(), full, "at once");
    s.tick(&mut e, 1000 + RESEND_AFTER_MS[0] - 1);
    assert!(s.render(&e, 1000 + RESEND_AFTER_MS[0] - 1).is_empty());
    for after in RESEND_AFTER_MS {
        s.tick(&mut e, 1000 + after);
        assert_eq!(
            s.render(&e, 1000 + after).len(),
            full,
            "again after {after} ms"
        );
    }
    s.tick(&mut e, 10_000);
    assert!(s.render(&e, 10_000).is_empty(), "then only what changes");
}

#[test]
fn a_surface_heard_again_is_sent_everything_now_and_while_it_settles() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None).without_refresh();
    let full = s.render(&e, 0).len();
    let link = Link {
        connected: true,
        session: 1,
        ..Link::default()
    };
    s.link_changed(0, Link::default(), link, &mut e, 0);
    for after in RESEND_AFTER_MS {
        s.tick(&mut e, after);
    }
    s.render(&e, 10_000);
    let back = Link { resync: 1, ..link };
    s.link_changed(0, link, back, &mut e, 20_000);
    assert_eq!(s.render(&e, 20_000).len(), full, "at once");
    for after in RESEND_AFTER_MS {
        s.tick(&mut e, 20_000 + after);
        assert_eq!(
            s.render(&e, 20_000 + after).len(),
            full,
            "again after {after} ms"
        );
    }
}

/// The scribble strip colour messages among `out`.
fn sysex(out: Vec<(usize, Vec<u8>)>) -> Vec<Vec<u8>> {
    (out.into_iter())
        .map(|(_, m)| m)
        .filter(|m| m.get(5) == Some(&0x72))
        .collect()
}

#[test]
fn a_sign_in_shows_on_white_strips_and_the_colours_come_back_after() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    delta(
        &mut e,
        6,
        json!([{ "path": "/channels/ch1/color", "value": "blue" }]),
        json!([]),
    );
    assert_eq!(
        sysex(s.render(&e, 0)),
        vec![mcu::colours(Device::Main, [4, 7, 7, 7, 7, 7, 7, 7])]
    );
    s.set_sign_in(Some([
        format!("{:<56}", "Sign in"),
        format!("{:<56}", "Code"),
    ]));
    assert_eq!(
        sysex(s.render(&e, 1)),
        vec![mcu::colours(Device::Main, [mcu::WHITE; 8])]
    );
    s.set_sign_in(None);
    assert_eq!(
        sysex(s.render(&e, 2)),
        vec![mcu::colours(Device::Main, [4, 7, 7, 7, 7, 7, 7, 7])]
    );
}

#[test]
fn the_scribble_strips_take_the_channels_colours_white_past_the_last() {
    let mut e = live(1);
    let mut s = Surface::new(units(1), None);
    assert_eq!(
        sysex(s.render(&e, 0)),
        vec![mcu::colours(Device::Main, [7; 8])]
    );
    delta(
        &mut e,
        6,
        json!([{ "path": "/channels/ch1/color", "value": "blue" }]),
        json!([]),
    );
    assert_eq!(
        sysex(s.render(&e, 1)),
        vec![mcu::colours(Device::Main, [4, 7, 7, 7, 7, 7, 7, 7])]
    );
    assert!(sysex(s.render(&e, 2)).is_empty(), "only on a change");
}

#[test]
fn a_bank_move_sends_the_new_banks_colours() {
    let mut e = channels(10);
    let mut s = Surface::new(units(1), None);
    delta(
        &mut e,
        6,
        json!([
            { "path": "/channels/ch2/color", "value": "blue" },
            { "path": "/channels/ch9/color", "value": "red" }
        ]),
        json!([]),
    );
    assert_eq!(
        sysex(s.render(&e, 0)),
        vec![mcu::colours(Device::Main, [7, 4, 7, 7, 7, 7, 7, 7])]
    );
    // BANK right: ch9 and ch10, then no channel.
    key(&mut s, &mut e, 47, true, 10);
    assert_eq!(
        sysex(s.render(&e, 20)),
        vec![mcu::colours(Device::Main, [1, 7, 7, 7, 7, 7, 7, 7])]
    );
}

#[test]
fn the_x_touch_shows_the_time_of_day_and_sends_only_the_digits_that_change() {
    let e = live(1);
    let mut s = Surface::new(units(2), None).with_clock(Some(60));
    let timecode = |out: &[(usize, Vec<u8>)], unit: usize| -> Vec<Vec<u8>> {
        (out.iter())
            .filter(|(u, m)| *u == unit && m[0] == 0xb0 && (0x40..0x4a).contains(&m[1]))
            .map(|(_, m)| m.clone())
            .collect()
    };
    assert!(
        timecode(&s.render(&e, 0), 0).is_empty(),
        "no clock before the time is set"
    );
    // 09:05:07 UTC, 10:05:07 at +01:00.
    s.set_time(1_795_683_907);
    let out = s.render(&e, 1);
    assert_eq!(timecode(&out, 0).len(), mcu::TIMECODE_DIGITS);
    assert!(
        timecode(&out, 1).is_empty(),
        "an Extender has no timecode display"
    );
    assert!(out.contains(&(0, mcu::timecode_digit(8, b'1', false))));
    assert!(out.contains(&(0, mcu::timecode_digit(7, b'0', true))));
    s.set_time(1_795_683_908);
    assert_eq!(
        timecode(&s.render(&e, 2), 0),
        vec![mcu::timecode_digit(3, b'8', false)]
    );
    s.resend(0);
    assert_eq!(
        timecode(&s.render(&e, 3), 0).len(),
        mcu::TIMECODE_DIGITS,
        "a unit shown everything again gets the clock again"
    );
    let mut dark = Surface::new(units(1), None);
    dark.set_time(1_795_683_907);
    assert!(
        timecode(&dark.render(&e, 0), 0).is_empty(),
        "no clock unless one is configured"
    );
}
