//! NMOS in a set of three faderz replicas with engines, in one process,
//! each on a runtime of its own, with a mock registry each replica
//! reaches under a path of its own: the set is one Node, registered by
//! the leader alone; staging goes to the leader from any replica; an
//! activation is a command of the set, routed alike by every engine and
//! shown by every replica's `/active`, through restarts and a leader
//! change, scheduled ones included.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::registry::{self, Seen, Shared};
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
    merged: String,
    /// Each replica's NMOS listener port.
    nmos: Vec<u16>,
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
        let merged = mxl_id();
        let set = Set {
            tls: certificates(&at.0, REPLICAS),
            at,
            domain,
            _sources: sources,
            inputs: json!({ "ch1": { "flow": ch1 }, "ch2": { "flow": ch2, "altFlow": ch2b } }),
            outputs: json!({ "out-pgm": { "flows": members, "mergedFlow": merged } }),
            merged,
            nmos: (0..REPLICAS)
                .map(|_| {
                    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                    l.local_addr().unwrap().port()
                })
                .collect(),
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
        let nmos = json!({ "bind": "127.0.0.1", "port": self.nmos[n] });
        let config = json!({
            "listeners": { "api": api, "ops": api, "nmos": nmos },
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
                "peers": self.nmos.iter().map(|p| format!("127.0.0.1:{p}")).collect::<Vec<_>>(),
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

    /// Stops the replica as SIGTERM does.
    fn stop(self) {
        let Node { runtime, server } = self;
        runtime.block_on(server.stop());
        runtime.shutdown_timeout(Duration::from_secs(5));
    }

    /// The connections the replica's state holds scheduled.
    fn scheduled(&self) -> usize {
        (self.server.app.replica.store()).with_state(|sm| sm.product.scheduled().len())
    }

    /// The highest true peak of each leg of the output its engine wrote
    /// since it started.
    fn peak(&self) -> Vec<f32> {
        let status = self.server.audio.as_ref().unwrap().engine().status();
        status.outputs[0].meters.max_true_peak.clone()
    }

    fn kill(self) {
        let Node { runtime, server } = self;
        drop(server);
        runtime.shutdown_timeout(Duration::from_secs(5));
    }
}

async fn request(addr: SocketAddr, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    request_with(addr, method, path, body, "").await
}

/// [`request`] with `headers`, each line ending in CRLF.
async fn request_with(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&Value>,
    headers: &str,
) -> (u16, Value) {
    let body = body.map(Value::to_string).unwrap_or_default();
    let mut tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{headers}\
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

/// What a replica writes to its member flow from its start: the blocks
/// its engine committed whole and the loudest sample in them, read every
/// 20 ms so none of its first blocks is missed.
struct Written {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: std::thread::JoinHandle<(usize, f32)>,
}

impl Written {
    fn watch(set: &Set, n: usize, node: &Node) -> Written {
        let audio = node.server.audio.clone().expect("an engine");
        let (domain, flow) = (set.domain.0.clone(), set.members[n].clone());
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut seen = BTreeSet::new();
            let mut loudest = 0.0f32;
            let mut reader: Option<(Generation, SlotState)> = None;
            let mut buf = vec![0.0f32; 2 * B as usize];
            let mut next = 0;
            while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
                if reader.is_none() {
                    reader = Generation::new(&api(), &domain, 1).ok().and_then(|mut g| {
                        let cfg = InputConfig {
                            flow_id: &flow,
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
                        g.attach(0, &cfg, &timing).ok()?;
                        Some((g, SlotState::default()))
                    });
                }
                if let Some((g, state)) = reader.as_mut() {
                    let input = g.input(0).unwrap();
                    let last = now_block();
                    for k in next.max(last.saturating_sub(200))..=last {
                        let whole = (audio.engine().block_status(0, k))
                            .is_some_and(|s| s.kind == Kind::Committed && s.range == Range::Whole);
                        if whole
                            && input.read(state, k, &GroupWait::Skipped, &mut buf) == Outcome::Copy
                        {
                            seen.insert(k);
                            loudest = buf.iter().fold(loudest, |m, v| m.max(v.abs()));
                        }
                    }
                    next = last + 1;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            (seen.len(), loudest)
        });
        Written { stop, thread }
    }

    fn stop(self) -> (usize, f32) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.thread.join().unwrap()
    }
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

/// The flow port `port` of replica `n` shows in `/active`.
fn shows(nodes: &[Option<Node>], n: usize, port: &str) -> String {
    let a = active(nodes, n, port);
    a["transport_params"][0]["mxl_flow_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// Requests replica `n` took from the registry at or after `since`.
fn heard(registry: &Shared, n: usize, since: Instant) -> Vec<Seen> {
    let r = registry.lock().unwrap();
    (r.seen.iter())
        .filter(|s| s.from == n && s.at >= since)
        .cloned()
        .collect()
}

fn heartbeats(seen: &[Seen]) -> usize {
    (seen.iter())
        .filter(|s| s.request.starts_with("POST health/"))
        .count()
}

/// The set is one Node. Only the leader registers it, and only while a
/// quorum acknowledges it; a gracefully stopped leader leaves it
/// registered, and the next leader registers the same ids. Staging on
/// any replica reaches the leader, so parameters staged on one replica
/// are activated on another. An activation is a command of the set:
/// every engine routes it alike, every `/active` shows it, concurrent
/// ones converge, a restarted replica catches up with it, and one that
/// fails shows the state again. A receiver never takes the set's own
/// output. A switch to input b shows on every replica. A disabled sender
/// releases every member flow without costing readiness. A scheduled
/// activation is taken into the state when staged and fires on time
/// after the leader that took it stopped.
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
    let (registry, addr) = registry::start();
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
    let (fa, fb) = (followers[0], followers[1]);
    let since = Instant::now();

    // The leader registers every resource; the followers send the
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
    for &f in &followers {
        let spoke = heard(&registry, f, since);
        assert!(
            spoke.is_empty(),
            "follower {f} spoke to the registry: {spoke:?}"
        );
    }
    assert!(
        heartbeats(&heard(&registry, leader, since)) >= 2,
        "the leader does not heartbeat"
    );
    assert_eq!(registry.lock().unwrap().nodes().len(), 1);

    let ch1 = format!("receivers/{}", id_of(&nodes, 0, "receivers", "Pres 1"));
    let ch2 = format!("receivers/{}", id_of(&nodes, 0, "receivers", "Pres 2"));
    let pgm = format!("senders/{}", id_of(&nodes, 0, "senders", "PGM out"));

    // No receiver takes the set's own output: the merged flow or any
    // replica's member flow, staged on any replica.
    for n in 0..REPLICAS {
        for flow in std::iter::once(&set.merged).chain(&set.members) {
            let staged = json!({ "transport_params": [{ "mxl_flow_id": flow }] });
            let path = format!("{CONNECTION}/{ch1}/staged");
            let (status, body) = node(&nodes, n).nmos("PATCH", &path, Some(&staged));
            assert_eq!(status, 400, "replica {n}, flow {flow}: {body}");
        }
    }

    // ch1 is the one channel open to the programme bus.
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

    // Parameters staged on one follower are what an activation on the
    // other activates: both reach the leader. Every engine routes it
    // and every /active shows it.
    let staged = json!({ "transport_params": [{ "mxl_flow_id": set.other }] });
    let (status, body) = node(&nodes, fa).nmos(
        "PATCH",
        &format!("{CONNECTION}/{ch1}/staged"),
        Some(&staged),
    );
    assert_eq!(status, 200, "{body}");
    let (_, seen) = node(&nodes, fb).nmos("GET", &format!("{CONNECTION}/{ch1}/staged"), None);
    assert_eq!(
        seen["transport_params"][0]["mxl_flow_id"],
        set.other.as_str()
    );
    activate(&nodes, fb, &ch1, json!({}));
    let routed = until(20, || {
        (0..REPLICAS).all(|n| node(&nodes, n).input(0) == (InputState::Attached, set.other.clone()))
    });
    assert!(
        routed,
        "not every replica reads the flow: {}",
        states(&nodes)
    );
    let shown = until(10, || {
        (0..REPLICAS).all(|n| shows(&nodes, n, &ch1) == set.other)
    });
    assert!(shown, "not every replica shows the activation");
    // The followers, which the leader served the activation for, show
    // its activation from the state.
    for n in [fa, fb] {
        let a = &active(&nodes, n, &ch1)["activation"];
        assert_eq!(a["mode"], "activate_immediate", "replica {n}: {a}");
        assert!(a["activation_time"].is_string(), "replica {n}: {a}");
    }
    // A request marked forwarded that reaches a follower is not staged
    // there.
    let marked = node(&nodes, fa).runtime.block_on(request_with(
        node(&nodes, fa).server.nmos,
        "PATCH",
        &format!("{CONNECTION}/{ch1}/staged"),
        Some(&json!({ "master_enable": true })),
        "x-faderz-forwarded: 1\r\n",
    ));
    assert_eq!(marked.0, 503, "{marked:?}");
    let after = alike(&set, &nodes, now_block() + 1, 100);
    assert!(
        (after - before).abs() > before * 0.1,
        "the activation did not change the mix: {before} -> {after}"
    );

    // Two activations of one port at once, on two replicas, converge:
    // every replica shows and reads the same flow.
    std::thread::scope(|scope| {
        for (n, flow) in [(fa, &set.ch1), (fb, &set.other)] {
            let nodes = &nodes;
            let ch1 = &ch1;
            scope.spawn(move || {
                activate(
                    nodes,
                    n,
                    ch1,
                    json!({ "transport_params": [{ "mxl_flow_id": flow }] }),
                );
            });
        }
    });
    let converged = until(20, || {
        let shown: BTreeSet<String> = (0..REPLICAS).map(|n| shows(&nodes, n, &ch1)).collect();
        shown.len() == 1
            && (0..REPLICAS).all(|n| {
                node(&nodes, n).input(0) == (InputState::Attached, shown.first().unwrap().clone())
            })
    });
    assert!(
        converged,
        "the replicas did not converge: {}",
        states(&nodes)
    );
    alike(&set, &nodes, now_block() + 1, 100);

    // A switch to input b on a follower shows on every replica, under a
    // new IS-04 version of the receiver.
    let path = format!("/x-nmos/node/v1.3/{ch2}");
    let versions: Vec<Value> = (0..REPLICAS)
        .map(|n| node(&nodes, n).nmos("GET", &path, None).1["version"].clone())
        .collect();
    write(
        &nodes,
        fa,
        json!([{ "path": "/channels/ch2/input", "value": "b" }]),
    );
    let b = set.inputs["ch2"]["altFlow"].as_str().unwrap().to_owned();
    let switched = until(10, || (0..REPLICAS).all(|n| shows(&nodes, n, &ch2) == b));
    assert!(switched, "not every replica shows input b");
    for (n, before) in versions.iter().enumerate() {
        let version = node(&nodes, n).nmos("GET", &path, None).1["version"].clone();
        assert_ne!(version, *before, "replica {n}'s receiver version");
    }

    // A sender disabled on a follower releases every replica's member
    // flow; the replicas stay ready. Enabled on the leader, every member
    // flow is written again and they agree.
    for n in 0..REPLICAS {
        assert!(set.member(n).exists(), "replica {n}'s member flow");
    }
    activate(&nodes, fb, &pgm, json!({ "master_enable": false }));
    let released = until(20, || (0..REPLICAS).all(|n| !set.member(n).exists()));
    assert!(released, "a member flow is still there: {}", states(&nodes));
    let disabled = until(10, || {
        (0..REPLICAS).all(|n| active(&nodes, n, &pgm)["master_enable"] == false)
    });
    assert!(disabled, "not every replica shows the sender disabled");
    for _ in 0..20 {
        for n in 0..REPLICAS {
            assert_eq!(
                node(&nodes, n).ops("/readyz").0,
                200,
                "replica {n}: {}",
                states(&nodes)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    activate(&nodes, leader, &pgm, json!({ "master_enable": true }));
    let written = until(20, || (0..REPLICAS).all(|n| set.member(n).exists()));
    assert!(written, "a member flow is not back: {}", states(&nodes));
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    alike(&set, &nodes, now_block() + 1, 100);

    // A follower down while ch1 is activated catches up with it when it
    // is back.
    nodes[fa].take().unwrap().kill();
    activate(
        &nodes,
        leader,
        &ch1,
        json!({ "transport_params": [{ "mxl_flow_id": set.ch1 }] }),
    );
    let routed = until(20, || node(&nodes, fb).input(0).1 == set.ch1);
    assert!(routed, "the follower does not read ch1's flow");
    nodes[fa] = Some(set.start(fa, None));
    steady(&nodes, fa);
    let caught = until(20, || {
        shows(&nodes, fa, &ch1) == set.ch1 && node(&nodes, fa).input(0).1 == set.ch1
    });
    assert!(
        caught,
        "the restarted replica did not catch up: {}",
        states(&nodes)
    );
    alike(&set, &nodes, now_block() + 1, 100);

    // A scheduled activation staged on one follower and cancelled on the
    // other never fires.
    let staged = json!({
        "transport_params": [{ "mxl_flow_id": set.other }],
        "activation": { "mode": "activate_scheduled_relative", "requested_time": "2:0" },
    });
    let (status, body) = node(&nodes, fa).nmos(
        "PATCH",
        &format!("{CONNECTION}/{ch1}/staged"),
        Some(&staged),
    );
    assert_eq!(status, 202, "{body}");
    let taken = until(5, || {
        (0..REPLICAS).all(|n| node(&nodes, n).scheduled() == 1)
    });
    assert!(taken, "the schedule is not state on every replica");
    let cancel = json!({ "activation": { "mode": null } });
    let (status, body) = node(&nodes, fb).nmos(
        "PATCH",
        &format!("{CONNECTION}/{ch1}/staged"),
        Some(&cancel),
    );
    assert_eq!(status, 200, "{body}");
    let gone = until(5, || {
        (0..REPLICAS).all(|n| node(&nodes, n).scheduled() == 0)
    });
    assert!(gone, "the cancel did not reach the state");
    std::thread::sleep(Duration::from_secs(3));
    for n in 0..REPLICAS {
        assert_eq!(
            node(&nodes, n).scheduled(),
            0,
            "the cancel left a schedule behind"
        );
        assert_eq!(
            node(&nodes, n).input(0).1,
            set.ch1,
            "replica {n} fired a cancelled activation"
        );
        assert_eq!(shows(&nodes, n, &ch1), set.ch1, "replica {n}");
    }

    // A scheduled activation staged on a follower is state at once. The
    // leader stopped gracefully before its time, the others route and
    // show it on time; the stopped leader deletes nothing from the
    // registry, and the new leader registers the same ids. The leader is
    // looked up again: a loaded host may have elected another.
    let leader = (0..REPLICAS)
        .find(|&n| node(&nodes, n).leads())
        .expect("a leader");
    let followers: Vec<usize> = (0..REPLICAS).filter(|&n| n != leader).collect();
    let fa = followers[0];
    let staged = json!({
        "transport_params": [{ "mxl_flow_id": set.other }],
        "activation": { "mode": "activate_scheduled_relative", "requested_time": "3:0" },
    });
    let (status, body) = node(&nodes, fa).nmos(
        "PATCH",
        &format!("{CONNECTION}/{ch1}/staged"),
        Some(&staged),
    );
    assert_eq!(status, 202, "{body}");
    let at = Instant::now() + Duration::from_secs(3);
    let taken = until(5, || {
        (0..REPLICAS).all(|n| node(&nodes, n).scheduled() == 1)
    });
    assert!(taken, "the schedule is not state on every replica");
    let stopping = Instant::now();
    nodes[leader].take().unwrap().stop();
    let stopped = Instant::now();
    assert!(
        heard(&registry, leader, stopping)
            .iter()
            .all(|s| !s.request.starts_with("DELETE")),
        "the stopped leader deleted from the registry"
    );
    std::thread::sleep(at.saturating_duration_since(Instant::now()));
    let fired = until(10, || {
        followers.iter().all(|&n| {
            node(&nodes, n).input(0).1 == set.other && shows(&nodes, n, &ch1) == set.other
        })
    });
    assert!(
        fired,
        "the scheduled activation did not fire: {}",
        states(&nodes)
    );
    assert!(followers.iter().all(|&n| node(&nodes, n).scheduled() == 0));
    let mut new = None;
    let elected = until(30, || {
        new = followers.iter().copied().find(|&n| node(&nodes, n).leads());
        new.is_some()
    });
    assert!(elected, "no new leader: {}", states(&nodes));
    let new = new.unwrap();
    let again = until(10, || {
        registry.lock().unwrap().posted_by(new, stopped) == all
    });
    assert!(
        again,
        "the new leader did not register the Node: {:?}",
        registry.lock().unwrap().seen
    );
    std::thread::sleep(Duration::from_secs(2));
    {
        let r = registry.lock().unwrap();
        let others: Vec<&Seen> = (r.seen.iter())
            .filter(|s| s.at > stopped && s.from != new)
            .collect();
        assert!(others.is_empty(), "only the new leader speaks: {others:?}");
        let node_id = node(&nodes, new)
            .nmos("GET", "/x-nmos/node/v1.3/self", None)
            .1["id"]
            .clone();
        assert_eq!(r.nodes(), [node_id.as_str().unwrap()]);
    }
    assert!(
        heartbeats(&heard(
            &registry,
            new,
            Instant::now() - Duration::from_secs(2)
        )) >= 1
    );

    let other = followers.iter().copied().find(|&n| n != new).unwrap();

    // A channel detached in the state is not heard from a replica
    // started again before it took the state: ch1, the one channel open
    // to the programme bus, detached, the restarted replica's output
    // peaks at nothing; ch1 attached again, it is heard.
    activate(&nodes, new, &ch1, json!({ "master_enable": false }));
    let detached = until(10, || {
        [new, other]
            .iter()
            .all(|&n| node(&nodes, n).input(0).0 == InputState::Detached)
    });
    assert!(detached, "ch1 is not detached: {}", states(&nodes));
    nodes[leader] = Some(set.start(leader, None));
    let written = Written::watch(&set, leader, node(&nodes, leader));
    steady(&nodes, leader);
    std::thread::sleep(Duration::from_secs(2));
    let (blocks, loudest) = written.stop();
    assert!(
        blocks > 100,
        "the restarted replica committed {blocks} blocks"
    );
    assert_eq!(
        loudest, 0.0,
        "a detached channel was heard in the member flow"
    );
    activate(
        &nodes,
        new,
        &ch1,
        json!({ "master_enable": true, "transport_params": [{ "mxl_flow_id": set.other }] }),
    );
    let heard_again = until(10, || node(&nodes, leader).peak().iter().any(|&p| p > 0.0));
    assert!(
        heard_again,
        "the restarted replica's output stays silent: {}",
        states(&nodes)
    );

    // A scheduled activation taken by a leader that then stops shows in
    // the next leader's /staged, which it locks; a cancel through any
    // replica removes it, and the same time staged again with another
    // target fires that one.
    let ch2_flow = set.inputs["ch2"]["flow"].as_str().unwrap().to_owned();
    let now = samplez_nmos::TaiClock::now(&samplez_nmos::SystemTai).as_nanos();
    let at = now + 12_000_000_000;
    let at = format!("{}:{}", at / 1_000_000_000, at % 1_000_000_000);
    let absolute = |flow: &str| {
        json!({
            "transport_params": [{ "mxl_flow_id": flow }],
            "activation": { "mode": "activate_scheduled_absolute", "requested_time": at },
        })
    };
    let path = format!("{CONNECTION}/{ch1}/staged");
    let (status, body) = node(&nodes, new).nmos("PATCH", &path, Some(&absolute(&set.ch1)));
    assert_eq!(status, 202, "{body}");
    let taken = until(5, || {
        [new, other, leader]
            .iter()
            .all(|&n| node(&nodes, n).scheduled() == 1)
    });
    assert!(taken, "the schedule is not state on every replica");
    nodes[new].take().unwrap().stop();
    let survivors = [leader, other];
    let mut next = None;
    let elected = until(30, || {
        next = survivors.iter().copied().find(|&n| node(&nodes, n).leads());
        next.is_some()
    });
    assert!(
        elected,
        "no leader after the second stop: {}",
        states(&nodes)
    );
    let next = next.unwrap();
    let follower = survivors.iter().copied().find(|&n| n != next).unwrap();
    for n in survivors {
        let (status, staged) = node(&nodes, n).nmos("GET", &path, None);
        assert_eq!(status, 200, "{staged}");
        assert_eq!(
            staged["activation"]["mode"], "activate_scheduled_absolute",
            "{staged}"
        );
        assert_eq!(
            staged["activation"]["requested_time"],
            at.as_str(),
            "{staged}"
        );
        assert_eq!(
            staged["transport_params"][0]["mxl_flow_id"],
            set.ch1.as_str(),
            "{staged}"
        );
    }
    let (status, _) =
        node(&nodes, follower).nmos("PATCH", &path, Some(&json!({ "master_enable": true })));
    assert_eq!(status, 423, "a pending activation locks /staged");
    let (status, body) = node(&nodes, follower).nmos("PATCH", &path, Some(&cancel));
    assert_eq!(status, 200, "{body}");
    let gone = until(5, || {
        survivors.iter().all(|&n| node(&nodes, n).scheduled() == 0)
    });
    assert!(gone, "the cancel did not reach the state");
    let (_, staged) = node(&nodes, next).nmos("GET", &path, None);
    assert!(staged["activation"]["mode"].is_null(), "{staged}");
    let (status, body) = node(&nodes, follower).nmos("PATCH", &path, Some(&absolute(&ch2_flow)));
    assert_eq!(status, 202, "{body}");
    let fired = until(20, || {
        survivors.iter().all(|&n| {
            let input = node(&nodes, n).input(0).1;
            assert_ne!(input, set.ch1, "replica {n} fired the cancelled activation");
            input == ch2_flow && shows(&nodes, n, &ch1) == ch2_flow
        })
    });
    assert!(
        fired,
        "the activation staged again did not fire: {}",
        states(&nodes)
    );
    let (new, other) = (next, follower);

    // The other follower killed, the leader has no quorum: it stops
    // heartbeating, and an activation on it fails and its /active shows
    // the state's connection again.
    nodes[other].take().unwrap().kill();
    std::thread::sleep(Duration::from_secs(3));
    let quiet = Instant::now();
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        heartbeats(&heard(&registry, new, quiet)),
        0,
        "a leader without a quorum heartbeats"
    );
    let was = shows(&nodes, new, &ch1);
    activate(
        &nodes,
        new,
        &ch1,
        json!({ "transport_params": [{ "mxl_flow_id": set.ch1 }] }),
    );
    assert_eq!(
        shows(&nodes, new, &ch1),
        set.ch1,
        "/active shows the activation at once"
    );
    let back = until(30, || {
        let a = active(&nodes, new, &ch1);
        a["master_enable"] == true && a["transport_params"][0]["mxl_flow_id"] == was.as_str()
    });
    assert!(
        back,
        "/active does not show the state again: {}",
        active(&nodes, new, &ch1)
    );
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
