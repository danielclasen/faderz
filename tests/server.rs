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
        "ducks": [{ "id": "comm", "name": "Commentary" }],
        "monitorOutputs": [],
        "outputs": [],
    })
}

/// A fresh data directory, removed when dropped.
struct Dir(PathBuf);

impl std::ops::Deref for Dir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A replica's data directory on tmpfs: on a disk busy with builds one
/// fsync of its log can outlast the 2 s a write waits to commit, and the
/// write is answered 503, not applied.
fn data_dir() -> Dir {
    let shm = PathBuf::from("/dev/shm");
    let base = if shm.is_dir() {
        shm
    } else {
        PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
    };
    let dir = base.join(format!(
        "faderz-server-{}-{}",
        std::process::id(),
        DIRS.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    Dir(dir)
}

async fn start(dir: &std::path::Path) -> Server {
    start_with(dir, json!({ "mode": "none" })).await
}

async fn start_with(dir: &std::path::Path, auth: Value) -> Server {
    start_configured(dir, auth, json!([])).await
}

async fn start_configured(dir: &std::path::Path, auth: Value, consoles: Value) -> Server {
    let listener = json!({ "bind": "127.0.0.1", "port": 0 });
    let config = json!({
        "listeners": { "api": listener, "ops": listener, "nmos": listener },
        "auth": auth,
        // The listeners take port 0, which the default origins name.
        "api": { "allowedOrigins": ["http://faderz.test"] },
        "replica": { "name": "r1", "dataDir": dir },
        "shape": shape(),
        "consoles": consoles,
    });
    let (config, shape, name) = Config::parse(&config.to_string(), || unreachable!()).unwrap();
    Server::start(&config, shape, name, None).await.unwrap()
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

    /// The next message other than a heartbeat, `selections` or
    /// `presence`, checked against the contract.
    async fn next(&mut self) -> Value {
        loop {
            let value = self.recv().await;
            if value["type"] != "selections" && value["type"] != "presence" {
                return value;
            }
        }
    }

    /// The next message other than a heartbeat, `selections` and
    /// `presence` included, within 5 s: heartbeats do not extend the wait.
    async fn recv(&mut self) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let message = tokio::time::timeout_at(deadline, self.ws.next())
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

    /// The messages other than heartbeats within `wait`, pings answered
    /// meanwhile so the server keeps the connection.
    async fn during(&mut self, wait: Duration) -> Vec<Value> {
        let end = tokio::time::Instant::now() + wait;
        let mut got = Vec::new();
        while let Ok(m) = tokio::time::timeout_at(end, self.recv()).await {
            got.push(m);
        }
        got
    }

    /// The next message of `kind` within [`WAIT`], skipping others.
    async fn next_of(&mut self, kind: &str) -> Value {
        let wait = async {
            loop {
                let m = if kind == "selections" || kind == "presence" {
                    self.recv().await
                } else {
                    self.next().await
                };
                if m["type"] == kind {
                    return m;
                }
            }
        };
        tokio::time::timeout(WAIT, wait)
            .await
            .unwrap_or_else(|_| panic!("no {kind} within {WAIT:?}"))
    }

    async fn hello(&mut self) -> Value {
        self.hello_as("primary").await
    }

    async fn hello_as(&mut self, mode: &str) -> Value {
        let session = self.session.clone();
        self.send(json!({ "type": "hello", "session": session, "kind": "ui", "mode": mode }))
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
        let wait = async {
            loop {
                let m = self.next().await;
                let origins = m["origins"].as_array().into_iter().flatten();
                if m["commandId"] == id || origins.into_iter().any(|o| o["commandId"] == id) {
                    return m;
                }
            }
        };
        tokio::time::timeout(WAIT, wait)
            .await
            .unwrap_or_else(|_| panic!("no answer to {id} within {WAIT:?}"))
    }
}

/// The longest a test waits for a message it skips others for.
const WAIT: Duration = Duration::from_secs(10);

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
    assert_eq!(delta["type"], "delta", "{delta}");
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

/// select and request are not commands: nothing answers them and they
/// change nothing; in particular neither is taken for a release.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn select_and_request_are_not_answered_and_change_nothing() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    let id = a.command("touch", "/channels/ch1/fader", None).await;
    assert_eq!(a.answer(&id).await["type"], "delta");
    a.send(json!({ "type": "select", "element": "ch1" })).await;
    a.send(json!({ "type": "request", "path": "/channels/ch1/fader" }))
        .await;
    a.send(json!({ "type": "select", "element": null })).await;
    // The next message is the answer to this set: nothing answered the
    // three before it, and the touch still holds.
    let id = a
        .command("set", "/channels/ch1/fader", Some(json!(-7)))
        .await;
    let next = a.next().await;
    assert_eq!(next["type"], "delta", "{next}");
    assert!(next["origins"][0]["commandId"] == id, "{next}");
    assert!(
        next.get("owners").is_none(),
        "no release was applied: {next}"
    );
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
    let id = a
        .command("set", "/mixMinus/mm1/interrupt/source", Some(json!("ch1")))
        .await;
    a.answer(&id).await;
    drop(a);
    // From a snapshot as well as from the log.
    let raft = server.app.replica.raft().clone();
    raft.trigger().snapshot().await.unwrap();
    raft.wait(Some(Duration::from_secs(5)))
        .metrics(|m| m.snapshot.is_some(), "snapshot")
        .await
        .unwrap();
    server.stop().await;
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    let after = a.hello().await;
    assert_eq!(after["epoch"], before["epoch"]);
    assert_eq!(after["revision"], 3);
    assert_eq!(after["state"]["channels"]["ch1"]["fader"], -7);
    assert_eq!(
        after["state"]["mixMinus"]["mm1"]["interrupt"]["source"],
        "ch1"
    );
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

/// The REST routes, every answer checked against api/openapi.yaml.
struct Rest {
    addr: std::net::SocketAddr,
    registry: jsonschema::Registry<'static>,
}

const OPENAPI: &str = "https://github.com/danielclasen/faderz/raw/main/api/openapi.yaml";

impl Rest {
    fn new(server: &Server) -> Rest {
        let openapi: Value = serde_yaml::from_str(include_str!("../api/openapi.yaml")).unwrap();
        let model: Value =
            serde_json::from_str(include_str!("../api/ws/model.schema.json")).unwrap();
        let counters: Value =
            serde_json::from_str(include_str!("../api/ws/counters.schema.json")).unwrap();
        let registry = jsonschema::Registry::new()
            .add(OPENAPI, openapi)
            .unwrap()
            .add(
                "https://github.com/danielclasen/faderz/raw/main/api/ws/model.schema.json",
                model,
            )
            .unwrap()
            .add(
                "https://github.com/danielclasen/faderz/raw/main/api/ws/counters.schema.json",
                counters,
            )
            .unwrap()
            .prepare()
            .unwrap();
        Rest {
            addr: server.api,
            registry,
        }
    }

    /// Sends a request and checks the answer against the schema at
    /// `pointer` in openapi.yaml; `None` for an answer without a body.
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        pointer: Option<&str>,
    ) -> (u16, Value) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let mut tcp = TcpStream::connect(self.addr).await.unwrap();
        let request = format!(
            "{method} /api/v1{path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
             X-Samplez-Request: 1\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        tcp.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        tcp.read_to_string(&mut answer).await.unwrap();
        let status: u16 = answer[9..12].parse().unwrap();
        let text = answer.split_once("\r\n\r\n").map_or("", |(_, b)| b);
        let value = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(text).unwrap()
        };
        match pointer {
            Some(pointer) => self.check(pointer, &value),
            None => assert!(text.is_empty(), "{method} {path} {status}: {text}"),
        }
        (status, value)
    }

    /// Checks `value` against the schema at `pointer` in openapi.yaml.
    fn check(&self, pointer: &str, value: &Value) {
        let schema = json!({ "$ref": format!("{OPENAPI}#{pointer}") });
        let v = jsonschema::options()
            .with_registry(&self.registry)
            .should_validate_formats(true)
            .build(&schema)
            .unwrap();
        check(&v, value).unwrap_or_else(|e| panic!("{pointer}: {e}: {value}"));
    }
}

const RESULT: &str = "/components/schemas/CommandResult";
const ERROR: &str = "/components/schemas/Error";

/// `PATCH /state` sets up and latches a duck group atomically and is
/// refused a channel that would trigger and be ducked by one group, and
/// a hold, which only a WebSocket session holding it sets; the state
/// read back matches the contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duck_groups_are_set_through_the_api() {
    let dir = data_dir();
    let server = start(&dir).await;
    let rest = Rest::new(&server);
    let id = || faderz::app::uuid().unwrap();
    let patch = |changes: Value| json!({ "commandId": id(), "changes": changes });
    let setup = patch(json!([
        { "path": "/ducks/comm/channels/ch1/trigger", "value": true },
        { "path": "/ducks/comm/channels/ch2/target", "value": true },
        { "path": "/ducks/comm/channels/ch2/depth", "value": 15 },
        { "path": "/ducks/comm/mode", "value": "on" },
    ]));
    let (s, _) = rest
        .call("PATCH", "/state", Some(setup), Some(RESULT))
        .await;
    assert_eq!(s, 200);
    let get = "/paths/~1state/get/responses/200/content/application~1json/schema";
    let (_, doc) = rest.call("GET", "/state", None, Some(get)).await;
    let comm = &doc["state"]["ducks"]["comm"];
    assert_eq!(
        (&comm["mode"], &comm["channels"]["ch2"]["depth"]),
        (&json!("on"), &json!(15))
    );
    assert_eq!(doc["shape"]["ducks"][0]["id"], "comm");
    let both = patch(json!([{ "path": "/ducks/comm/channels/ch2/trigger", "value": true }]));
    let (s, e) = rest.call("PATCH", "/state", Some(both), Some(ERROR)).await;
    assert_eq!(s, 400, "{e}");
    let hold = patch(json!([{ "path": "/ducks/comm/hold", "value": true }]));
    let (s, e) = rest.call("PATCH", "/state", Some(hold), Some(ERROR)).await;
    assert_eq!(s, 400, "{e}");

    let path = "/ducks/comm/hold";
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    let c = a.command("set", path, Some(json!(true))).await;
    assert_eq!(a.answer(&c).await["code"], 400, "not held");
    let c = a.command("touch", path, None).await;
    a.answer(&c).await;
    let c = a.command("set", path, Some(json!(true))).await;
    let d = a.answer(&c).await;
    assert_eq!(d["changes"], json!([{ "path": path, "value": true }]));
    let c = a.command("release", path, None).await;
    let d = a.answer(&c).await;
    assert_eq!(d["changes"], json!([{ "path": path, "value": false }]));
    server.stop().await;
}

/// The build a replica names, on the WebSocket and over REST, is the one
/// it was built from: the crate's version, and the samplez release that
/// Cargo.lock pins and that the samplez crate linked in says it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replica_names_the_build_cargo_lock_pins() {
    let lock = include_str!("../Cargo.lock");
    let source = (lock.split("[[package]]"))
        .find(|p| p.lines().any(|l| l.trim() == r#"name = "samplez""#))
        .and_then(|p| p.lines().find_map(|l| l.trim().strip_prefix("source = ")))
        .expect("Cargo.lock pins samplez from git");
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    a.send(json!({ "type": "hello", "session": SESSION_A, "kind": "ui", "mode": "primary" }))
        .await;
    let replicas = a.next_of("replicas").await;
    let build = &replicas["build"];
    assert_eq!(build["faderz"], env!("CARGO_PKG_VERSION"));
    let samplez = build["samplez"].as_str().unwrap();
    match samplez.split_once('+') {
        // A commit no tag names: the version, and the commit pinned.
        Some((version, commit)) => {
            assert_eq!(version, samplez::ENGINE_VERSION);
            assert!(source.contains(&format!("#{commit}")), "{source}");
        }
        None => {
            assert_eq!(samplez, samplez::ENGINE_VERSION);
            assert!(source.contains(&format!("?tag=v{samplez}#")), "{source}");
        }
    }
    let rest = Rest::new(&server);
    let (s, v) = rest
        .call(
            "GET",
            "/replicas",
            None,
            Some("/paths/~1replicas/get/responses/200/content/application~1json/schema"),
        )
        .await;
    assert_eq!((s, &v["build"]), (200, build));
    server.stop().await;
}

/// GET /about answers with the CHANGELOG.md of the checkout the binary
/// was built from, byte for byte, and the build the replica names: the
/// changelog is in the binary, so a replica with no way out answers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn about_serves_the_changelog_the_binary_was_built_with() {
    let dir = data_dir();
    let server = start(&dir).await;
    let rest = Rest::new(&server);
    let (s, v) = rest
        .call(
            "GET",
            "/about",
            None,
            Some("/paths/~1about/get/responses/200/content/application~1json/schema"),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["changelog"], include_str!("../CHANGELOG.md"));
    assert_eq!(v["build"], faderz::app::build());
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rest_answers_match_the_contract() {
    let dir = data_dir();
    let server = start(&dir).await;
    let rest = Rest::new(&server);
    let id = || faderz::app::uuid().unwrap();
    let (s, _) = rest
        .call(
            "GET",
            "/state",
            None,
            Some("/paths/~1state/get/responses/200/content/application~1json/schema"),
        )
        .await;
    assert_eq!(s, 200);
    let (s, v) = rest
        .call(
            "GET",
            "/session",
            None,
            Some("/paths/~1session/get/responses/200/content/application~1json/schema"),
        )
        .await;
    assert_eq!(
        (s, v),
        (
            200,
            json!({ "mode": "none", "user": null, "role": "operator" })
        )
    );
    let (s, _) = rest
        .call(
            "GET",
            "/replicas",
            None,
            Some("/paths/~1replicas/get/responses/200/content/application~1json/schema"),
        )
        .await;
    assert_eq!(s, 200);
    // A set of one without an engine: no contact, no engine.
    let (s, v) = rest
        .call("GET", "/status", None, Some("/components/schemas/Status"))
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["replica"]["role"], "leader");
    assert_eq!(v["replica"]["ready"], true);
    assert_eq!(
        (&v["replica"]["state"], &v["engine"]),
        (&Value::Null, &Value::Null)
    );
    let write =
        json!({ "commandId": id(), "changes": [{ "path": "/channels/ch1/fader", "value": -3 }] });
    let (s, r) = rest
        .call("PATCH", "/state", Some(write.clone()), Some(RESULT))
        .await;
    assert_eq!((s, r["result"].as_str()), (200, Some("applied")));
    assert!(r["atIndex"].as_u64().is_some());
    let (_, r) = rest
        .call("PATCH", "/state", Some(write), Some(RESULT))
        .await;
    assert_eq!(r["result"], "duplicate");
    let unknown =
        json!({ "commandId": id(), "changes": [{ "path": "/channels/zz/fader", "value": 0 }] });
    assert_eq!(
        rest.call("PATCH", "/state", Some(unknown), Some(ERROR))
            .await
            .0,
        404
    );
    let invalid =
        json!({ "commandId": id(), "changes": [{ "path": "/channels/ch1/fader", "value": 99 }] });
    assert_eq!(
        rest.call("PATCH", "/state", Some(invalid), Some(ERROR))
            .await
            .0,
        400
    );
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    let t = a.command("touch", "/channels/ch2/fader", None).await;
    a.answer(&t).await;
    let owned =
        json!({ "commandId": id(), "changes": [{ "path": "/channels/ch2/fader", "value": 0 }] });
    let (s, e) = rest.call("PATCH", "/state", Some(owned), Some(ERROR)).await;
    assert_eq!(
        (s, &e["conflicts"][0]["path"]),
        (409, &json!("/channels/ch2/fader"))
    );
    let scene_result = "/components/responses/SceneResult/content/application~1json/schema";
    let (s, created) = rest
        .call(
            "POST",
            "/scenes",
            Some(json!({ "commandId": id(), "name": "Opening" })),
            Some(scene_result),
        )
        .await;
    assert_eq!(s, 201);
    let scene = created["scene"]["id"].as_str().unwrap().to_owned();
    let (s, _) = rest
        .call(
            "PUT",
            &format!("/scenes/{scene}"),
            Some(json!({ "commandId": id(), "name": "Open" })),
            Some(scene_result),
        )
        .await;
    assert_eq!(s, 200);
    let recall = json!({ "commandId": id(), "crossfadeMs": 0 });
    assert_eq!(
        rest.call(
            "POST",
            &format!("/scenes/{scene}/recall"),
            Some(recall),
            Some(RESULT)
        )
        .await
        .0,
        200
    );
    assert_eq!(
        rest.call("DELETE", &format!("/scenes/{scene}"), None, Some(RESULT))
            .await
            .0,
        200
    );
    assert_eq!(
        rest.call("DELETE", &format!("/scenes/{scene}"), None, Some(ERROR))
            .await
            .0,
        404
    );
    let tally = json!({ "source": "CAM1", "state": "program" });
    assert_eq!(
        rest.call("POST", "/afv/tally", Some(tally), Some(RESULT))
            .await
            .0,
        200
    );
    let tally = json!({ "sources": { "CAM2": "preview" } });
    assert_eq!(
        rest.call("PUT", "/afv/tally", Some(tally), Some(RESULT))
            .await
            .0,
        200
    );
    assert_eq!(
        rest.call(
            "POST",
            "/loudness/reset",
            Some(json!({ "ids": ["pgm"] })),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        rest.call(
            "POST",
            "/loudness/reset",
            Some(json!({ "ids": ["ch1"] })),
            Some(ERROR)
        )
        .await
        .0,
        404
    );
    server.stop().await;
}

/// A replica that cannot order commands answers them not applied, with
/// the command, so the client sends it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_that_cannot_be_ordered_is_not_applied() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    server.app.replica.shutdown().await;
    let id = a
        .command("set", "/channels/ch1/fader", Some(json!(-3)))
        .await;
    let e = a.answer(&id).await;
    assert_eq!(
        (e["type"].as_str(), e["code"].as_u64()),
        (Some("error"), Some(503))
    );
    assert_eq!(e["path"], "/channels/ch1/fader");
    server.stop().await;
}

/// A viewer's command is refused before it is ordered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_cannot_write() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let dir = data_dir();
    let auth = json!({ "mode": "proxy", "proxy": { "groupRoles": { "watchers": "viewer" } } });
    let server = start_with(&dir, auth).await;
    let url = format!("ws://{}/api/v1/ws?session={SESSION_A}", server.api);
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("X-Forwarded-User", "vera".parse().unwrap());
    request
        .headers_mut()
        .insert("X-Forwarded-Groups", "watchers".parse().unwrap());
    // In proxy mode an upgrade without an allowed Origin is refused.
    request
        .headers_mut()
        .insert("Origin", "http://faderz.test".parse().unwrap());
    let (ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let mut v = Client {
        ws,
        session: SESSION_A.into(),
        seq: 0,
    };
    assert_eq!(v.hello().await["you"]["principal"], "vera");
    let presence = v.next_of("presence").await;
    assert_eq!(
        (
            &presence["sessions"][0]["user"],
            &presence["sessions"][0]["role"]
        ),
        (&json!("vera"), &json!("viewer")),
        "the proxy's user by name, as a viewer"
    );
    let id = v
        .command("set", "/channels/ch1/fader", Some(json!(-3)))
        .await;
    let e = v.answer(&id).await;
    assert_eq!(
        (e["code"].as_u64(), e["message"].as_str()),
        (Some(400), Some("operator role required"))
    );
    v.send(console("xt-1", 0)).await;
    let e = v.next_of("error").await;
    assert_eq!(
        (e["code"].as_u64(), e["message"].as_str()),
        (Some(400), Some("operator role required"))
    );
    assert!(e.get("commandId").is_none(), "{e}");
    server.stop().await;
}

/// The session route tells a client how to sign in, and in oidc mode
/// nothing else is served without a sign-in. The provider is read on first
/// use, so faderz starts without reaching it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oidc_mode_answers_the_session_route_and_wants_a_sign_in() {
    let dir = data_dir();
    let auth = json!({ "mode": "oidc", "oidc": {
        "issuerUrl": "http://127.0.0.1:9/realms/r",
        "clientId": "faderz",
        "publicUrl": "https://faderz.test",
    } });
    let server = start_with(&dir, auth).await;
    let rest = Rest::new(&server);
    let session = "/paths/~1session/get/responses/200/content/application~1json/schema";
    let (s, v) = rest.call("GET", "/session", None, Some(session)).await;
    assert_eq!(s, 200);
    assert_eq!(v, json!({ "mode": "oidc", "user": null, "role": null }));
    assert_eq!(rest.call("GET", "/state", None, Some(ERROR)).await.0, 401);
    let logout = "/paths/~1logout/post/responses/200/content/application~1json/schema";
    let (s, v) = rest.call("POST", "/logout", None, Some(logout)).await;
    assert_eq!((s, v), (200, json!({ "redirect": null })));
    server.stop().await;
}

const SESSION_C: &str = "session-c-0123456789abcdef";

/// A selection reaches every connection, the selecting one's included; one
/// of an element the shape lacks changes nothing; a connection opened later
/// gets the selections after its snapshot, and a selection goes with its
/// session's last connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selections_reach_every_connection() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    assert_eq!(
        a.next_of("selections").await["selections"],
        json!([]),
        "after the snapshot"
    );
    let mut b = Client::connect(&server, SESSION_B).await;
    b.hello().await;
    b.next_of("selections").await;
    a.send(json!({ "type": "select", "element": "nope" })).await;
    a.send(json!({ "type": "select", "element": "ch2" })).await;
    let seen = b.next_of("selections").await;
    let list = seen["selections"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{seen}");
    assert_eq!(list[0]["element"], "ch2");
    assert_eq!(
        list[0]["session"]["handle"],
        samplez_service::OwnerHandle::of(SESSION_A).as_str()
    );
    let mut c = Client::connect(&server, SESSION_C).await;
    c.hello().await;
    assert_eq!(
        c.next_of("selections").await["selections"][0]["element"],
        "ch2"
    );
    a.send(json!({ "type": "select", "element": null })).await;
    assert_eq!(b.next_of("selections").await["selections"], json!([]));
    // A selection goes with the last connection of its session, at once
    // rather than with the lease a second or more later.
    let mut a2 = Client::connect(&server, SESSION_A).await;
    a2.hello_as("standby").await;
    let a2 = keep_open(a2);
    a.send(json!({ "type": "select", "element": "ch1" })).await;
    assert_eq!(
        b.next_of("selections").await["selections"][0]["element"],
        "ch1"
    );
    drop(a);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut d = Client::connect(&server, "session-d-0123456789abcdef").await;
    d.hello().await;
    assert_eq!(
        d.next_of("selections").await["selections"][0]["element"],
        "ch1",
        "a's standby connection is still open"
    );
    a2.abort();
    let gone = tokio::time::timeout(Duration::from_millis(900), b.next_of("selections"))
        .await
        .expect("gone with the last connection");
    assert_eq!(gone["selections"], json!([]));
    server.stop().await;
}

/// A request for a control another session holds reaches that session's
/// primary connections only, a standby once promoted, and a repeat within
/// 3 s is dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_control_request_reaches_the_holders_primary_connections() {
    let dir = data_dir();
    let server = start(&dir).await;
    let path = "/channels/ch1/fader";
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    let mut standby = Client::connect(&server, SESSION_A).await;
    let snapshot = standby.hello_as("standby").await;
    let t = a.command("touch", path, None).await;
    a.answer(&t).await;
    let mut b = Client::connect(&server, SESSION_B).await;
    b.hello().await;
    b.send(json!({ "type": "request", "path": path })).await;
    let r = a.next_of("controlRequest").await;
    assert_eq!(r["path"], path);
    assert_eq!(
        r["from"]["handle"],
        samplez_service::OwnerHandle::of(SESSION_B).as_str()
    );
    // The standby got the touch's delta but no request.
    let delta = standby.next_of("delta").await;
    assert!(delta["owners"].is_array(), "{delta}");
    b.send(json!({ "type": "request", "path": path })).await;
    let epoch = snapshot["epoch"].clone();
    standby
        .send(json!({ "type": "promote", "lastRevision": 1, "epoch": epoch }))
        .await;
    let mut c = Client::connect(&server, SESSION_C).await;
    c.hello().await;
    c.send(json!({ "type": "request", "path": path })).await;
    let promoted = standby.next_of("controlRequest").await;
    assert_eq!(
        promoted["from"]["handle"],
        samplez_service::OwnerHandle::of(SESSION_C).as_str(),
        "b's repeat was dropped"
    );
    assert_eq!(
        a.next_of("controlRequest").await["from"]["handle"],
        samplez_service::OwnerHandle::of(SESSION_C).as_str(),
        "the primary connection got c's request as well"
    );
    // A request of the holder itself, and one for a control nobody holds,
    // go nowhere, and c's repeat within 3 s is dropped; the one after them
    // goes out.
    a.send(json!({ "type": "request", "path": path })).await;
    b.send(json!({ "type": "request", "path": "/channels/ch2/fader" }))
        .await;
    c.send(json!({ "type": "request", "path": path })).await;
    let wait = Duration::from_millis(3100);
    let (got, _, _) = tokio::join!(a.during(wait), b.during(wait), c.during(wait));
    let requests: Vec<&Value> = got
        .iter()
        .filter(|m| m["type"] == "controlRequest")
        .collect();
    assert!(requests.is_empty(), "{requests:?}");
    c.send(json!({ "type": "request", "path": path })).await;
    assert_eq!(
        a.next_of("controlRequest").await["from"]["handle"],
        samplez_service::OwnerHandle::of(SESSION_C).as_str()
    );
    server.stop().await;
}

/// The auth and browser checks answer in the API's Error body.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refusals_of_the_auth_checks_carry_the_error_body() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = data_dir();
    let server = start(&dir).await;
    let rest = Rest::new(&server);
    let mut tcp = TcpStream::connect(server.api).await.unwrap();
    let body = r#"{"commandId":"0b6b2c2e-7d1f-4a3c-9a55-0f5c1d8e2b41","changes":[]}"#;
    let request = format!(
        "PATCH /api/v1/state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    tcp.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    tcp.read_to_string(&mut answer).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    let text = answer.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    let value: Value = serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {answer}"));
    rest.check(ERROR, &value);
    assert_eq!(value["code"], 403);
    server.stop().await;
}

/// Keeps `client` open, answering pings, until the task is aborted, which
/// closes it.
fn keep_open(mut client: Client) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            client.during(Duration::from_secs(1)).await;
        }
    })
}

/// A selection made right after the hello stays, though its session has
/// no lease yet: samplez renews the leases of live sessions once a second.
/// Four sessions select a quarter of a second apart, so some select before
/// their first renewal whatever its phase.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_selection_made_right_after_the_hello_stays() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut clients = Vec::new();
    for (session, element) in [
        (SESSION_A, "ch1"),
        (SESSION_B, "ch2"),
        (SESSION_C, "pgm"),
        ("session-d-0123456789abcdef", "mm1"),
    ] {
        let mut c = Client::connect(&server, session).await;
        c.hello().await;
        c.send(json!({ "type": "select", "element": element }))
            .await;
        clients.push(keep_open(c));
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    tokio::time::sleep(Duration::from_millis(1250)).await;
    let mut late = Client::connect(&server, "session-e-0123456789abcdef").await;
    late.hello().await;
    let seen = late.next_of("selections").await;
    let mut elements: Vec<&str> = seen["selections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["element"].as_str().unwrap())
        .collect();
    elements.sort_unstable();
    assert_eq!(elements, ["ch1", "ch2", "mm1", "pgm"], "{seen}");
    clients.iter().for_each(|c| c.abort());
    server.stop().await;
}

/// The sessions of `presence`, as (handle, kind, user, role).
fn sessions(presence: &Value) -> Vec<(String, String, String, String)> {
    presence["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let f = |k: &str| s[k].as_str().unwrap().to_owned();
            (f("handle"), f("kind"), f("user"), f("role"))
        })
        .collect()
}

/// Every session connected to the replica is in `presence` once, the
/// receiving one included, however many connections it holds; it leaves
/// with its last connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_lists_each_connected_session_once() {
    let dir = data_dir();
    let server = start(&dir).await;
    let handle = |s: &str| samplez_service::OwnerHandle::of(s).as_str().to_owned();
    let a_entry = (
        handle(SESSION_A),
        "ui".into(),
        "anonymous".into(),
        "operator".into(),
    );
    let b_entry = (
        handle(SESSION_B),
        "ui".into(),
        "anonymous".into(),
        "operator".into(),
    );

    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    let first = a.next_of("presence").await;
    assert_eq!(
        sessions(&first),
        vec![a_entry.clone()],
        "the receiver itself"
    );
    assert_eq!(
        first["sessions"][0]["since"],
        first["sessions"][0]["active"]
    );

    let mut b = Client::connect(&server, SESSION_B).await;
    b.hello().await;
    let mut b2 = Client::connect(&server, SESSION_B).await;
    b2.hello_as("standby").await;
    let with_b = loop {
        let p = a.next_of("presence").await;
        if p["sessions"].as_array().unwrap().len() == 2 {
            break p;
        }
    };
    assert_eq!(
        sessions(&with_b),
        vec![a_entry.clone(), b_entry],
        "b once, in order of connecting"
    );
    let on_standby = b2.next_of("presence").await;
    assert_eq!(
        on_standby["sessions"].as_array().unwrap().len(),
        2,
        "a standby gets it too"
    );

    drop(b);
    let quiet = a.during(Duration::from_millis(500)).await;
    let still: Vec<_> = quiet.iter().filter(|m| m["type"] == "presence").collect();
    assert!(
        still
            .iter()
            .all(|p| p["sessions"].as_array().unwrap().len() == 2),
        "b keeps its standby connection: {still:?}"
    );
    drop(b2);
    let gone = loop {
        let p = a.next_of("presence").await;
        if p["sessions"].as_array().unwrap().len() == 1 {
            break p;
        }
    };
    assert_eq!(sessions(&gone), vec![a_entry]);
    server.stop().await;
}

/// A command makes its session active in `presence`; a heartbeat does
/// not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_marks_its_session_active() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    let first = a.next_of("presence").await;
    let since = first["sessions"][0]["since"].clone();
    // Activity goes out to the second: a second passes first.
    let quiet = a.during(Duration::from_millis(1100)).await;
    assert!(
        quiet.iter().all(|m| m["type"] != "presence"),
        "heartbeats are no activity: {quiet:?}"
    );
    a.command("set", "/channels/ch1/fader", Some(json!(-3)))
        .await;
    // The presence may go out ahead of the delta, so nothing is skipped.
    let got = a.during(Duration::from_millis(1000)).await;
    let p = got
        .iter()
        .find(|m| m["type"] == "presence")
        .unwrap_or_else(|| panic!("no presence after the command: {got:?}"));
    assert_eq!(p["sessions"][0]["since"], since);
    assert!(
        p["sessions"][0]["active"].as_str().unwrap() > since.as_str().unwrap(),
        "{p}"
    );
    server.stop().await;
}

/// In token mode a session without a name is listed as token.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_names_a_token_session_token() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let dir = data_dir();
    let token = dir.0.with_extension("token");
    std::fs::write(&token, "a-token-of-at-least-thirty-two-bytes-0123").unwrap();
    let auth = json!({ "mode": "token", "token": { "file": token } });
    let server = start_with(&dir, auth).await;
    let url = format!("ws://{}/api/v1/ws?session={SESSION_A}", server.api);
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Authorization",
        "Bearer a-token-of-at-least-thirty-two-bytes-0123"
            .parse()
            .unwrap(),
    );
    request
        .headers_mut()
        .insert("Origin", "http://faderz.test".parse().unwrap());
    let (ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let mut a = Client {
        ws,
        session: SESSION_A.into(),
        seq: 0,
    };
    a.hello().await;
    let p = a.next_of("presence").await;
    assert_eq!(p["sessions"][0]["user"], "token");
    let _ = std::fs::remove_file(token);
    server.stop().await;
}

/// A `console` message for console `id` at bank `bank`.
fn console(id: &str, bank: u64) -> Value {
    json!({
        "type": "console", "id": id, "name": "Bridge name", "protocol": "mcu",
        "transport": "bridge", "strips": 8, "bankOffset": bank,
    })
}

/// A registration reaches every client as one change of /consoles, with
/// the registering session's handle; another session is refused while it
/// is connected; the same registration again changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_console_message_registers_a_console_of_its_session() {
    let dir = data_dir();
    let declared = json!([{ "id": "xt-1", "name": "X-Touch" }]);
    let server = start_configured(&dir, json!({ "mode": "none" }), declared).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    let snapshot = a.hello().await;
    let status = |handle: Value, connected: bool, bank: u64| {
        json!([{
            "id": "xt-1", "name": "X-Touch", "handle": handle, "protocol": "mcu",
            "transport": "bridge", "connected": connected, "bankOffset": bank, "strips": 8,
        }])
    };
    assert_eq!(snapshot["state"]["consoles"], status(Value::Null, false, 0));
    let mut b = Client::connect(&server, SESSION_B).await;
    b.hello().await;
    a.send(console("xt-1", 8)).await;
    let registered = status(snapshot["you"]["handle"].clone(), true, 8);
    for c in [&mut a, &mut b] {
        let delta = c.next_of("delta").await;
        assert_eq!(
            delta["changes"],
            json!([{ "path": "/consoles", "value": registered }])
        );
        assert!(delta.get("origins").is_none(), "{delta}");
    }
    b.send(console("xt-1", 0)).await;
    let e = b.next_of("error").await;
    assert_eq!(e["code"], 409, "{e}");
    assert!(e.get("commandId").is_none(), "{e}");
    // Unchanged: the next message is the answer to the set after it.
    a.send(console("xt-1", 8)).await;
    let id = a
        .command("set", "/channels/ch1/fader", Some(json!(-3)))
        .await;
    let next = a.next().await;
    assert_eq!(next["origins"][0]["commandId"], id, "{next}");
    let (_, state) = Rest::new(&server)
        .call(
            "GET",
            "/state",
            None,
            Some("/paths/~1state/get/responses/200/content/application~1json/schema"),
        )
        .await;
    assert_eq!(state["state"]["consoles"], registered);
    server.stop().await;
}

/// More than 20 console messages within a second on one connection are
/// refused with 429.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_messages_are_rate_limited() {
    let dir = data_dir();
    let server = start(&dir).await;
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    for _ in 0..25 {
        a.send(console("xt-1", 0)).await;
    }
    let errors: Vec<Value> = a
        .during(Duration::from_millis(500))
        .await
        .into_iter()
        .filter(|m| m["type"] == "error")
        .collect();
    assert!(!errors.is_empty());
    assert!(errors.iter().all(|e| e["code"] == 429), "{errors:?}");
    server.stop().await;
}

/// REST creates, updates and removes consoles; a declared one is not
/// removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rest_puts_and_deletes_consoles() {
    let dir = data_dir();
    let declared = json!([{ "id": "xt-1", "name": "X-Touch" }]);
    let server = start_configured(&dir, json!({ "mode": "none" }), declared).await;
    let rest = Rest::new(&server);
    let status = "/components/responses/Console/content/application~1json/schema";
    let (s, created) = rest
        .call(
            "PUT",
            "/consoles/desk",
            Some(json!({ "name": "Desk" })),
            Some(status),
        )
        .await;
    assert_eq!(s, 201);
    assert_eq!(
        created,
        json!({
            "id": "desk", "name": "Desk", "handle": null, "protocol": "mcu",
            "transport": "bridge", "connected": false, "bankOffset": 0, "strips": 8,
        })
    );
    let (s, updated) = rest
        .call(
            "PUT",
            "/consoles/desk",
            Some(json!({ "name": "Desk 2", "protocol": "osc", "strips": 16 })),
            Some(status),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        (
            &updated["name"],
            &updated["protocol"],
            &updated["transport"],
            &updated["strips"]
        ),
        (
            &json!("Desk 2"),
            &json!("osc"),
            &json!("bridge"),
            &json!(16)
        )
    );
    let list = "/paths/~1consoles/get/responses/200/content/application~1json/schema";
    let (_, all) = rest.call("GET", "/consoles", None, Some(list)).await;
    assert_eq!(all[0]["id"], "xt-1");
    assert_eq!(all[1], updated);
    let bad = Some(json!({ "name": "" }));
    assert_eq!(
        rest.call("PUT", "/consoles/desk", bad, Some(ERROR)).await.0,
        400
    );
    let bad = Some(json!({ "name": "A", "strips": 0 }));
    assert_eq!(
        rest.call("PUT", "/consoles/desk", bad, Some(ERROR)).await.0,
        400
    );
    let bad = Some(json!({ "name": "A" }));
    assert_eq!(
        rest.call("PUT", "/consoles/Desk", bad, Some(ERROR)).await.0,
        400
    );
    assert_eq!(
        rest.call("DELETE", "/consoles/desk", None, None).await.0,
        204
    );
    assert_eq!(
        rest.call("DELETE", "/consoles/desk", None, Some(ERROR))
            .await
            .0,
        404
    );
    let (s, e) = rest
        .call("DELETE", "/consoles/xt-1", None, Some(ERROR))
        .await;
    assert_eq!(s, 409);
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("declared in the configuration"),
        "{e}"
    );
    // A console connected to a session is not removed: its next
    // registration would bring it back.
    let mut a = Client::connect(&server, SESSION_A).await;
    a.hello().await;
    a.send(console("live", 0)).await;
    let delta = a.next_of("delta").await;
    assert_eq!(delta["changes"][0]["path"], "/consoles", "{delta}");
    let (s, e) = rest
        .call("DELETE", "/consoles/live", None, Some(ERROR))
        .await;
    assert_eq!(
        (s, e["message"].as_str()),
        (409, Some("console live is connected"))
    );
    let (_, all) = rest.call("GET", "/consoles", None, Some(list)).await;
    assert!(
        all.as_array().unwrap().iter().any(|c| c["id"] == "live"),
        "{all}"
    );
    server.stop().await;
}

/// Every status the console routes answer with is declared for them.
#[test]
fn the_console_routes_declare_their_statuses() {
    let openapi: Value = serde_yaml::from_str(include_str!("../api/openapi.yaml")).unwrap();
    let declared = |method: &str| -> Vec<String> {
        let op = &openapi["paths"]["/consoles/{consoleId}"][method]["responses"];
        op.as_object().unwrap().keys().cloned().collect()
    };
    let put = declared("put");
    for status in ["200", "201", "400", "403", "404", "503"] {
        assert!(put.contains(&status.to_owned()), "PUT {status}: {put:?}");
    }
    let delete = declared("delete");
    for status in ["204", "403", "404", "409", "503"] {
        assert!(
            delete.contains(&status.to_owned()),
            "DELETE {status}: {delete:?}"
        );
    }
}
