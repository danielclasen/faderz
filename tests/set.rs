//! Three faderz replicas of one set on loopback, reaching each other on
//! their peer listeners with mutual TLS, starting from a marker held in
//! memory: a write sent to any replica reaches the clients of every
//! other.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use faderz::config::Config;
use faderz::server::{Server, SetParts};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

mod common;
use common::*;

const REPLICAS: usize = 3;

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
    let tls = certificates(at, REPLICAS);
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
    let at = scratch("write");
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

/// The `replicas` message of a set lists all three, by the names the
/// pods have, with the leader among them. A set without an engine has
/// raft's view alone: the leader hears a follower that has matched its
/// log, so one stopped is listed down once the log has moved on without
/// it, and the other two still hold the quorum.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_leader_lists_every_replica_and_one_stopped_as_down() {
    let at = scratch("members");
    let mut servers = start_set(&at.0, Arc::default()).await;
    formed(&servers).await;
    let names = |v: &Value| -> Vec<String> {
        (v.as_array().unwrap().iter())
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect()
    };
    // The leader is taken from a listing that names it, every replica
    // healthy: leadership can move while the set settles, and only the
    // leader lists a follower by the log it matched; a follower lists the
    // replica it follows and no other as up.
    let mut listed = Value::Null;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let leader = 'found: loop {
        for (n, server) in servers.iter().enumerate() {
            if replica_info_sync(server)["role"] != "leader" {
                continue;
            }
            listed = server.app.replicas();
            let all = listed.as_array().unwrap();
            let healthy = |r: &Value| r["healthy"] == true && r["commitable"] == true;
            let leads = |r: &Value| r["role"] == "leader" && r["id"] == format!("faderz-{n}");
            if all.iter().all(healthy) && all.iter().any(leads) {
                break 'found n;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no leader listing all healthy within 15 s: {listed}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(
        names(&listed),
        ["faderz-0", "faderz-1", "faderz-2"],
        "{listed}"
    );
    let leaders: Vec<&Value> = (listed.as_array().unwrap().iter())
        .filter(|r| r["role"] == "leader")
        .collect();
    assert_eq!(leaders.len(), 1, "{listed}");

    let gone = (leader + 1) % REPLICAS;
    servers.remove(gone).stop().await;
    let leader_now = if gone < leader { leader - 1 } else { leader };
    let name = format!("faderz-{gone}");
    // Raft's view tells a stopped follower from an idle one only once the
    // log moves on without it: writes go on until the leader lists it
    // down. The listing is read from whichever replica leads at the time,
    // and only one in which it leads counts: a follower, or a replica
    // amid an election, lists every replica but the one it follows down.
    let mut writer = client(&servers[leader_now], "session-m-0123456789abcdef").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut seq = 0;
    let mut listed = Value::Null;
    loop {
        let leading = (servers.iter().enumerate())
            .find(|(_, s)| replica_info_sync(s)["role"] == "leader")
            .map(|(n, s)| (if n >= gone { n + 1 } else { n }, s.app.replicas()));
        if let Some((n, now)) = leading {
            let leads = (now.as_array().unwrap().iter())
                .any(|r| r["id"] == format!("faderz-{n}") && r["role"] == "leader");
            // The stopped one down, the others healthy and holding the
            // quorum: what the asserts below check, so a listing amid a
            // move of leadership never reaches them.
            let settled = (now.as_array().unwrap().iter()).all(|r| {
                let up = r["id"] != name.as_str();
                r["healthy"] == up && r["commitable"] == up
            });
            listed = now;
            if leads && settled {
                break;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{name} not listed down, the others up, by a leader within 15 s: {listed}"
        );
        seq += 1;
        let id = faderz::app::uuid().unwrap();
        let set = json!({ "type": "set", "id": id, "seq": seq, "path": "/channels/ch1/fader", "value": -(seq % 60) });
        writer.send(Message::text(set.to_string())).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(2), answer(&mut writer, &id)).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(
        names(&listed),
        ["faderz-0", "faderz-1", "faderz-2"],
        "{listed}"
    );
    for r in listed.as_array().unwrap() {
        let up = r["id"] != name.as_str();
        assert_eq!(r["healthy"], up, "{listed}");
        assert_eq!(
            r["commitable"], up,
            "the quorum holds with two of three: {listed}"
        );
    }
    for server in servers {
        server.stop().await;
    }
}

fn replica_info_sync(server: &Server) -> Value {
    let info = server.app.replica.raft().metrics().borrow().clone();
    let role = if info.current_leader == Some(server.app.replica.id()) {
        "leader"
    } else {
        "follower"
    };
    json!({ "role": role })
}
