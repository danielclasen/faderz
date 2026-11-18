//! NMOS in a set of three faderz replicas with engines, in one process,
//! each on a runtime of its own: the set is one Node, which only the
//! leader registers, with a mock registry each replica reaches under a
//! path of its own; an IS-05 activation taken on a follower routes every
//! replica's engine alike and shows in every replica's `/active`; a
//! disabled sender releases every replica's member flow; and once the
//! leader is killed the new leader registers the same Node.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::routing::any;
use common::*;
use faderz::config::Config;
use faderz::server::{Server, SetParts};
use samplez::InputState;
use samplez::blocks::{Kind, Range};
use samplez_mxl::input::{Generation, GroupWait, InputConfig, Outcome, SlotState, Timing};
use samplez_mxl::time;
use samplez_replica::state::ReplicaState;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Runtime;

const REPLICAS: usize = 3;
const DOMAIN: &str = "6f1f1d1e-56f6-4c4c-8d32-9e7f4a3b2c10";
const CONNECTION: &str = "/x-nmos/connection/v1.2/single";
/// A registry drops a Node it heard no heartbeat from for this long.
const EXPIRY: Duration = Duration::from_secs(12);

fn shape() -> Value {
    json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "ch1", "name": "Pres 1", "width": 1, "layout": "mono", "altInput": false },
            { "id": "ch2", "name": "Pres 2", "width": 1, "layout": "mono", "altInput": true },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
        ],
    })
}

/// One request the mock registry took: from replica `from`, by the path
/// prefix of the registry URL that replica was given.
#[derive(Debug, Clone)]
struct Seen {
    from: usize,
    at: Instant,
    /// The method and what follows `/x-nmos/registration/v1.3/`.
    request: String,
    /// A POSTed resource: its type and id.
    resource: Option<(String, String)>,
}

/// A Registration API, as far as a node's registration uses it: the
/// resources it holds by id, each Node's last heartbeat, and every
/// request it took.
#[derive(Default)]
struct Registry {
    resources: BTreeMap<String, String>,
    heartbeats: BTreeMap<String, Instant>,
    seen: Vec<Seen>,
}

impl Registry {
    /// Drops every Node, and with it every resource, whose last
    /// heartbeat is older than [`EXPIRY`].
    fn expire(&mut self) {
        let now = Instant::now();
        let expired: Vec<String> = (self.heartbeats.iter())
            .filter(|(_, at)| now.duration_since(**at) > EXPIRY)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.heartbeats.remove(&id);
            self.resources.clear();
        }
    }

    fn nodes(&self) -> Vec<String> {
        (self.resources.iter())
            .filter(|(_, kind)| *kind == "node")
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The ids of the resources replica `n` posted at or after `since`.
    fn posted_by(&self, n: usize, since: Instant) -> BTreeSet<String> {
        (self.seen.iter())
            .filter(|s| s.from == n && s.at >= since)
            .filter_map(|s| s.resource.as_ref().map(|(_, id)| id.clone()))
            .collect()
    }
}

type Shared = Arc<Mutex<Registry>>;

/// Serves the mock registry on a runtime of its own: replica `n` reaches
/// it at `http://<addr>/r<n>`.
fn registry() -> (Shared, SocketAddr, Runtime) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let shared: Shared = Arc::default();
    let app = Router::new()
        .route("/{*rest}", any(answer))
        .with_state(shared.clone());
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    runtime.spawn(async move { axum::serve(listener, app).await });
    (shared, addr, runtime)
}

async fn answer(
    State(shared): State<Shared>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> (StatusCode, String) {
    let path = uri.path();
    let Some((from, rest)) = (path.strip_prefix("/r"))
        .and_then(|p| p.split_once("/x-nmos/registration/v1.3/"))
        .and_then(|(n, rest)| Some((n.parse::<usize>().ok()?, rest.to_owned())))
    else {
        return (StatusCode::NOT_FOUND, String::new());
    };
    let mut r = shared.lock().unwrap();
    r.expire();
    let mut seen = Seen {
        from,
        at: Instant::now(),
        request: format!("{method} {rest}"),
        resource: None,
    };
    let parts: Vec<&str> = rest.split('/').collect();
    let reply = match (&method, parts.as_slice()) {
        (&Method::POST, ["resource"]) => {
            let v: Value = serde_json::from_slice(&body).unwrap_or_default();
            let kind = v["type"].as_str().unwrap_or_default().to_owned();
            let id = v["data"]["id"].as_str().unwrap_or_default().to_owned();
            seen.resource = Some((kind.clone(), id.clone()));
            if kind == "node" {
                r.heartbeats.insert(id.clone(), Instant::now());
            }
            let held = r.resources.insert(id, kind).is_some();
            let status = if held {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            (status, v["data"].to_string())
        }
        (&Method::POST, ["health", "nodes", id]) => match r.heartbeats.get_mut(*id) {
            Some(at) => {
                *at = Instant::now();
                (StatusCode::OK, json!({ "health": "0" }).to_string())
            }
            None => (StatusCode::NOT_FOUND, String::new()),
        },
        (&Method::DELETE, ["resource", "nodes", id]) => {
            if r.heartbeats.remove(*id).is_some() {
                r.resources.clear();
                (StatusCode::NO_CONTENT, String::new())
            } else {
                (StatusCode::NOT_FOUND, String::new())
            }
        }
        (&Method::DELETE, ["resource", _, id]) => match r.resources.remove(*id) {
            Some(_) => (StatusCode::NO_CONTENT, String::new()),
            None => (StatusCode::NOT_FOUND, String::new()),
        },
        _ => (StatusCode::BAD_REQUEST, String::new()),
    };
    r.seen.push(seen);
    reply
}

/// What every replica of the set shares, and what each keeps across a
/// restart.
struct Set {
    at: Dir,
    domain: Dir,
    _sources: Vec<Source>,
    ch1: String,
    /// The flow an activation routes ch1 to.
    other: String,
    inputs: Value,
    outputs: Value,
    ports: Vec<u16>,
    tls: Vec<Value>,
    markers: Arc<Memory>,
    /// Each replica's member flow of the programme output.
    members: Vec<String>,
    registry: SocketAddr,
}

struct Node {
    runtime: Runtime,
    server: Server,
}

impl Set {
    fn new(registry: SocketAddr) -> (Set, Vec<std::net::TcpListener>) {
        let at = domain("set-nmos-volumes");
        let domain = domain("set-nmos");
        let (ch1, ch2, ch2b, other) = (mxl_id(), mxl_id(), mxl_id(), mxl_id());
        let sources = vec![
            Source::sine(&domain.0, &ch1, 0.5, 440),
            Source::sine(&domain.0, &ch2, 0.25, 1000),
            Source::sine(&domain.0, &ch2b, 0.3, 220),
            Source::sine(&domain.0, &other, 0.1, 200),
        ];
        let listeners: Vec<std::net::TcpListener> = (0..REPLICAS)
            .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
            .collect();
        let ports = (listeners.iter())
            .map(|l| l.local_addr().unwrap().port())
            .collect();
        let members: Vec<String> = (0..REPLICAS).map(|_| mxl_id()).collect();
        let set = Set {
            tls: certificates(&at.0, REPLICAS),
            at,
            domain,
            _sources: sources,
            inputs: json!({ "ch1": { "flow": ch1 }, "ch2": { "flow": ch2, "altFlow": ch2b } }),
            outputs: json!({ "out-pgm": { "flows": members, "mergedFlow": mxl_id() } }),
            ch1,
            other,
            members,
            ports,
            markers: Arc::default(),
            registry,
        };
        (set, listeners)
    }

    /// Starts replica `n` on a runtime of its own, on `listener` or its
    /// peer port bound again.
    fn start(&self, n: usize, listener: Option<std::net::TcpListener>) -> Node {
        let listener = listener.unwrap_or_else(|| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match std::net::TcpListener::bind(("127.0.0.1", self.ports[n])) {
                    Ok(l) => break l,
                    Err(e) => assert!(Instant::now() < deadline, "rebinding port: {e}"),
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let peers: Vec<String> = (self.ports.iter())
            .map(|p| format!("localhost:{p}"))
            .collect();
        let api = json!({ "bind": "127.0.0.1", "port": 0 });
        let config = json!({
            "listeners": { "api": api, "ops": api, "nmos": api },
            "auth": { "mode": "none" },
            "replica": {
                "name": format!("faderz-{n}"),
                "ordinal": n,
                "dataDir": self.at.0.join(format!("data-{n}")),
                "replicas": REPLICAS,
                "peers": peers,
                "peer": { "bind": "127.0.0.1", "port": self.ports[n], "tls": self.tls[n] },
                "marker": "memory",
                "maxApplyLeadMs": 120,
            },
            "shape": shape(),
            "mxl": {
                "libraryPath": libmxl(),
                "domains": [{ "name": "d", "path": self.domain.0, "id": DOMAIN }],
                "inputs": self.inputs,
                "outputs": self.outputs,
            },
            "rt": { "rtCpu": "none" },
            // The seed defaults to the StatefulSet's name, `faderz`, on
            // every replica; each reaches the registry under its own path.
            "nmos": {
                "enabled": true,
                "advertiseHost": "127.0.0.1",
                "advertisePort": 8081,
                "heartbeatS": 1,
                "registry": { "url": format!("http://{}/r{n}", self.registry) },
            },
        });
        let (config, shape, name) = Config::parse(&config.to_string(), || unreachable!()).unwrap();
        let parts = SetParts {
            markers: self.markers.clone(),
            listener,
            write_timeout: Duration::from_secs(10),
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        let server = runtime
            .block_on(Server::start_with(&config, shape, name, None, Some(parts)))
            .unwrap();
        Node { runtime, server }
    }

    fn member(&self, n: usize) -> std::path::PathBuf {
        self.domain.0.join(format!("{}.mxl-flow", self.members[n]))
    }
}

impl Node {
    fn state(&self) -> Option<ReplicaState> {
        self.server.app.replica.contact_metrics()?.replica?.state
    }

    fn ops(&self, route: &str) -> (u16, Value) {
        let (status, body) = self.runtime.block_on(get(self.server.ops, route));
        (status, serde_json::from_str(&body).unwrap())
    }

    /// `method` `path` on the replica's NMOS listener: status and body.
    fn nmos(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        self.runtime
            .block_on(request(self.server.nmos, method, path, body))
    }

    fn leads(&self) -> bool {
        let r = self.ops("/healthz").1["replica"].clone();
        r["leader"] == r["id"]
    }

    /// Channel `slot`'s input: its state and flow.
    fn input(&self, slot: usize) -> (InputState, String) {
        let status = self.server.audio.as_ref().unwrap().engine().status();
        let i = &status.inputs[slot];
        (i.state, i.flow_id.clone())
    }

    fn kill(self) {
        let Node { runtime, server } = self;
        drop(server);
        runtime.shutdown_timeout(Duration::from_secs(5));
    }
}

async fn request(addr: SocketAddr, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let body = body.map(Value::to_string).unwrap_or_default();
    let mut tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    tcp.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    tcp.read_to_string(&mut answer).await.unwrap();
    let status = answer[9..12].parse().unwrap();
    let text = answer.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, serde_json::from_str(text).unwrap_or(Value::Null))
}

fn node(nodes: &[Option<Node>], n: usize) -> &Node {
    nodes[n].as_ref().expect("running")
}

fn until(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !f() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Waits until replica `n` has been committing and ready for a second.
fn steady(nodes: &[Option<Node>], n: usize) {
    let mut since: Option<Instant> = None;
    let steady = until(60, || {
        let node = node(nodes, n);
        if node.state() == Some(ReplicaState::Committing) && node.ops("/readyz").0 == 200 {
            since.get_or_insert_with(Instant::now).elapsed() >= Duration::from_secs(1)
        } else {
            since = None;
            false
        }
    });
    assert!(steady, "replica {n} did not commit: {}", states(nodes));
}

/// Activates `staged` at once on port `port` of replica `n`.
fn activate(nodes: &[Option<Node>], n: usize, port: &str, mut staged: Value) {
    staged["activation"] = json!({ "mode": "activate_immediate" });
    let path = format!("{CONNECTION}/{port}/staged");
    let (status, body) = node(nodes, n).nmos("PATCH", &path, Some(&staged));
    assert_eq!(status, 200, "replica {n}: {body}");
}

fn active(nodes: &[Option<Node>], n: usize, port: &str) -> Value {
    let (status, body) = node(nodes, n).nmos("GET", &format!("{CONNECTION}/{port}/active"), None);
    assert_eq!(status, 200, "replica {n}: {body}");
    body
}

/// The id of the resource of `kind` labelled `label` on replica `n`.
fn id_of(nodes: &[Option<Node>], n: usize, kind: &str, label: &str) -> String {
    let (_, list) = node(nodes, n).nmos("GET", &format!("/x-nmos/node/v1.3/{kind}/"), None);
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

/// Every resource id replica `n`'s Node API serves.
fn ids(nodes: &[Option<Node>], n: usize) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for kind in [
        "self",
        "devices",
        "sources",
        "flows",
        "senders",
        "receivers",
    ] {
        let (_, v) = node(nodes, n).nmos("GET", &format!("/x-nmos/node/v1.3/{kind}/"), None);
        for r in v.as_array().cloned().unwrap_or_else(|| vec![v.clone()]) {
            ids.insert(r["id"].as_str().unwrap().to_owned());
        }
    }
    ids
}

/// The block 80 ms behind libmxl's clock.
fn now_block() -> u64 {
    let api = api();
    time::timestamp_to_index(&api, time::now(&api)) / B - 80
}

/// Blocks per replica: an XXH3 of each block's samples and its energy,
/// for the blocks its engine committed whole.
type Blocks = Vec<BTreeMap<u64, (u64, f64)>>;

/// Reads the member flows of the running replicas from block `from`, but
/// at most 200 blocks back, to 80 ms behind the clock into `blocks`: the
/// engine reports the state of recent blocks only.
fn read(set: &Set, nodes: &[Option<Node>], from: u64, blocks: &mut Blocks) {
    let last = now_block();
    let from = from.max(last.saturating_sub(200));
    let running = (0..REPLICAS).filter(|&n| nodes[n].is_some());
    for (i, n) in running.enumerate() {
        let mut generation = Generation::new(&api(), &set.domain.0, 1).unwrap();
        let cfg = InputConfig {
            flow_id: &set.members[n],
            channels: 2,
            flow_channels: None,
            offset: 0,
        };
        let timing = Timing {
            block: B,
            margin: 0,
            max_catchup_blocks: 0,
            resync_budget: 0,
        };
        generation.attach(0, &cfg, &timing).unwrap();
        let input = generation.input(0).unwrap();
        let engine = node(nodes, n).server.audio.as_ref().unwrap().engine();
        let mut state = SlotState::default();
        let mut buf = vec![0.0f32; 2 * B as usize];
        for k in from..=last {
            let whole = (engine.block_status(0, k))
                .is_some_and(|s| s.kind == Kind::Committed && s.range == Range::Whole);
            if whole && input.read(&mut state, k, &GroupWait::Skipped, &mut buf) == Outcome::Copy {
                let bytes: Vec<u8> = buf.iter().flat_map(|v| v.to_bits().to_le_bytes()).collect();
                let energy = buf.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
                blocks[i].insert(k, (xxhash_rust::xxh3::xxh3_64(&bytes), energy));
            }
        }
    }
}

/// The blocks every replica read: how many, the indices where any
/// differs, and the mean energy of a block.
fn compare(blocks: &Blocks) -> (usize, Vec<u64>, f64) {
    let mut compared = 0;
    let mut differing = Vec::new();
    let mut energy = 0.0;
    for (k, (hash, e)) in &blocks[0] {
        let others: Vec<_> = blocks[1..].iter().filter_map(|b| b.get(k)).collect();
        if others.len() + 1 < blocks.len() {
            continue;
        }
        compared += 1;
        energy += e;
        if others.iter().any(|(h, _)| h != hash) {
            differing.push(*k);
        }
    }
    (compared, differing, energy / compared.max(1) as f64)
}

/// Reads the member flows of the running replicas from block `from` on
/// until they committed at least `least` blocks alike, within 30 s; no
/// block may differ. The mean energy of a block.
fn alike(set: &Set, nodes: &[Option<Node>], from: u64, least: usize) -> f64 {
    let running = (0..REPLICAS).filter(|&n| nodes[n].is_some()).count();
    let mut blocks: Blocks = vec![BTreeMap::new(); running];
    let mut last = (0, Vec::new(), 0.0);
    let done = until(30, || {
        read(set, nodes, from, &mut blocks);
        last = compare(&blocks);
        assert!(last.1.is_empty(), "replicas differ: {last:?}");
        last.0 >= least
    });
    assert!(done, "only {} blocks compared: {}", last.0, states(nodes));
    last.2
}

/// Writes `changes` in one PATCH /state through replica `n`.
fn write(nodes: &[Option<Node>], n: usize, changes: Value) {
    let body = json!({ "commandId": faderz::app::uuid().unwrap(), "changes": changes });
    let api = node(nodes, n).server.api;
    let (status, answer) = node(nodes, n).runtime.block_on(async {
        let body = body.to_string();
        let mut tcp = tokio::net::TcpStream::connect(api).await.unwrap();
        let request = format!(
            "PATCH /api/v1/state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
             X-Samplez-Request: 1\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        tcp.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        tcp.read_to_string(&mut answer).await.unwrap();
        (answer[9..12].to_owned(), answer)
    });
    assert_eq!(status, "200", "{answer}");
}

/// Only the leader registers the Node. An activation taken on a follower
/// is a command of the set: every replica's engine routes it at the same
/// sample, so the member flows stay bit for bit alike, and every
/// replica's `/active` shows it. A disabled sender releases every
/// replica's member flow, an enabled one writes them again. Killed, the
/// leader's Node is registered again by the new leader, under the same
/// ids, and the registry holds that one Node.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn the_set_is_one_node_that_the_leader_registers_and_every_replica_connects() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .try_init();
    let (registry, addr, _registry_runtime) = registry();
    let (set, listeners) = Set::new(addr);
    let mut nodes: Vec<Option<Node>> = listeners
        .into_iter()
        .enumerate()
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }

    // One Node: every replica serves the same ids.
    let all = ids(&nodes, 0);
    for n in 1..REPLICAS {
        assert_eq!(ids(&nodes, n), all, "replica {n}");
    }
    let leader = (0..REPLICAS)
        .find(|&n| node(&nodes, n).leads())
        .expect("a leader");
    let followers: Vec<usize> = (0..REPLICAS).filter(|&n| n != leader).collect();
    let since = Instant::now();

    // (a) The leader registers every resource; the followers send the
    // registry nothing, through several heartbeats.
    let registered = until(20, || {
        let r = registry.lock().unwrap();
        r.posted_by(leader, Instant::now() - Duration::from_secs(3600)) == all
    });
    assert!(
        registered,
        "the leader did not register: {:?}",
        registry.lock().unwrap().seen
    );
    std::thread::sleep(Duration::from_secs(3));
    {
        let r = registry.lock().unwrap();
        for &f in &followers {
            let spoke = r
                .seen
                .iter()
                .filter(|s| s.from == f && s.at >= since)
                .count();
            assert_eq!(spoke, 0, "follower {f} spoke to the registry: {:?}", r.seen);
        }
        let heartbeats = (r.seen.iter())
            .filter(|s| s.from == leader && s.request.starts_with("POST health/"))
            .count();
        assert!(
            heartbeats >= 2,
            "the leader does not heartbeat: {:?}",
            r.seen
        );
        assert_eq!(r.nodes().len(), 1);
    }

    // (c) An activation PATCHed to a follower routes every replica's
    // engine to the flow and shows in every replica's /active. ch1 is
    // the one channel open to the programme bus.
    write(
        &nodes,
        leader,
        json!([
            { "path": "/channels/ch1/fader", "value": 0 },
            { "path": "/channels/ch1/routes/pgm", "value": true },
            { "path": "/channels/ch2/mute", "value": true },
        ]),
    );
    std::thread::sleep(Duration::from_millis(500));
    let before = alike(&set, &nodes, now_block() + 1, 100);
    assert!(before > 1e-6, "a silent mix");
    let ch1 = format!("receivers/{}", id_of(&nodes, 0, "receivers", "Pres 1"));
    activate(
        &nodes,
        followers[0],
        &ch1,
        json!({ "transport_params": [{ "mxl_flow_id": set.other }] }),
    );
    let routed = until(20, || {
        (0..REPLICAS).all(|n| node(&nodes, n).input(0) == (InputState::Attached, set.other.clone()))
    });
    assert!(
        routed,
        "not every replica reads the flow: {}",
        states(&nodes)
    );
    let shown = until(10, || {
        (0..REPLICAS).all(|n| {
            active(&nodes, n, &ch1)["transport_params"][0]["mxl_flow_id"] == set.other.as_str()
        })
    });
    assert!(shown, "not every replica shows the activation");
    let after = alike(&set, &nodes, now_block() + 1, 100);
    assert!(
        (after - before).abs() > before * 0.1,
        "the activation did not change the mix: {before} -> {after}"
    );
    assert_ne!(set.other, set.ch1);

    // (d) A sender disabled on a follower releases every replica's member
    // flow; enabled on another, it writes them again.
    let pgm = format!("senders/{}", id_of(&nodes, 0, "senders", "PGM out"));
    for n in 0..REPLICAS {
        assert!(set.member(n).exists(), "replica {n}'s member flow");
    }
    activate(
        &nodes,
        followers[1],
        &pgm,
        json!({ "master_enable": false }),
    );
    let released = until(20, || (0..REPLICAS).all(|n| !set.member(n).exists()));
    assert!(released, "a member flow is still there: {}", states(&nodes));
    let disabled = until(10, || {
        (0..REPLICAS).all(|n| active(&nodes, n, &pgm)["master_enable"] == false)
    });
    assert!(disabled, "not every replica shows the sender disabled");
    activate(&nodes, leader, &pgm, json!({ "master_enable": true }));
    let written = until(20, || (0..REPLICAS).all(|n| set.member(n).exists()));
    assert!(written, "a member flow is not back: {}", states(&nodes));
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    alike(&set, &nodes, now_block() + 1, 100);

    // (b) The leader killed, the new leader registers the same Node: the
    // same ids, and the registry holds that Node alone, heartbeated by
    // the new leader.
    nodes[leader].take().unwrap().kill();
    let killed = Instant::now();
    let mut new = None;
    let elected = until(30, || {
        new = followers.iter().copied().find(|&n| node(&nodes, n).leads());
        new.is_some()
    });
    assert!(elected, "no new leader: {}", states(&nodes));
    let new = new.unwrap();
    let again = until(30, || {
        registry.lock().unwrap().posted_by(new, killed) == all
    });
    assert!(
        again,
        "the new leader did not register the Node: {:?}",
        registry.lock().unwrap().seen
    );
    std::thread::sleep(Duration::from_secs(3));
    let r = registry.lock().unwrap();
    let after_kill: Vec<&Seen> = r.seen.iter().filter(|s| s.at > killed).collect();
    let others: Vec<&&Seen> = after_kill.iter().filter(|s| s.from != new).collect();
    // The killed leader's runtime is gone; the remaining follower stays
    // silent.
    assert!(others.is_empty(), "only the new leader speaks: {others:?}");
    let node_id = node(&nodes, new)
        .nmos("GET", "/x-nmos/node/v1.3/self", None)
        .1["id"]
        .clone();
    assert_eq!(r.nodes(), [node_id.as_str().unwrap()]);
    let recent = Instant::now() - Duration::from_secs(2);
    assert!(
        after_kill
            .iter()
            .any(|s| s.at > recent && s.request.starts_with("POST health/")),
        "the new leader does not heartbeat"
    );
    drop(r);
    // The activations held across the leader change.
    for &n in &followers {
        assert_eq!(
            active(&nodes, n, &ch1)["transport_params"][0]["mxl_flow_id"],
            set.other.as_str()
        );
        assert_eq!(node(&nodes, n).input(0).1, set.other);
    }
    for node in nodes.into_iter().flatten() {
        node.kill();
    }
}

/// Each running replica's state, for a failure message.
fn states(nodes: &[Option<Node>]) -> String {
    let mut out = String::new();
    for (n, node) in nodes.iter().enumerate() {
        let Some(node) = node else {
            continue;
        };
        let replica = &node.server.app.replica;
        let raft = replica.raft().metrics().borrow().clone();
        let engine = node.server.audio.as_ref().unwrap().engine();
        out += &format!(
            "\nreplica {n}: {:?} {:?} applied {:?}, inputs {:?}, outputs {:?}",
            node.state(),
            raft.state,
            raft.last_applied,
            (engine.status().inputs.iter())
                .map(|i| (i.state, i.flow_id.clone()))
                .collect::<Vec<_>>(),
            engine.status().outputs,
        );
    }
    out
}

#[test]
fn the_mock_registry_expires_a_silent_node() {
    let mut r = Registry::default();
    r.resources.insert("n".into(), "node".into());
    r.resources.insert("d".into(), "device".into());
    r.heartbeats
        .insert("n".into(), Instant::now() - EXPIRY - Duration::from_secs(1));
    r.expire();
    assert!(r.resources.is_empty() && r.heartbeats.is_empty());
}
