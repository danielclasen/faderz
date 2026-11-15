//! A faderz server on loopback ports, spoken to as clients do: every
//! message it sends is checked against api/ws/server-message.schema.json.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use faderz::config::Config;
use faderz::contract::{check, contract};
use faderz::server::Server;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

static DIRS: AtomicU64 = AtomicU64::new(0);

fn shape() -> Value {
    json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "ch1", "name": "Pres 1", "width": 1, "layout": "mono", "altInput": true },
            { "id": "ch2", "name": "Pres 2", "width": 1, "layout": "mono", "altInput": false },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [],
        "mixMinus": [{ "id": "mm1", "name": "N-1", "bus": "pgm", "width": 2, "layout": "stereo" }],
        "vcas": [],
        "monitorOutputs": [],
        "outputs": [],
    })
}

/// A fresh data directory under the target directory.
fn data_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "faderz-server-{}-{}",
        std::process::id(),
        DIRS.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn start(dir: &std::path::Path) -> Server {
    let listener = json!({ "bind": "127.0.0.1", "port": 0 });
    let config = json!({
        "listeners": { "api": listener, "ops": listener, "nmos": listener },
        "replica": { "name": "r1", "dataDir": dir },
        "shape": shape(),
    });
    let (config, shape, name) = Config::parse(&config.to_string(), || unreachable!()).unwrap();
    Server::start(&config, shape, name).await.unwrap()
}

/// A client on one WebSocket connection that answers pings and checks
/// every message against the contract.
struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    session: String,
    seq: u64,
}

impl Client {
    async fn connect(server: &Server, session: &str) -> Client {
        let url = format!("ws://{}/api/v1/ws?session={session}", server.api);
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        Client {
            ws,
            session: session.into(),
            seq: 0,
        }
    }

    async fn send(&mut self, message: Value) {
        self.ws
            .send(Message::text(message.to_string()))
            .await
            .unwrap();
    }

    /// The next message other than a heartbeat, checked against the
    /// contract.
    async fn next(&mut self) -> Value {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), self.ws.next())
                .await
                .expect("a message within 5 s")
                .expect("open")
                .unwrap();
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            check(&contract().server_message, &value).unwrap_or_else(|e| panic!("{e}: {value}"));
            match value["type"].as_str() {
                Some("ping") => self.send(json!({ "type": "pong", "t": value["t"] })).await,
                Some("pong") => {}
                _ => return value,
            }
        }
    }

    /// The next message of `kind`, skipping others.
    async fn next_of(&mut self, kind: &str) -> Value {
        loop {
            let m = self.next().await;
            if m["type"] == kind {
                return m;
            }
        }
    }

    async fn hello(&mut self) -> Value {
        let session = self.session.clone();
        self.send(json!({ "type": "hello", "session": session, "kind": "ui", "mode": "primary" }))
            .await;
        assert_eq!(
            self.next().await["type"],
            "replicas",
            "replicas go ahead of the state"
        );
        self.next().await
    }

    /// Sends a command; returns its id.
    async fn command(&mut self, kind: &str, path: &str, value: Option<Value>) -> String {
        self.seq += 1;
        let id = faderz::app::uuid().unwrap();
        let mut m = json!({ "type": kind, "id": id, "seq": self.seq, "path": path });
        if let Some(v) = value {
            m["value"] = v;
        }
        self.send(m).await;
        id
    }

    /// What answered command `id`: an ack, an error or the delta listing
    /// it among its origins.
    async fn answer(&mut self, id: &str) -> Value {
        loop {
            let m = self.next().await;
            let origins = m["origins"].as_array().into_iter().flatten();
            if m["commandId"] == id || origins.into_iter().any(|o| o["commandId"] == id) {
                return m;
            }
        }
    }
}

const SESSION_A: &str = "session-a-0123456789abcdef";
const SESSION_B: &str = "session-b-0123456789abcdef";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_gets_the_state_and_its_own_writes_as_deltas() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    let snapshot = a.hello().await;
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["revision"], 0);
    assert_eq!(snapshot["replica"], json!({ "id": "r1", "role": "leader" }));
    assert_eq!(snapshot["you"]["kind"], "ui");
    let id = a
        .command("set", "/channels/ch1/fader", Some(json!(-3)))
        .await;
    let delta = a.answer(&id).await;
    assert_eq!(delta["type"], "delta");
    assert_eq!(delta["revision"], 1);
    assert_eq!(
        delta["changes"],
        json!([{ "path": "/channels/ch1/fader", "value": -3 }])
    );
    assert!(delta["atIndex"].as_u64().is_some());
    // The same value again changes nothing: an ack, no revision.
    let id = a
        .command("set", "/channels/ch1/fader", Some(json!(-3)))
        .await;
    let ack = a.answer(&id).await;
    assert_eq!(
        (
            ack["type"].as_str(),
            ack["result"].as_str(),
            &ack["revision"]
        ),
        (Some("ack"), Some("noop"), &json!(1))
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refusals_carry_their_code_and_command() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    a.send(json!({ "type": "set", "id": faderz::app::uuid().unwrap(), "seq": 1, "path": "/channels/ch1/fader", "value": 0 })).await;
    let e = a.next().await;
    assert_eq!(
        (e["code"].as_u64(), e["message"].as_str()),
        (Some(400), Some("hello first"))
    );
    a.hello().await;
    let id = a
        .command("set", "/channels/ch9/fader", Some(json!(0)))
        .await;
    let e = a.answer(&id).await;
    assert_eq!(e["code"], 404);
    let id = a
        .command("set", "/channels/ch2/input", Some(json!("b")))
        .await;
    assert_eq!(a.answer(&id).await["code"], 400, "ch2 has no B input");
    let id = a
        .command("set", "/channels/ch1/fader", Some(json!(99)))
        .await;
    assert_eq!(a.answer(&id).await["code"], 400, "above +10 dB");
    a.send(json!({ "type": "nope" })).await;
    assert_eq!(a.next().await["code"], 400);
    server.stop().await;
}

/// A resend of a command id is answered as a duplicate, never applied
/// twice; a seq not above the last one of the control is stale.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copies_and_old_seqs_change_nothing() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    let id = faderz::app::uuid().unwrap();
    let set =
        json!({ "type": "set", "id": id, "seq": 10, "path": "/channels/ch1/fader", "value": -3 });
    a.send(set.clone()).await;
    assert_eq!(a.answer(&id).await["type"], "delta");
    a.send(set).await;
    let ack = a.answer(&id).await;
    assert_eq!(
        (ack["result"].as_str(), &ack["revision"]),
        (Some("duplicate"), &json!(1))
    );
    let old = faderz::app::uuid().unwrap();
    a.send(
        json!({ "type": "set", "id": old, "seq": 9, "path": "/channels/ch1/fader", "value": -9 }),
    )
    .await;
    assert_eq!(a.answer(&old).await["result"], "stale");
    server.stop().await;
}

/// Touch ownership: another session's write gets 409 with the value and
/// the owner; once the owner's connection is gone its lease runs out and
/// every client sees the control released.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_touch_holds_until_its_session_is_gone_for_the_lease() {
    let dir = data_dir();
    let server = start(&dir).await;
    let path = "/channels/ch1/fader";
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    let mut b = Client::connect(&server, SESSION_B).await;
    b.hello().await;
    let id = a.command("touch", path, None).await;
    let touched = a.answer(&id).await;
    let id = b.command("set", path, Some(json!(0))).await;
    let e = b.answer(&id).await;
    assert_eq!(
        (e["code"].as_u64(), &e["current"]),
        (Some(409), &json!(-90))
    );
    assert_eq!(
        e["owner"]["handle"],
        touched["owners"][0]["owner"]["handle"]
    );
    drop(a);
    let released = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let m = b.next_of("delta").await;
            if m["owners"] == json!([{ "path": path, "owner": null }]) {
                return m;
            }
        }
    })
    .await
    .expect("released within the 2 s lease plus a renewal period");
    assert!(released.get("origins").is_none(), "a server-side change");
    let id = b.command("set", path, Some(json!(0))).await;
    assert_eq!(b.answer(&id).await["type"], "delta");
    server.stop().await;
}

/// A client naming the last position it saw gets the deltas after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hello_with_a_position_resumes_from_it() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    let snapshot = a.hello().await;
    let epoch = snapshot["epoch"].clone();
    for v in [-1, -2] {
        let id = a
            .command("set", "/channels/ch2/fader", Some(json!(v)))
            .await;
        a.answer(&id).await;
    }
    let mut b = Client::connect(&server, SESSION_B).await;
    b.send(json!({ "type": "hello", "session": SESSION_B, "kind": "ui", "mode": "standby", "lastRevision": 1, "epoch": epoch })).await;
    b.next_of("replicas").await;
    let resume = b.next().await;
    assert_eq!(resume["type"], "resume");
    assert_eq!(resume["revision"], 2);
    let deltas = resume["deltas"].as_array().unwrap();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0]["changes"][0]["value"], -2);
    b.send(json!({ "type": "resync", "lastRevision": 2, "epoch": "1", "full": true }))
        .await;
    assert_eq!(
        b.next().await["type"],
        "snapshot",
        "another epoch's position"
    );
    server.stop().await;
}

/// The state, its revision and its epoch survive a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_keeps_the_state_and_its_epoch() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    let before = a.hello().await;
    let id = a
        .command("set", "/channels/ch1/fader", Some(json!(-7)))
        .await;
    a.answer(&id).await;
    let id = a.command("touch", "/channels/ch2/fader", None).await;
    a.answer(&id).await;
    drop(a);
    server.stop().await;
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    let after = a.hello().await;
    assert_eq!(after["epoch"], before["epoch"]);
    assert_eq!(after["revision"], 2);
    assert_eq!(after["state"]["channels"]["ch1"]["fader"], -7);
    assert!(
        after["owners"].get("/channels/ch2/fader").is_some(),
        "ownership is state"
    );
    server.stop().await;
    let fresh = data_dir();
    let server = start(&fresh).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    assert_ne!(
        a.hello().await["epoch"],
        before["epoch"],
        "an empty start draws a new epoch"
    );
    server.stop().await;
}
