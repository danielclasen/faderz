//! IS-05 over MXL against a running faderz: activations of a channel's
//! receiver route the engine, a disabled receiver detaches its channel,
//! a disabled sender removes its output flow, a switch to input b shows
//! in `/active`, and a restart keeps the resource ids, `/active` and the
//! routing.

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use faderz::config::Config;
use faderz::server::Server;
use samplez::InputState;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

mod common;
use common::*;

const DOMAIN: &str = "6f1f1d1e-56f6-4c4c-8d32-9e7f4a3b2c10";
const CONNECTION: &str = "/x-nmos/connection/v1.2/single";

fn shape() -> Value {
    json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "ch1", "name": "Pres 1", "width": 1, "layout": "mono", "altInput": false },
            { "id": "ch2", "name": "Pres 2", "width": 1, "layout": "mono", "altInput": true },
        ],
        "buses": [
            { "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" },
        ],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
        ],
    })
}

struct Flows {
    ch1: String,
    ch2: String,
    ch2b: String,
    pgm: String,
}

async fn start(data: &Path, domain: &Path, f: &Flows, registry: SocketAddr) -> Server {
    let listener = json!({ "bind": "127.0.0.1", "port": 0 });
    let config = json!({
        "listeners": { "api": listener, "ops": listener, "nmos": listener },
        "auth": { "mode": "none" },
        "replica": { "name": "r1", "dataDir": data },
        "shape": shape(),
        "mxl": {
            "libraryPath": libmxl(),
            "domains": [{ "name": "d", "path": domain }],
            "inputs": {
                "ch1": { "flow": f.ch1 },
                "ch2": { "flow": f.ch2, "altFlow": f.ch2b },
            },
            "outputs": { "out-pgm": { "flow": f.pgm } },
        },
        "rt": { "rtCpu": "none" },
        "nmos": {
            "enabled": true,
            "advertiseHost": "127.0.0.1",
            "advertisePort": 8081,
            "heartbeatS": 1,
            "registry": { "url": format!("http://{registry}/r0") },
        },
    });
    let (config, shape, name) = Config::parse(&config.to_string(), || unreachable!()).unwrap();
    Server::start(&config, shape, name, None).await.unwrap()
}

/// `method` `path` on `addr` with a JSON `body`: status and JSON body.
async fn request(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let body = body.map(Value::to_string).unwrap_or_default();
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
         X-Samplez-Request: 1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    tcp.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    tcp.read_to_string(&mut answer).await.unwrap();
    let status = answer[9..12].parse().unwrap();
    let text = answer.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, serde_json::from_str(text).unwrap_or(Value::Null))
}

/// The id of the resources of `kind` labelled `label`.
async fn id_of(server: &Server, kind: &str, label: &str) -> String {
    let (status, list) = request(
        server.nmos,
        "GET",
        &format!("/x-nmos/node/v1.3/{kind}/"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let found = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["label"] == label);
    found.unwrap_or_else(|| panic!("no {kind} {label}"))["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Activates `staged` on the port at once.
async fn activate(server: &Server, port: &str, mut staged: Value) {
    staged["activation"] = json!({ "mode": "activate_immediate" });
    let path = format!("{CONNECTION}/{port}/staged");
    let (status, body) = request(server.nmos, "PATCH", &path, Some(&staged)).await;
    assert_eq!(status, 200, "{body}");
}

async fn active(server: &Server, port: &str) -> Value {
    let (status, body) = request(
        server.nmos,
        "GET",
        &format!("{CONNECTION}/{port}/active"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    body
}

/// Waits up to 10 s for `done`.
async fn until(what: &str, mut done: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done().await {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn input(server: &Server, slot: usize) -> (InputState, String) {
    let status = server.audio.as_ref().unwrap().engine().status();
    let i = &status.inputs[slot];
    (i.state, i.flow_id.clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn is05_activations_route_the_engine_and_survive_a_restart() {
    let data = dir(Path::new(env!("CARGO_TARGET_TMPDIR")), "nmos-data");
    let domain = domain("nmos");
    std::fs::write(
        domain.0.join("domain_def.json"),
        json!({ "id": DOMAIN, "label": "test" }).to_string(),
    )
    .unwrap();
    let f = Flows {
        ch1: mxl_id(),
        ch2: mxl_id(),
        ch2b: mxl_id(),
        pgm: mxl_id(),
    };
    let other = mxl_id();
    let _sources = [
        Source::start(&domain.0, &f.ch1, vec![0.5]),
        Source::start(&domain.0, &other, vec![0.25]),
    ];
    let (registry, at) = registry::start();
    let server = start(&data.0, &domain.0, &f, at).await;
    let node = request(server.nmos, "GET", "/x-nmos/node/v1.3/self", None)
        .await
        .1;
    let ch1 = format!("receivers/{}", id_of(&server, "receivers", "Pres 1").await);
    let ch2 = format!("receivers/{}", id_of(&server, "receivers", "Pres 2").await);
    let pgm = format!("senders/{}", id_of(&server, "senders", "PGM out").await);
    let a = active(&server, &ch1).await;
    assert_eq!(a["transport_params"][0]["mxl_flow_id"], f.ch1.as_str());
    assert_eq!(a["transport_params"][0]["mxl_domain_id"], DOMAIN);

    // A receiver activation is the channel's input.
    activate(
        &server,
        &ch1,
        json!({ "transport_params": [{ "mxl_flow_id": other }] }),
    )
    .await;
    until("ch1 reads the activated flow", async || {
        input(&server, 0) == (InputState::Attached, other.clone())
    })
    .await;
    assert_eq!(
        active(&server, &ch1).await["transport_params"][0]["mxl_flow_id"],
        other.as_str()
    );

    // Of two activations in a row the later one stands: the earlier one
    // committing does not take /active back.
    let flow_of = |a: Value| {
        a["transport_params"][0]["mxl_flow_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    activate(
        &server,
        &ch1,
        json!({ "transport_params": [{ "mxl_flow_id": f.ch1 }] }),
    )
    .await;
    activate(
        &server,
        &ch1,
        json!({ "transport_params": [{ "mxl_flow_id": other }] }),
    )
    .await;
    for _ in 0..20 {
        assert_eq!(flow_of(active(&server, &ch1).await), other);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    until("ch1 reads the later flow", async || {
        input(&server, 0) == (InputState::Attached, other.clone())
    })
    .await;

    // Disabled, it reads nothing.
    activate(&server, &ch1, json!({ "master_enable": false })).await;
    until("ch1 is detached", async || {
        input(&server, 0).0 == InputState::Detached
    })
    .await;

    // A disabled sender removes its flow, an enabled one writes it again.
    let flow = domain.0.join(format!("{}.mxl-flow", f.pgm));
    until("the output writes its flow", async || flow.exists()).await;
    activate(&server, &pgm, json!({ "master_enable": false })).await;
    until("the output flow is removed", async || !flow.exists()).await;
    activate(&server, &pgm, json!({ "master_enable": true })).await;
    until("the output flow is back", async || flow.exists()).await;
    activate(&server, &pgm, json!({ "master_enable": false })).await;
    until("the output flow is removed", async || !flow.exists()).await;

    // A switch to input b shows in the receiver's /active.
    patch(&server, &[("/channels/ch2/input", json!("b"))]).await;
    until("ch2's receiver shows input b", async || {
        active(&server, &ch2).await["transport_params"][0]["mxl_flow_id"] == f.ch2b.as_str()
    })
    .await;
    until("ch2 reads input b", async || input(&server, 1).1 == f.ch2b).await;

    let shown = [
        active(&server, &ch1).await,
        active(&server, &ch2).await,
        active(&server, &pgm).await,
    ];
    // A set of one registers, and deletes its Node when it stops.
    let id = node["id"].as_str().unwrap().to_owned();
    until("the node registers", async || {
        registry.lock().unwrap().nodes() == [id.as_str()]
    })
    .await;
    server.stop().await;
    let deleted = format!("DELETE resource/nodes/{id}");
    {
        let r = registry.lock().unwrap();
        assert!(r.seen.iter().any(|s| s.request == deleted), "{:?}", r.seen);
        assert!(r.nodes().is_empty());
    }

    // The state carries the connections: a restart shows and routes them
    // under the same ids.
    let server = start(&data.0, &domain.0, &f, at).await;
    let again = request(server.nmos, "GET", "/x-nmos/node/v1.3/self", None)
        .await
        .1;
    assert_eq!(again["id"], node["id"]);
    for (port, before) in [&ch1, &ch2, &pgm].into_iter().zip(&shown) {
        let after = active(&server, port).await;
        assert_eq!(after["master_enable"], before["master_enable"], "{port}");
        assert_eq!(
            after["transport_params"], before["transport_params"],
            "{port}"
        );
    }
    until("ch1 is detached after the restart", async || {
        input(&server, 0).0 == InputState::Detached
    })
    .await;
    until("ch2 reads input b", async || input(&server, 1).1 == f.ch2b).await;
    until("the disabled output stays disabled", async || {
        !flow.exists()
    })
    .await;
    server.stop().await;
}
