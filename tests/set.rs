//! Three faderz replicas of one set on loopback, reaching each other on
//! their peer listeners with mutual TLS, starting from a marker held in
//! memory: a write sent to any replica reaches the clients of every
//! other.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use faderz::config::Config;
use faderz::server::{Server, SetParts};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use samplez_replica::bootstrap::{Marker, Markers};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const REPLICAS: usize = 3;

/// The marker ConfigMap, in memory.
#[derive(Default)]
struct Memory(Mutex<Option<Marker>>);

impl Markers for Memory {
    fn name(&self) -> &str {
        "memory"
    }

    fn get(&self) -> BoxFuture<'_, Result<Option<Marker>, String>> {
        let marker = *self.0.lock().unwrap();
        Box::pin(async move { Ok(marker) })
    }

    fn create(&self, marker: Marker) -> BoxFuture<'_, Result<bool, String>> {
        let mut held = self.0.lock().unwrap();
        let created = held.is_none();
        if created {
            *held = Some(marker);
        }
        Box::pin(async move { Ok(created) })
    }
}

/// A directory under the target directory, removed when dropped.
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(name: &str) -> Dir {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("faderz-set-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Dir(dir)
}

/// A CA, and per replica a certificate it issued for `localhost`: the
/// `replica.peer.tls` of each.
fn certificates(at: &Path) -> Vec<Value> {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
    let ca_file = at.join("ca.crt");
    std::fs::write(&ca_file, ca.pem()).unwrap();
    (0..REPLICAS)
        .map(|n| {
            let key = KeyPair::generate().unwrap();
            let cert = CertificateParams::new(vec!["localhost".into()])
                .unwrap()
                .signed_by(&key, &ca)
                .unwrap();
            let (cert_file, key_file) = (at.join(format!("{n}.crt")), at.join(format!("{n}.key")));
            std::fs::write(&cert_file, cert.pem()).unwrap();
            std::fs::write(&key_file, key.serialize_pem()).unwrap();
            json!({ "mode": "files", "certFile": cert_file, "keyFile": key_file,
                    "clientCaFile": ca_file })
        })
        .collect()
}

fn shape() -> Value {
    json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [{ "id": "ch1", "name": "Pres 1", "width": 1, "layout": "mono", "altInput": false }],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [], "outputs": [],
    })
}

async fn start_set(at: &Path, markers: Arc<Memory>) -> Vec<Server> {
    let tls = certificates(at);
    // Bound before any replica starts, and handed to it: no other test
    // can take the port between.
    let listeners: Vec<std::net::TcpListener> = (0..REPLICAS)
        .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    let ports: Vec<u16> = (listeners.iter())
        .map(|l| l.local_addr().unwrap().port())
        .collect();
    let peers: Vec<String> = ports.iter().map(|p| format!("localhost:{p}")).collect();
    let listener = json!({ "bind": "127.0.0.1", "port": 0 });
    let mut servers = Vec::new();
    for (n, listener_n) in listeners.into_iter().enumerate() {
        let config = json!({
            "listeners": { "api": listener, "ops": listener, "nmos": listener },
            "auth": { "mode": "none" },
            "api": { "allowedOrigins": ["http://faderz.test"] },
            "replica": {
                "name": format!("faderz-{n}"),
                "ordinal": n,
                "dataDir": at.join(format!("data-{n}")),
                "replicas": REPLICAS,
                "peers": peers,
                "peer": { "bind": "127.0.0.1", "port": ports[n], "tls": tls[n] },
                "marker": "memory",
            },
            "shape": shape(),
        });
        let (config, shape, name) = Config::parse(&config.to_string(), || unreachable!()).unwrap();
        let parts = SetParts {
            markers: markers.clone(),
            listener: listener_n,
            // Long, for a loaded test host.
            write_timeout: Duration::from_secs(10),
        };
        servers.push(
            Server::start_with(&config, shape, name, None, Some(parts))
                .await
                .unwrap(),
        );
    }
    servers
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The status line of `route` on the replica `server`'s ops listener,
/// and the replica its body reports.
async fn ops(server: &Server, route: &str) -> (u16, Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut tcp = TcpStream::connect(server.ops).await.unwrap();
    let request = format!("GET {route} HTTP/1.1\r\nHost: faderz\r\nConnection: close\r\n\r\n");
    tcp.write_all(request.as_bytes()).await.unwrap();
    let mut text = String::new();
    tcp.read_to_string(&mut text).await.unwrap();
    let status = text.split(' ').nth(1).expect("a status").parse().unwrap();
    let body = text.split_once("\r\n\r\n").expect("a body").1;
    (
        status,
        serde_json::from_str::<Value>(body).expect("JSON")["replica"].clone(),
    )
}

async fn replica_info(server: &Server) -> Value {
    ops(server, "/healthz").await.1
}

/// Waits until every replica of `servers` is a voter a leader counts and
/// answers `/readyz` with 200: a set runs no engine, so nothing else holds
/// it back.
async fn formed(servers: &[Server]) {
    let all = async {
        loop {
            let mut commitable = 0;
            for s in servers {
                let (ready, info) = ops(s, "/readyz").await;
                commitable += usize::from(ready == 200 && info["commitable"] == true);
            }
            if commitable == servers.len() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    if tokio::time::timeout(Duration::from_secs(30), all)
        .await
        .is_err()
    {
        let mut infos = Vec::new();
        for s in servers {
            infos.push(replica_info(s).await);
        }
        panic!("the set did not form within 30 s: {infos:?}");
    }
}

/// A client of `server` past its hello.
async fn client(server: &Server, session: &str) -> Ws {
    let url = format!("ws://{}/api/v1/ws?session={session}", server.api);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let hello = json!({ "type": "hello", "session": session, "kind": "ui", "mode": "primary" });
    ws.send(Message::text(hello.to_string())).await.unwrap();
    ws
}

/// The next message of `kind`, any but a heartbeat for `""`; pings
/// answered.
async fn next_of(ws: &mut Ws, kind: &str) -> Value {
    loop {
        let message = ws.next().await.expect("open").unwrap();
        let Message::Text(text) = message else {
            assert!(!message.is_close(), "closed: {message:?}");
            continue;
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        if value["type"] == "ping" {
            let pong = json!({ "type": "pong", "t": value["t"] });
            ws.send(Message::text(pong.to_string())).await.unwrap();
        }
        let heartbeat = value["type"] == "ping" || value["type"] == "pong";
        if value["type"] == kind || (kind.is_empty() && !heartbeat) {
            return value;
        }
    }
}

/// What answered command `id`: an error, an ack, or the delta naming it
/// among its origins.
async fn answer(ws: &mut Ws, id: &str) -> Value {
    loop {
        let m = next_of(ws, "").await;
        let origins = m["origins"].as_array().into_iter().flatten();
        if m["commandId"] == id || origins.into_iter().any(|o| o["commandId"] == id) {
            return m;
        }
    }
}

/// The set elects a leader and takes the other two in as voters; a fader
/// set on one follower reaches a client of the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_to_one_replica_reaches_the_clients_of_another() {
    let at = dir("write");
    let servers = start_set(&at.0, Arc::default()).await;
    formed(&servers).await;
    let path = "/channels/ch1/fader";
    // Read throughout, so its pings are answered.
    let mut reader = client(&servers[2], "session-r-0123456789abcdef").await;
    let seen = tokio::spawn(async move {
        loop {
            let m = next_of(&mut reader, "delta").await;
            let changes = m["changes"].as_array().cloned().unwrap_or_default();
            if changes
                .iter()
                .any(|c| c["path"] == path && c["value"] == -6)
            {
                return;
            }
        }
    });
    let mut writer = client(&servers[1], "session-w-0123456789abcdef").await;
    // A leader change while it is ordered answers it not applied: sent
    // again.
    let mut answered = Value::Null;
    for seq in 1..=5 {
        let id = faderz::app::uuid().unwrap();
        let set = json!({ "type": "set", "id": id, "seq": seq, "path": path, "value": -6 });
        writer.send(Message::text(set.to_string())).await.unwrap();
        answered = tokio::time::timeout(Duration::from_secs(15), answer(&mut writer, &id))
            .await
            .expect("an answer within 15 s");
        if answered["type"] == "delta" {
            break;
        }
    }
    assert_eq!(answered["type"], "delta", "{answered}");
    let seen = tokio::time::timeout(Duration::from_secs(10), seen).await;
    assert!(seen.is_ok(), "the other replica's client saw no delta");
    for server in servers {
        server.stop().await;
    }
}
