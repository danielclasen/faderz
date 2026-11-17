//! The bridge against a real faderz process: FADERZ_BIN names the faderz
//! binary. Two emulated units (an X-Touch and an Extender) drive it, and
//! a second client of kind ui watches the state and plays the screen.
//!
//! Run with `FADERZ_BIN=... cargo test --features e2e --test e2e`.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use faderz_bridge::bridge::{self, Options, Static};
use faderz_bridge::emulator::Emulator;
use faderz_bridge::mcu::{Device, Key, Led};
use faderz_bridge::mirror::Mirror;
use faderz_bridge::session;
use faderz_bridge::surface::UnitSpec;
use faderz_bridge::taper;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

const WAIT: Duration = Duration::from_secs(10);

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn shape() -> Value {
    let channels: Vec<Value> = (1..=10)
        .map(|i| {
            json!({ "id": format!("ch{i}"), "name": format!("Chan {i}"), "width": 1,
                "layout": "mono", "altInput": false })
        })
        .collect();
    json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": channels,
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [], "outputs": [],
    })
}

struct Faderz {
    child: Option<Child>,
    port: u16,
    ops: u16,
    nmos: u16,
    dir: PathBuf,
}

impl Faderz {
    async fn start() -> Faderz {
        let dir =
            std::env::temp_dir().join(format!("bridge-e2e-{}", session::random_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let (port, ops, nmos) = (free_port(), free_port(), free_port());
        let mut f = Faderz {
            child: None,
            port,
            ops,
            nmos,
            dir,
        };
        f.spawn().await;
        f
    }

    /// Starts the process on this instance's ports and data directory.
    async fn spawn(&mut self) {
        let bin = std::env::var("FADERZ_BIN").expect("FADERZ_BIN names the faderz binary");
        let listener = |port: u16| json!({ "bind": "127.0.0.1", "port": port });
        let config = json!({
            "listeners": { "api": listener(self.port), "ops": listener(self.ops), "nmos": listener(self.nmos) },
            "replica": { "name": "r1", "dataDir": self.dir.join("data") },
            "shape": shape(),
        });
        let path = self.dir.join("config.json");
        std::fs::write(&path, config.to_string()).unwrap();
        let child = Command::new(bin)
            .arg("--config")
            .arg(&path)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        self.child = Some(child);
        for _ in 0..200 {
            if self
                .get("/api/v1/replicas")
                .await
                .is_some_and(|r| r.starts_with("HTTP/1.1 200"))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("faderz did not come up");
    }

    async fn get(&self, path: &str) -> Option<String> {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", self.port))
            .await
            .ok()?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.ok()?;
        let mut out = String::new();
        s.read_to_string(&mut out).await.ok()?;
        Some(out)
    }

    fn endpoint(&self) -> String {
        format!("ws://127.0.0.1:{}/api/v1/ws", self.port)
    }

    fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Faderz {
    fn drop(&mut self) {
        self.kill();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A ui client: keeps the state it is sent and writes like a screen.
struct Screen {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    mirror: Mirror,
    seq: u64,
}

impl Screen {
    async fn connect(f: &Faderz) -> Screen {
        let session = session::random_id().unwrap();
        let url = bridge::with_session(&f.endpoint(), &session);
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let hello = json!({ "type": "hello", "session": session, "kind": "ui", "mode": "primary" });
        ws.send(Message::text(hello.to_string())).await.unwrap();
        let mut s = Screen {
            ws,
            mirror: Mirror::default(),
            seq: 1,
        };
        s.until(|m| m.ready()).await;
        s
    }

    /// Takes in messages until `done` holds of the state.
    async fn until(&mut self, done: impl Fn(&Mirror) -> bool) -> &Mirror {
        let deadline = tokio::time::Instant::now() + WAIT;
        while !done(&self.mirror) {
            let next = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "timed out; state {}, owners {:?}",
                        self.mirror.state, self.mirror.owners
                    )
                });
            let Some(Ok(Message::Text(t))) = next else {
                continue;
            };
            let m: Value = serde_json::from_str(t.as_str()).unwrap();
            match m["type"].as_str() {
                Some("snapshot") => self.mirror.snapshot(&m),
                Some("delta") => self.mirror.apply(&m),
                Some("ping") => {
                    let pong = json!({ "type": "pong", "t": m["t"] });
                    self.ws.send(Message::text(pong.to_string())).await.unwrap();
                }
                _ => {}
            }
        }
        &self.mirror
    }

    async fn command(&mut self, kind: &str, path: &str, value: Option<Value>) {
        self.seq += 1;
        let mut m = json!({ "type": kind, "id": session::uuid(), "seq": self.seq, "path": path });
        if let Some(v) = value {
            m["value"] = v;
        }
        self.ws.send(Message::text(m.to_string())).await.unwrap();
    }
}

struct Bridge {
    main: Emulator,
    extender: Emulator,
    handle: String,
    stop: Option<oneshot::Sender<()>>,
}

impl Bridge {
    fn start(f: &Faderz) -> Bridge {
        let session = session::random_id().unwrap();
        let handle = session::handle(&session);
        let (main, main_port) = Emulator::new(Device::Main);
        let (extender, extender_port) = Emulator::new(Device::Extender);
        let units = vec![
            UnitSpec {
                id: "xt".into(),
                name: "X-Touch".into(),
                device: Device::Main,
            },
            UnitSpec {
                id: "xt-ext".into(),
                name: "X-Touch Ext".into(),
                device: Device::Extender,
            },
        ];
        let options = Options {
            endpoints: vec![f.endpoint()],
            session,
            credentials: Arc::new(Static(None)),
            units,
            master: None,
        };
        let (stop, stopped) = oneshot::channel::<()>();
        tokio::spawn(bridge::run(
            options,
            vec![main_port, extender_port],
            async {
                let _ = stopped.await;
            },
        ));
        Bridge {
            main,
            extender,
            handle,
            stop: Some(stop),
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn console<'a>(m: &'a Mirror, id: &str) -> Option<&'a Value> {
    m.state["consoles"]
        .as_array()?
        .iter()
        .find(|c| c["id"] == id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_units_register_and_ride_faders_keys_and_banks() {
    let f = Faderz::start().await;
    let mut screen = Screen::connect(&f).await;
    let mut b = Bridge::start(&f);
    let handle = b.handle.clone();

    // Both units are consoles of the bridge's session.
    screen
        .until(|m| {
            let ok = |id: &str, bank: u64| {
                console(m, id).is_some_and(|c| {
                    c["connected"] == true
                        && c["handle"] == handle.as_str()
                        && c["bankOffset"] == bank
                        && c["protocol"] == "mcu"
                        && c["transport"] == "bridge"
                        && c["strips"] == 8
                })
            };
            ok("xt", 0) && ok("xt-ext", 8)
        })
        .await;
    // Names on the scribble strips: channel 9 is the Extender's first strip.
    assert!(b.main.until(WAIT, |e| e.cell(0, 0) == "Chan 1 ").await);
    assert!(
        b.extender
            .until(WAIT, |e| e.cell(0, 0) == "Chan 9 "
                && e.cell(0, 2) == "       ")
            .await
    );

    // A touched fader takes ownership and writes; letting go releases it.
    b.main.touch(0);
    screen
        .until(|m| {
            m.owners
                .get("/channels/ch1/fader")
                .is_some_and(|o| o["handle"] == handle.as_str() && o["kind"] == "console")
        })
        .await;
    b.main.move_fader(0, taper::db_to_fader(-10.0));
    screen
        .until(|m| m.get("/channels/ch1/fader") == Some(&json!(-10.0)))
        .await;
    b.main.untouch(0);
    screen
        .until(|m| !m.owners.contains_key("/channels/ch1/fader"))
        .await;

    // A screen's write moves the motor.
    screen
        .command("set", "/channels/ch2/fader", Some(json!(-20)))
        .await;
    let want = taper::db_to_fader(-20.0);
    assert!(b.main.until(WAIT, |e| e.faders[1] == want).await);
    assert!(b.main.until(WAIT, |e| e.cell(1, 1) == "-20.0  ").await);

    // MUTE writes, and its LED follows the state.
    b.main.press(Key::Mute(0));
    screen
        .until(|m| m.get("/channels/ch1/mute") == Some(&json!(true)))
        .await;
    assert!(b.main.until(WAIT, |e| e.led(Key::Mute(0)) == Led::On).await);

    // BANK RIGHT moves both units; the last bank starts at the last channel.
    b.main.press(Key::BankRight);
    screen
        .until(|m| {
            console(m, "xt").is_some_and(|c| c["bankOffset"] == 9)
                && console(m, "xt-ext").is_some_and(|c| c["bankOffset"] == 17)
        })
        .await;
    assert!(
        b.main
            .until(WAIT, |e| e.cell(0, 0) == "Chan 10"
                && e.cell(0, 1) == "       ")
            .await
    );

    // The master fader drives the program bus.
    b.main.touch(8);
    b.main.move_fader(8, taper::db_to_fader(-3.0));
    screen
        .until(|m| m.get("/buses/pgm/fader") == Some(&json!(-3.0)))
        .await;
    b.main.untouch(8);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fader_held_on_a_screen_is_refused_and_shown() {
    let f = Faderz::start().await;
    let mut screen = Screen::connect(&f).await;
    let mut b = Bridge::start(&f);
    assert!(b.main.until(WAIT, |e| e.cell(0, 0) == "Chan 1 ").await);
    screen.command("touch", "/channels/ch1/fader", None).await;
    screen
        .until(|m| m.owners.contains_key("/channels/ch1/fader"))
        .await;

    b.main.touch(0);
    b.main.move_fader(0, taper::db_to_fader(5.0));
    // The holder is shown and the strip's SELECT blinks.
    assert!(
        b.main
            .until(WAIT, |e| e.cell(1, 0) == "screen "
                && e.led(Key::Select(0)) == Led::Blink)
            .await
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    screen.until(|_| true).await;
    assert_eq!(screen.mirror.get("/channels/ch1/fader"), Some(&json!(-90)));
    // Let go, the motor goes back to the state.
    b.main.untouch(0);
    assert!(
        b.main
            .until(WAIT, |e| e.faders[0] == 0
                && e.led(Key::Select(0)) == Led::Off)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bridge_reconnects_after_faderz_restarts() {
    let mut f = Faderz::start().await;
    let mut b = Bridge::start(&f);
    {
        let mut screen = Screen::connect(&f).await;
        screen
            .until(|m| console(m, "xt").is_some_and(|c| c["connected"] == true))
            .await;
    }
    f.kill();
    f.spawn().await;
    let mut screen = Screen::connect(&f).await;
    let handle = b.handle.clone();
    screen
        .until(|m| {
            console(m, "xt")
                .is_some_and(|c| c["connected"] == true && c["handle"] == handle.as_str())
        })
        .await;
    b.main.touch(2);
    b.main.move_fader(2, taper::db_to_fader(0.0));
    screen
        .until(|m| m.get("/channels/ch3/fader") == Some(&json!(0.0)))
        .await;
    b.main.untouch(2);
}
