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
use faderz::state::{Activation, PortRef, tai_text};
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
    // And the activation that made them, not none until a port changes.
    for port in [&ch1, &pgm] {
        let activation = &active(&server, port).await["activation"];
        assert_eq!(activation["mode"], "activate_immediate", "{port}");
        assert!(activation["activation_time"].is_string(), "{port}");
    }
    // The engine starts as the state left it, from its first status on:
    // ch1 detached with no reader opened, the disabled output with no
    // flow created, not even for a moment.
    assert_eq!(input(&server, 0).0, InputState::Detached, "ch1 at start");
    for _ in 0..40 {
        assert!(!flow.exists(), "the disabled output's flow was created");
        assert_eq!(input(&server, 0).0, InputState::Detached);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    until("ch2 reads input b", async || input(&server, 1).1 == f.ch2b).await;

    // A scheduled activation cancelled before its time never fires, and
    // the state holds it no longer.
    let staged = json!({
        "transport_params": [{ "mxl_flow_id": f.ch1 }],
        "activation": { "mode": "activate_scheduled_relative", "requested_time": "1:0" },
    });
    let path = format!("{CONNECTION}/{ch1}/staged");
    let (status, body) = request(server.nmos, "PATCH", &path, Some(&staged)).await;
    assert_eq!(status, 202, "{body}");
    until("the schedule is state", async || scheduled(&server) == 1).await;
    // Until it fires, the port shows the activation it has.
    let shown = active(&server, &ch1).await;
    assert_eq!(shown["activation"]["mode"], "activate_immediate", "{shown}");
    let cancel = json!({ "activation": { "mode": null } });
    let (status, body) = request(server.nmos, "PATCH", &path, Some(&cancel)).await;
    assert_eq!(status, 200, "{body}");
    until("the cancel is state", async || scheduled(&server) == 0).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(input(&server, 0).0, InputState::Detached);
    assert_eq!(scheduled(&server), 0, "the cancel left a schedule behind");

    // A channel detached in the state is not heard after a restart: ch1,
    // fader up and routed to the programme bus, the one channel open, the
    // output written from the start peaks at nothing.
    patch(
        &server,
        &[
            ("/channels/ch1/fader", json!(0)),
            ("/channels/ch1/routes/pgm", json!(true)),
        ],
    )
    .await;
    activate(&server, &pgm, json!({ "master_enable": true })).await;
    until("the output writes its flow", async || flow.exists()).await;
    server.stop().await;
    let server = start(&data.0, &domain.0, &f, at).await;
    until("ch1 is detached after the restart", async || {
        input(&server, 0).0 == InputState::Detached
    })
    .await;
    until("the output writes its flow", async || flow.exists()).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let peak = || {
        let status = server.audio.as_ref().unwrap().engine().status();
        status.outputs[0].meters.max_true_peak.clone()
    };
    assert!(
        peak().iter().all(|&p| p == 0.0),
        "ch1 was heard: {:?}",
        peak()
    );
    // Attached, it is heard: the meter sees the output.
    activate(&server, &ch1, json!({ "master_enable": true })).await;
    until("ch1 is heard", async || peak().iter().any(|&p| p > 0.0)).await;
    server.stop().await;
}

/// A scheduled activation is taken into the state with the mode and
/// requested time it was staged with, through `/staged` and through
/// `/bulk`, and fired with them: wherever `/active` is written from the
/// state, a relative activation shows as relative.
#[tokio::test(flavor = "multi_thread")]
async fn a_scheduled_activation_keeps_the_mode_and_requested_time_it_was_staged_with() {
    let data = dir(Path::new(env!("CARGO_TARGET_TMPDIR")), "nmos-asked-data");
    let domain = domain("nmos-asked");
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
    let (_registry, at) = registry::start();
    let server = start(&data.0, &domain.0, &f, at).await;
    let ch1 = id_of(&server, "receivers", "Pres 1").await;
    let ch2 = id_of(&server, "receivers", "Pres 2").await;
    let asked = |requested: &str, at: u64| Activation {
        mode: "activate_scheduled_relative".into(),
        requested: Some(requested.into()),
        time: tai_text(at),
    };
    let fired = |channel: &str| {
        (server.app.replica.store()).with_state(|sm| {
            (sm.product.receiver(channel))
                .and_then(|(c, _)| c.cloned())
                .and_then(|c| c.activation)
        })
    };
    let params = |requested: &str| {
        json!({
            "transport_params": [{ "mxl_flow_id": other }],
            "activation": { "mode": "activate_scheduled_relative", "requested_time": requested },
        })
    };

    let path = format!("{CONNECTION}/receivers/{ch1}/staged");
    let (status, body) = request(server.nmos, "PATCH", &path, Some(&params("0:300000000"))).await;
    assert_eq!(status, 202, "{body}");
    until("the schedule is state", async || scheduled(&server) == 1).await;
    let s = (server.app.replica.store()).with_state(|sm| sm.product.scheduled()[0].clone());
    assert_eq!(s.active.activation, Some(asked("0:300000000", s.at)));
    until("the schedule fired", async || scheduled(&server) == 0).await;
    assert_eq!(fired("ch1"), Some(asked("0:300000000", s.at)));

    let bulk = json!([{ "id": ch2, "params": params("0:400000000") }]);
    let path = "/x-nmos/connection/v1.2/bulk/receivers";
    let (status, body) = request(server.nmos, "POST", path, Some(&bulk)).await;
    assert_eq!(status, 200, "{body}");
    until("the bulk schedule is state", async || {
        scheduled(&server) == 1
    })
    .await;
    let s = (server.app.replica.store()).with_state(|sm| sm.product.scheduled()[0].clone());
    assert_eq!(s.active.activation, Some(asked("0:400000000", s.at)));
    until("the bulk schedule fired", async || scheduled(&server) == 0).await;
    assert_eq!(fired("ch2"), Some(asked("0:400000000", s.at)));

    // A `/bulk` cancel is judged per item: one samplez-nmos refused
    // leaves its schedule to fire, though the request answers 200.
    let back = json!({
        "transport_params": [{ "mxl_flow_id": f.ch1 }],
        "activation": { "mode": "activate_scheduled_relative", "requested_time": "60:0" },
    });
    let bulk = json!([{ "id": ch1, "params": back }, { "id": ch2, "params": back }]);
    let (status, body) = request(server.nmos, "POST", path, Some(&bulk)).await;
    assert_eq!(status, 200, "{body}");
    until("both schedules are state", async || scheduled(&server) == 2).await;
    let cancel = json!({ "activation": { "mode": null } });
    let refused = json!({ "activation": { "mode": null }, "master_enable": "yes" });
    let bulk = json!([{ "id": ch1, "params": refused }, { "id": ch2, "params": cancel }]);
    let (status, body) = request(server.nmos, "POST", path, Some(&bulk)).await;
    assert_eq!(status, 200, "{body}");
    let code = |id: &str| {
        (body.as_array().unwrap().iter())
            .find(|item| item["id"] == id)
            .map(|item| item["code"].clone())
    };
    assert_eq!(code(&ch2), Some(json!(200)), "{body}");
    assert_ne!(code(&ch1), Some(json!(200)), "{body}");
    until("ch2's cancel is state", async || scheduled(&server) == 1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let left = (server.app.replica.store()).with_state(|sm| sm.product.scheduled().to_vec());
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].port, PortRef::Receiver("ch1".into()));
    server.stop().await;
}

/// The scheduled activations the replica's state holds.
fn scheduled(server: &Server) -> usize {
    (server.app.replica.store()).with_state(|sm| sm.product.scheduled().len())
}
