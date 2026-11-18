//! Three faderz replicas of one set with engines, in one process: each
//! runs on a runtime of its own, so killing one ends every task it ran
//! and drops its engine. They read the same input flows, each writes its
//! own member flow, and what they write agrees, through a busy command
//! sequence and a replica killed and started again.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use common::*;
use faderz::audio::Audio;
use faderz::config::Config;
use faderz::server::{Server, SetParts};
use samplez::blocks::{Kind, Range};
use samplez_mxl::input::{Generation, GroupWait, InputConfig, Outcome, SlotState, Timing};
use samplez_mxl::time;
use samplez_replica::state::ReplicaState;
use serde_json::{Value, json};
use tokio::runtime::Runtime;

const REPLICAS: usize = 3;

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

/// What every replica of the set shares, and what each one keeps across
/// a restart: its data directory and peer port.
struct Set {
    at: Dir,
    domain: Dir,
    _sources: Vec<Source>,
    inputs: Value,
    outputs: Value,
    ports: Vec<u16>,
    tls: Vec<Value>,
    markers: Arc<Memory>,
    /// Each replica's member flow of the programme output.
    members: Vec<String>,
}

/// A replica on its own runtime.
struct Node {
    runtime: Runtime,
    server: Server,
}

impl Set {
    fn new(name: &str) -> (Set, Vec<std::net::TcpListener>) {
        // The volumes on tmpfs: a test host's disk, shared with builds,
        // can take longer to sync an append than the apply lead allows.
        let at = domain(&format!("set-volumes-{name}"));
        let domain = domain(&format!("set-{name}"));
        let (ch1, ch2, ch2b) = (new_id(), new_id(), new_id());
        let sources = vec![
            Source::sine(&domain.0, &ch1, 0.5, 440),
            Source::sine(&domain.0, &ch2, 0.25, 1000),
            Source::sine(&domain.0, &ch2b, 0.3, 220),
        ];
        let listeners: Vec<std::net::TcpListener> = (0..REPLICAS)
            .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
            .collect();
        let ports = (listeners.iter())
            .map(|l| l.local_addr().unwrap().port())
            .collect();
        let members: Vec<String> = (0..REPLICAS).map(|_| new_id()).collect();
        let set = Set {
            tls: certificates(&at.0, REPLICAS),
            at,
            domain,
            _sources: sources,
            inputs: json!({ "ch1": { "flow": ch1 }, "ch2": { "flow": ch2, "altFlow": ch2b } }),
            outputs: json!({ "out-pgm": { "flows": members } }),
            members,
            ports,
            markers: Arc::default(),
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
            "api": { "allowedOrigins": ["http://faderz.test"] },
            "replica": {
                "name": format!("faderz-{n}"),
                "ordinal": n,
                "dataDir": self.at.0.join(format!("data-{n}")),
                "replicas": REPLICAS,
                "peers": peers,
                "peer": { "bind": "127.0.0.1", "port": self.ports[n], "tls": self.tls[n] },
                "marker": "memory",
                // A loaded test host applies late more often than a node.
                "maxApplyLeadMs": 120,
            },
            "shape": shape(),
            "mxl": {
                "libraryPath": libmxl(),
                "domains": [{ "name": "d", "path": self.domain.0 }],
                "inputs": self.inputs,
                "outputs": self.outputs,
            },
            "rt": { "rtCpu": "none" },
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
}

impl Node {
    fn engine(&self) -> Weak<Audio> {
        Arc::downgrade(self.server.audio.as_ref().expect("an engine"))
    }

    /// The replica's state as its contact keeps it.
    fn state(&self) -> Option<ReplicaState> {
        let metrics = self.server.app.replica.contact_metrics()?;
        metrics.replica?.state
    }

    fn ops(&self, route: &str) -> (u16, Value) {
        let (status, body) = self.runtime.block_on(get(self.server.ops, route));
        (status, serde_json::from_str(&body).unwrap())
    }

    /// Ends every task of the replica and drops its engine, as a killed
    /// process would: nothing is quiesced or stopped first.
    fn kill(self) {
        let Node { runtime, server } = self;
        drop(server);
        runtime.shutdown_timeout(Duration::from_secs(5));
    }
}

/// What each replica committed to its member flow of the programme
/// output, read 80 ms behind libmxl's clock every 20 ms: per block index,
/// an XXH3 of the block's samples, for every block its engine reports
/// committed whole. A block it withheld is never written, so the flow
/// holds an older lap's samples there. A replica's flow is gone while it
/// is down, and read again once it is back.
struct Outputs {
    blocks: Mutex<Vec<BTreeMap<u64, u64>>>,
    engines: Mutex<Vec<Weak<Audio>>>,
    stop: AtomicBool,
}

impl Outputs {
    fn read(
        domain: PathBuf,
        flows: Vec<String>,
        engines: Vec<Weak<Audio>>,
    ) -> (Arc<Outputs>, std::thread::JoinHandle<()>) {
        let outputs = Arc::new(Outputs {
            blocks: Mutex::new(vec![BTreeMap::new(); flows.len()]),
            engines: Mutex::new(engines),
            stop: AtomicBool::new(false),
        });
        let out = outputs.clone();
        let thread = std::thread::spawn(move || {
            let mut readers: Vec<Option<(Generation, SlotState)>> =
                flows.iter().map(|_| None).collect();
            let mut next = vec![0u64; flows.len()];
            let mut buf = vec![0.0f32; 2 * B as usize];
            while !out.stop.load(Ordering::Relaxed) {
                let last = Outputs::now();
                for (n, flow) in flows.iter().enumerate() {
                    if readers[n].is_none() {
                        readers[n] = attach(&domain, flow).map(|g| (g, SlotState::default()));
                    }
                    let Some((generation, state)) = readers[n].as_mut() else {
                        continue;
                    };
                    let input = generation.input(0).unwrap();
                    let engine = out.engines.lock().unwrap()[n].upgrade();
                    let from = next[n].max(last.saturating_sub(800));
                    let mut copied = false;
                    for k in from..=last {
                        let committed = engine
                            .as_ref()
                            .and_then(|e| e.engine().block_status(0, k))
                            .is_some_and(|s| s.kind == Kind::Committed && s.range == Range::Whole);
                        let read =
                            input.read(state, k, &GroupWait::Skipped, &mut buf) == Outcome::Copy;
                        if read && committed {
                            let bytes: Vec<u8> =
                                buf.iter().flat_map(|v| v.to_bits().to_le_bytes()).collect();
                            let hash = xxhash_rust::xxh3::xxh3_64(&bytes);
                            out.blocks.lock().unwrap()[n].insert(k, hash);
                            copied = true;
                        }
                    }
                    next[n] = last + 1;
                    // A flow deleted and created again is read anew.
                    if !copied {
                        readers[n] = None;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        (outputs, thread)
    }

    /// Blocks replicas `a` and `b` both wrote at or after block `from`:
    /// how many, the indices where they differ, and how many distinct
    /// blocks there were.
    fn compare(&self, a: usize, b: usize, from: u64) -> Comparison {
        let blocks = self.blocks.lock().unwrap();
        let mut c = Comparison::default();
        for (k, ha) in blocks[a].range(from..) {
            let Some(hb) = blocks[b].get(k) else {
                continue;
            };
            c.compared += 1;
            if ha != hb {
                c.differing.push(*k);
            }
            c.distinct.insert(*ha);
        }
        c
    }

    /// Replica `n` runs `engine` from now on.
    fn restarted(&self, n: usize, engine: Weak<Audio>) {
        self.engines.lock().unwrap()[n] = engine;
        self.blocks.lock().unwrap()[n].clear();
    }

    /// The block 80 ms behind libmxl's clock: the newest one read.
    fn now() -> u64 {
        let api = api();
        time::timestamp_to_index(&api, time::now(&api)) / B - 80
    }
}

/// A reader of the stereo flow `flow`, once it exists.
fn attach(domain: &Path, flow: &str) -> Option<Generation> {
    let mut generation = Generation::new(&api(), domain, 1).ok()?;
    let cfg = InputConfig {
        flow_id: flow,
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
    generation.attach(0, &cfg, &timing).ok()?;
    Some(generation)
}

#[derive(Default, Debug)]
struct Comparison {
    compared: usize,
    differing: Vec<u64>,
    distinct: std::collections::BTreeSet<u64>,
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

/// Writes `changes` in one PATCH through the running replica `first`
/// names, or the next one, while the set answers otherwise than applied,
/// as a client fails over.
fn write(nodes: &[Option<Node>], first: usize, changes: &[(&str, Value)]) {
    let changes: Vec<Value> = (changes.iter())
        .map(|(p, v)| json!({ "path": p, "value": v }))
        .collect();
    let mut answers = Vec::new();
    let running: Vec<&Node> = (0..REPLICAS)
        .map(|n| (first + n) % REPLICAS)
        .filter_map(|n| nodes[n].as_ref())
        .collect();
    for attempt in 0..30 {
        let node = running[attempt % running.len()];
        let body =
            json!({ "commandId": faderz::app::uuid().unwrap(), "changes": &changes }).to_string();
        let answer = node.runtime.block_on(request(node.server.api, &body));
        if answer.starts_with("HTTP/1.1 200") {
            return;
        }
        answers.push(answer.split_once("\r\n\r\n").map_or("", |b| b.1).to_owned());
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{changes:?} not applied: {answers:?}{}", states(nodes));
}

async fn request(api: std::net::SocketAddr, body: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
    answer
}

/// The `i`th command of a console session: faders, trims, mutes and
/// routes, and every seventh switches ch2 between its inputs a and b,
/// once together with a fader.
fn command(i: usize) -> Vec<(&'static str, Value)> {
    let db = |k: usize| json!(-((k * 7 % 40) as i64));
    match i % 7 {
        0 if i.is_multiple_of(14) => vec![("/channels/ch2/input", json!("b"))],
        0 => vec![
            ("/channels/ch2/input", json!("a")),
            ("/channels/ch1/fader", db(i + 3)),
        ],
        1 => vec![("/channels/ch1/fader", db(i))],
        2 => vec![("/channels/ch2/fader", db(i))],
        3 => vec![("/channels/ch1/trim", json!((i % 13) as i64 - 6))],
        4 => vec![("/channels/ch2/mute", json!(i.is_multiple_of(2)))],
        5 => vec![("/channels/ch1/routes/pgm", json!(!i.is_multiple_of(3)))],
        _ => vec![
            ("/channels/ch1/fader", db(i + 1)),
            ("/channels/ch2/fader", db(i + 2)),
        ],
    }
}

fn node(nodes: &[Option<Node>], n: usize) -> &Node {
    nodes[n].as_ref().expect("running")
}

fn committing(node: &Node) -> bool {
    node.state() == Some(ReplicaState::Committing)
}

/// Compares every pair of replicas from block `from` on until each pair
/// committed at least `least` blocks alike, within 60 s: no block differs,
/// and they are not all alike.
fn agree(nodes: &[Option<Node>], outputs: &Outputs, from: u64, least: usize) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let mut short = None;
        for a in 0..REPLICAS {
            for b in a + 1..REPLICAS {
                let c = outputs.compare(a, b, from);
                assert!(c.differing.is_empty(), "replicas {a} and {b}: {c:?}");
                if c.compared < least {
                    short = Some((a, b, c));
                    continue;
                }
                // The sines repeat every 50 ms; gains and routes add more.
                assert!(c.distinct.len() > 20, "a constant output: {c:?}");
            }
        }
        let Some((a, b, c)) = short else {
            return;
        };
        assert!(
            Instant::now() < deadline,
            "replicas {a} and {b} committed {} blocks alike, not {least}: {}",
            c.compared,
            states(nodes)
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Waits until replica `n` has been committing and ready for a second:
/// an engine started or rebuilt is withdrawn until its pages match the
/// others'.
fn steady(nodes: &[Option<Node>], n: usize) {
    let mut since: Option<Instant> = None;
    let steady = until(60, || {
        let node = node(nodes, n);
        if committing(node) && node.ops("/readyz").0 == 200 {
            since.get_or_insert_with(Instant::now).elapsed() >= Duration::from_secs(1)
        } else {
            since = None;
            false
        }
    });
    assert!(steady, "replica {n} did not commit: {}", states(nodes));
}

/// Every replica of the set writes the same audio through a session of
/// faders, trims, mutes, routes and input switches. A follower killed
/// meanwhile misses commands, input switches among them; started again
/// on its volume it catches up, commits again and writes what the
/// others write.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn replicas_write_equal_audio_and_a_restarted_one_catches_up() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .try_init();
    let (set, listeners) = Set::new("audio");
    let mut nodes: Vec<Option<Node>> = listeners
        .into_iter()
        .enumerate()
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    let engines = (0..REPLICAS).map(|n| node(&nodes, n).engine()).collect();
    let (outputs, reader) = Outputs::read(set.domain.0.clone(), set.members.clone(), engines);
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }

    let from = Outputs::now() + 1;
    for i in 0..42 {
        write(&nodes, i, &command(i));
        std::thread::sleep(Duration::from_millis(40));
    }
    std::thread::sleep(Duration::from_millis(500));
    agree(&nodes, &outputs, from, 500);

    // A follower is killed; the other two go on through input switches.
    let leader = node(&nodes, 0).ops("/healthz").1["replica"]["leader"].clone();
    let killed = (0..REPLICAS)
        .find(|&n| node(&nodes, n).ops("/healthz").1["replica"]["id"] != leader)
        .unwrap();
    let alive: Vec<usize> = (0..REPLICAS).filter(|&n| n != killed).collect();
    nodes[killed].take().unwrap().kill();
    let flow = set
        .domain
        .0
        .join(format!("{}.mxl-flow", set.members[killed]));
    let gone = until(10, || !flow.exists());
    assert!(gone, "the killed replica's engine still holds its flow");
    for i in 42..70 {
        write(&nodes, alive[i % 2], &command(i));
        std::thread::sleep(Duration::from_millis(40));
    }
    nodes[killed] = Some(set.start(killed, None));
    outputs.restarted(killed, node(&nodes, killed).engine());
    // Back once it applied the log it missed and commits again.
    steady(&nodes, killed);
    let from = Outputs::now() + 1;
    for i in 70..98 {
        write(&nodes, i, &command(i));
        std::thread::sleep(Duration::from_millis(40));
    }
    std::thread::sleep(Duration::from_millis(500));
    agree(&nodes, &outputs, from, 300);

    outputs.stop.store(true, Ordering::Relaxed);
    reader.join().unwrap();
    for node in nodes.into_iter().flatten() {
        node.kill();
    }
}

/// Each running replica's state and contact, for a failure message.
fn states(nodes: &[Option<Node>]) -> String {
    let mut out = String::new();
    for (n, node) in nodes.iter().enumerate() {
        let Some(node) = node else {
            continue;
        };
        let replica = &node.server.app.replica;
        let raft = replica.raft().metrics().borrow().clone();
        let engine = node.server.audio.as_ref().unwrap().engine();
        let k = Outputs::now() + 40;
        out += &format!(
            "\nreplica {n}: outputs {:?} block {k}: {:?}",
            engine.status().outputs,
            engine.block_status(0, k)
        );
        out += &format!(
            "\nreplica {n}: {:?} {:?} {:?} applied {:?} status {:?}\n  {:?}",
            node.state(),
            raft.state,
            raft.membership_config.membership(),
            raft.last_applied,
            replica.status(),
            replica.contact_metrics(),
        );
    }
    out
}
