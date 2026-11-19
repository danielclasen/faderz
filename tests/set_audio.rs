//! Three faderz replicas of one set with engines, in one process: each
//! runs on a runtime of its own, so killing one ends every task it ran
//! and drops its engine. They read the same input flows, each writes its
//! own member flow, and what they write agrees, through a busy command
//! sequence with a duck group keying and releasing, and a replica killed
//! and started again.

mod common;

use std::collections::{BTreeMap, BTreeSet};
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
        "auxes": [], "vcas": [{ "id": "vca1", "name": "Music" }],
        "mixMinus": [{ "id": "mm1", "name": "N-1", "bus": "pgm", "width": 2, "layout": "stereo" }],
        "monitorOutputs": [{ "id": "mon", "name": "Monitor", "width": 2, "layout": "stereo" }],
        "ducks": [{ "id": "talk", "name": "Talk" }],
        "outputs": [
            { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
            { "id": "out-mm", "name": "N-1 out", "source": "mm1", "width": 2, "layout": "stereo" },
        ],
    })
}

/// The duck group: ch1 keys it after its fader, so the faders of
/// [`command`] trigger and release it (ch1's sine at -6 dBFS reaches the
/// threshold with its fader above -14 dB), and it lowers ch2 in the
/// programme. Short times, so it moves between commands.
fn ducking() -> Vec<(&'static str, Value)> {
    vec![
        ("/ducks/talk/channels/ch1/trigger", json!(true)),
        ("/ducks/talk/channels/ch2/target", json!(true)),
        ("/ducks/talk/channels/ch2/depth", json!(12)),
        ("/ducks/talk/keyTap", json!("postFader")),
        ("/ducks/talk/threshold", json!(-20)),
        ("/ducks/talk/attackMs", json!(10)),
        ("/ducks/talk/holdMs", json!(50)),
        ("/ducks/talk/releaseMs", json!(100)),
    ]
}

/// What every replica of the set shares, and what each one keeps across
/// a restart: its data directory and peer port.
struct Set {
    at: Dir,
    domain: Dir,
    _sources: Vec<Source>,
    inputs: Value,
    outputs: Value,
    shape: Value,
    ports: Vec<u16>,
    tls: Vec<Value>,
    markers: Arc<Memory>,
    /// Each replica's member flow of the programme output.
    members: Vec<String>,
    /// Each replica's member flow of the mix-minus output.
    mm_members: Vec<String>,
}

/// A replica on its own runtime.
struct Node {
    runtime: Runtime,
    server: Server,
}

impl Set {
    fn new(name: &str) -> (Set, Vec<std::net::TcpListener>) {
        Set::with(name, shape(), &["out-pgm", "out-mm"], |domain| {
            let (ch1, ch2, ch2b) = (new_id(), new_id(), new_id());
            let sources = vec![
                Source::sine(domain, &ch1, 0.5, 440),
                Source::sine(domain, &ch2, 0.25, 1000),
                Source::sine(domain, &ch2b, 0.3, 220),
            ];
            let inputs = json!({ "ch1": { "flow": ch1 }, "ch2": { "flow": ch2, "altFlow": ch2b } });
            (sources, inputs)
        })
    }

    /// A set of `shape`, each of `outputs` written by every replica to a
    /// flow of its own, reading the `inputs` `sources` writes into the
    /// domain it is given.
    fn with(
        name: &str,
        shape: Value,
        outputs: &[&str],
        sources: impl FnOnce(&Path) -> (Vec<Source>, Value),
    ) -> (Set, Vec<std::net::TcpListener>) {
        // The volumes on tmpfs: a test host's disk, shared with builds,
        // can take longer to sync an append than the apply lead allows.
        let at = domain(&format!("set-volumes-{name}"));
        let domain = domain(&format!("set-{name}"));
        let (sources, inputs) = sources(&domain.0);
        let listeners: Vec<std::net::TcpListener> = (0..REPLICAS)
            .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
            .collect();
        let ports = (listeners.iter())
            .map(|l| l.local_addr().unwrap().port())
            .collect();
        // The first output asked for, whatever order the map keeps.
        let first = outputs[0];
        let flows = |_: &&str| -> Vec<String> { (0..REPLICAS).map(|_| new_id()).collect() };
        let outputs: serde_json::Map<String, Value> = (outputs.iter())
            .map(|id| ((*id).to_owned(), json!({ "flows": flows(id) })))
            .collect();
        let flows_of = |output: &Value| -> Vec<String> {
            (output["flows"].as_array().unwrap().iter())
                .map(|f| f.as_str().unwrap().to_owned())
                .collect()
        };
        let members = flows_of(&outputs[first]);
        let mm_members = outputs.get("out-mm").map(flows_of).unwrap_or_default();
        let set = Set {
            tls: certificates(&at.0, REPLICAS),
            at,
            domain,
            _sources: sources,
            inputs,
            outputs: Value::Object(outputs),
            shape,
            members,
            mm_members,
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
            "shape": self.shape,
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
/// an XXH3 of the block's samples and their sum of squares, for every
/// block its engine reports committed whole. A block it withheld is never written, so the flow
/// holds an older lap's samples there. A replica's flow is gone while it
/// is down, and read again once it is back.
struct Outputs {
    blocks: Mutex<Vec<BTreeMap<u64, (u64, f64)>>>,
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
                            let energy = buf.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
                            out.blocks.lock().unwrap()[n].insert(k, (hash, energy));
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
    /// how many, the indices where they differ, how many distinct blocks
    /// there were, and the levels `a` wrote ([`levels`]).
    fn compare(&self, a: usize, b: usize, from: u64) -> Comparison {
        let blocks = self.blocks.lock().unwrap();
        let mut c = Comparison {
            levels: levels(&blocks[a], from),
            ..Comparison::default()
        };
        for (k, (ha, _)) in blocks[a].range(from..) {
            let Some((hb, _)) = blocks[b].get(k) else {
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
    attach_wide(domain, flow, 2)
}

/// A reader of the flow `flow` of `channels` legs, once it exists.
fn attach_wide(domain: &Path, flow: &str, channels: u32) -> Option<Generation> {
    let mut generation = Generation::new(&api(), domain, 1).ok()?;
    let cfg = InputConfig {
        flow_id: flow,
        channels,
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
    distinct: BTreeSet<u64>,
    levels: BTreeSet<i64>,
}

/// Blocks per window of [`levels`]: 50 ms, a whole number of periods of
/// every source sine, so a window's energy is the same while no command
/// changes the mix.
const WINDOW: u64 = 50;

/// The levels of the whole windows of `blocks` at or after block `from`:
/// each window's energy in thousandths of a decade, `i64::MIN` for a
/// silent one. One level is a mix no command changed.
fn levels(blocks: &BTreeMap<u64, (u64, f64)>, from: u64) -> BTreeSet<i64> {
    let mut windows: BTreeMap<u64, (u64, f64)> = BTreeMap::new();
    for (k, (_, energy)) in blocks.range(from..) {
        let w = windows.entry(k / WINDOW).or_default();
        *w = (w.0 + 1, w.1 + energy);
    }
    (windows.values())
        .filter(|(count, _)| *count == WINDOW)
        .map(|&(_, energy)| {
            if energy > 1e-9 {
                (energy.log10() * 1000.0).round() as i64
            } else {
                i64::MIN
            }
        })
        .collect()
}

/// A mix no command changed has one level, silence included; commands
/// that change gains add others.
#[test]
fn a_mix_no_command_changed_has_one_level() {
    let window = |from: u64, energy: f64| (from..from + WINDOW).map(move |k| (k, (k, energy)));
    let steady: BTreeMap<u64, (u64, f64)> = (0..4).flat_map(|w| window(w * WINDOW, 0.01)).collect();
    assert_eq!(levels(&steady, 0).len(), 1);
    let silent: BTreeMap<u64, (u64, f64)> = (0..4).flat_map(|w| window(w * WINDOW, 0.0)).collect();
    assert_eq!(levels(&silent, 0), BTreeSet::from([i64::MIN]));
    let mixed: BTreeMap<u64, (u64, f64)> = (0..4)
        .flat_map(|w| window(w * WINDOW, 0.01 / (1 + w) as f64))
        .collect();
    assert_eq!(levels(&mixed, 0).len(), 4);
    // A partial window counts for nothing.
    assert_eq!(levels(&mixed, 10).len(), 3);
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
/// once together with a fader; every eleventh latches the duck group on
/// or back to auto.
fn command(i: usize) -> Vec<(&'static str, Value)> {
    let db = |k: usize| json!(-((k * 7 % 40) as i64));
    if i % 11 == 10 {
        let mode = if i % 22 == 10 { "on" } else { "auto" };
        return vec![("/ducks/talk/mode", json!(mode))];
    }
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

/// Compares every pair of the running replicas from block `from` on
/// until each pair committed at least `least` blocks alike, within 60 s:
/// no block differs, they are not all alike, and the commands sent since
/// `from` changed the mix, which is not silent.
fn agree(nodes: &[Option<Node>], outputs: &Outputs, from: u64, least: usize) {
    let running: Vec<usize> = (0..REPLICAS).filter(|&n| nodes[n].is_some()).collect();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let mut short = None;
        for (i, &a) in running.iter().enumerate() {
            for &b in &running[i + 1..] {
                let c = outputs.compare(a, b, from);
                assert!(c.differing.is_empty(), "replicas {a} and {b}: {c:?}");
                if c.compared < least {
                    short = Some((a, b, c));
                    continue;
                }
                // The sines repeat every 50 ms; gains and routes add more.
                assert!(c.distinct.len() > 20, "a constant output: {c:?}");
                assert!(c.levels.len() >= 4, "no command changed the mix: {c:?}");
                assert!(
                    c.levels.last().is_some_and(|&l| l > -3_000),
                    "a silent mix: {c:?}"
                );
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

/// Where in the meter frame's values ch1's peak and the monitor's left
/// leg are, as the shape lays the frame out.
const CH1: usize = 0;
fn mon() -> usize {
    let shape = faderz::shape::Shape::parse(&shape()).unwrap();
    faderz::meters::Layout::of(&shape).offset("mon").unwrap()
}

/// Held by each test that runs a set: two sets of three engines at once
/// miss their deadlines on a CI host, so they run one after the other.
static ONE_SET: Mutex<()> = Mutex::new(());

/// Value `index` of the meter frame the replica `node` sends a client
/// after the first `skip`: [`CH1`] or [`mon()`], a peak in centi-dB.
fn peak(node: &Node, skip: usize, index: usize) -> i16 {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    node.runtime.block_on(async {
        let session = "session-m-0123456789abcdef";
        let url = format!("ws://{}/api/v1/ws?session={session}", node.server.api);
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let hello = json!({ "type": "hello", "session": session, "kind": "ui", "mode": "primary" });
        ws.send(Message::text(hello.to_string())).await.unwrap();
        let credit = json!({ "type": "credit", "n": 16 });
        ws.send(Message::text(credit.to_string())).await.unwrap();
        let mut frames = 0;
        loop {
            let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("a message within 5 s")
                .unwrap()
                .unwrap();
            if let Message::Binary(f) = m {
                frames += 1;
                if frames <= skip {
                    continue;
                }
                assert_eq!(f[..2], [0x4d, 4], "magic and version");
                // After the 12-byte header and the replica's name, padded
                // to an even length.
                let name = usize::from(f[2]);
                let at = 12 + name + name % 2 + 2 * index;
                return i16::from_le_bytes([f[at], f[at + 1]]);
            }
        }
    })
}

/// Each replica of a set meters its channels at their taps on the
/// replicated state, as a single replica does: ch1 meters after its fader
/// by default, so on every replica it reads its -6 dBFS sine once its
/// fader opens, digital silence once it closes, 10 dB less at -10, and
/// silence again once muted.
/// The faders start closed, so a replica metering at the gains it
/// started with reads silence throughout. The monitor meter follows a
/// PFL alike.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn every_replicas_meters_follow_the_faders() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    /// A level of digital silence, -120 dB.
    const FLOOR: i16 = -12_000;
    let (set, listeners) = Set::new("taps");
    let nodes: Vec<Option<Node>> = (listeners.into_iter().enumerate())
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    let steps = [
        (
            ("/channels/ch1/fader", json!(0)),
            -602,
            "with its fader open",
        ),
        (
            ("/channels/ch1/fader", json!(-90)),
            FLOOR,
            "with its fader closed",
        ),
        (
            ("/channels/ch1/fader", json!(-10)),
            -1602,
            "with its fader at -10 dB",
        ),
        (("/channels/ch1/mute", json!(true)), FLOOR, "muted"),
    ];
    // The monitor meters what it plays: the programme, ch2's -12 dBFS
    // sine centred at -3 dB, until ch1's PFL takes its place, centred
    // at -3 dB before its closed fader and mute, and the programme again
    // once every solo is off; then a VCA's AFL of ch2, which the
    // programme no longer carries, plays it at its fader.
    let monitor = [
        (("/monitor/level", json!(0)), FLOOR, "on its silent source"),
        (("/channels/ch2/fader", json!(0)), -1505, "on its source"),
        (("/channels/ch1/fader", json!(-90)), -1505, "on its source"),
        (("/channels/ch1/solo", json!("pfl")), -903, "on ch1's PFL"),
        (
            ("/channels/ch1/solo", json!("off")),
            -1505,
            "on its source again",
        ),
        // ch2 off the programme leaves the source silent; its VCA's AFL
        // plays it after its fader again.
        (
            ("/channels/ch2/routes/pgm", json!(false)),
            FLOOR,
            "on its silent source",
        ),
        (
            ("/vcas/vca1/members", json!(["ch2"])),
            FLOOR,
            "on its silent source",
        ),
        (("/vcas/vca1/solo", json!("afl")), -1505, "on its VCA's AFL"),
        (
            ("/vcas/vca1/solo", json!("off")),
            FLOOR,
            "on its silent source again",
        ),
    ];
    let steps =
        (steps.into_iter().map(|s| (CH1, s))).chain(monitor.into_iter().map(|s| (mon(), s)));
    for (index, ((path, value), want, what)) in steps {
        write(&nodes, 0, &[(path, value)]);
        for n in 0..REPLICAS {
            let mut last = 0;
            let read = until(10, || {
                // The meters cover a 100 ms window: the first frames are
                // skipped.
                last = peak(node(&nodes, n), 3, index);
                (last - want).abs() <= 10
            });
            let meter = if index == mon() { "monitor" } else { "ch1" };
            assert!(
                read,
                "replica {n}'s {meter} meter {what}: {last}, not {want}: {}",
                states(&nodes)
            );
        }
    }
}

/// A reset of an output's loudness posted to one replica restarts the
/// measurement on every replica: the highest momentary loudness since
/// the last reset falls on each to the level the output has then, and
/// the replicas agree on the integrated loudness.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn a_loudness_reset_restarts_every_replicas_measurement() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    let (set, listeners) = Set::new("loud");
    let nodes: Vec<Option<Node>> = (listeners.into_iter().enumerate())
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    let shape = faderz::shape::Shape::parse(&shape()).unwrap();
    // pgm: peak, true peak and RMS of two legs, then momentary,
    // short-term, integrated, range, and the highest momentary.
    let at = faderz::meters::Layout::of(&shape).offset("pgm").unwrap();
    let (momentary, integrated, max_momentary) = (at + 6, at + 8, at + 10);
    let null = i16::MIN;
    let read = |n: usize, index: usize| peak(node(&nodes, n), 1, index);
    write(&nodes, 0, &[("/channels/ch1/fader", json!(0))]);
    for n in 0..REPLICAS {
        let measured = until(60, || {
            read(n, max_momentary) != null && read(n, integrated) != null
        });
        assert!(measured, "replica {n} measures: {}", states(&nodes));
    }
    // A steady tone: the maximum is settled when the momentary loudness
    // has reached it.
    for n in 0..REPLICAS {
        let settled = until(60, || {
            let (now, max) = (read(n, momentary), read(n, max_momentary));
            now != null && max != null && max - now <= 30
        });
        assert!(settled, "replica {n}'s maximum settles: {}", states(&nodes));
    }
    let loud = (0..REPLICAS).map(|n| read(n, max_momentary)).max().unwrap();
    // 20 dB down: every maximum holds, the momentary levels follow.
    write(&nodes, 0, &[("/channels/ch1/fader", json!(-20))]);
    for n in 0..REPLICAS {
        let lower = until(30, || {
            let now = read(n, momentary);
            now != null && now <= loud - 1800
        });
        assert!(lower, "replica {n} is 20 dB down: {}", states(&nodes));
        assert!(read(n, max_momentary) >= loud - 100, "the maximum holds");
    }
    // Posted to a replica that need not lead.
    let api = node(&nodes, 1).server.api;
    let (status, body) =
        node(&nodes, 1)
            .runtime
            .block_on(post(api, "/loudness/reset", &json!({ "ids": ["pgm"] })));
    assert_eq!(status, 204, "{body}");
    for n in 0..REPLICAS {
        let reset = until(30, || {
            let max = read(n, max_momentary);
            max != null && max <= loud - 1500
        });
        assert!(reset, "replica {n} restarted: {}", states(&nodes));
    }
    // A steady tone measured from one sample: the replicas agree.
    for n in 0..REPLICAS {
        assert!(until(60, || read(n, integrated) != null));
    }
    std::thread::sleep(Duration::from_secs(3));
    let each: Vec<i16> = (0..REPLICAS).map(|n| read(n, integrated)).collect();
    let spread = each.iter().max().unwrap() - each.iter().min().unwrap();
    assert!(spread <= 5, "integrated loudness {each:?} centi-LU");
}

/// Every replica of a set with engines lists all three in its `replicas`
/// message, as clients show the set, healthy and able to commit; one
/// killed, the other two each list it down within a few report windows,
/// and themselves still able to commit with two of three.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn every_replica_lists_the_set_and_a_killed_one_as_down() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    let (set, listeners) = Set::new("members");
    let mut nodes: Vec<Option<Node>> = (listeners.into_iter().enumerate())
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    let names: Vec<String> = (0..REPLICAS).map(|n| format!("faderz-{n}")).collect();
    let ids = |v: &Value| -> Vec<String> {
        (v.as_array().unwrap().iter())
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect()
    };
    for n in 0..REPLICAS {
        let mut last = Value::Null;
        let all = until(10, || {
            last = node(&nodes, n).server.app.replicas();
            last.as_array()
                .unwrap()
                .iter()
                .all(|r| r["healthy"] == true && r["commitable"] == true)
                && ids(&last) == names
        });
        assert!(all, "replica {n} lists {last}: {}", states(&nodes));
        let leaders = (last.as_array().unwrap().iter())
            .filter(|r| r["role"] == "leader")
            .count();
        assert_eq!(leaders, 1, "replica {n} lists {last}");
    }
    nodes[2].take().unwrap().kill();
    for n in 0..2 {
        let mut last = Value::Null;
        let down = until(20, || {
            last = node(&nodes, n).server.app.replicas();
            let listed = last.as_array().unwrap();
            ids(&last) == names
                && listed.iter().all(|r| {
                    let up = r["id"] != "faderz-2";
                    r["healthy"] == up && r["commitable"] == up
                })
        });
        assert!(down, "replica {n} lists {last}: {}", states(&nodes));
    }
}

/// Contact runs from each replica's start. The set forms with the first
/// replica alone and grows by two voters at once: with two of the three
/// running the first commits alone and the second stays a learner, and
/// once the third starts all three are voters and commit. Every replica
/// of the set then writes the same audio through a session of faders,
/// trims, mutes, routes and input switches, with a duck group keyed by one
/// channel's fader lowering the other. A follower killed meanwhile misses
/// commands, input switches among them; started again on its volume it
/// catches up, commits again and writes what the others write. The leader
/// killed, the other two elect another and go on writing alike; it comes
/// back the same way.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn replicas_write_equal_audio_and_a_restarted_one_catches_up() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .try_init();
    let (set, listeners) = Set::new("audio");
    let mut listeners: Vec<Option<std::net::TcpListener>> =
        listeners.into_iter().map(Some).collect();
    let mut nodes: Vec<Option<Node>> = (0..REPLICAS).map(|_| None).collect();
    for n in 0..2 {
        nodes[n] = Some(set.start(n, listeners[n].take()));
    }
    let membership = |nodes: &[Option<Node>]| {
        let raft = node(nodes, 0)
            .server
            .app
            .replica
            .raft()
            .metrics()
            .borrow()
            .clone();
        let m = raft.membership_config.membership().clone();
        (m.voter_ids().count(), m.learner_ids().count())
    };
    // The set forms with the first replica alone, and grows by two
    // voters at once, never to an even count: with two of three running,
    // the first commits as a set of one and the second stays a learner.
    steady(&nodes, 0);
    let learner = until(30, || membership(&nodes) == (1, 1));
    assert!(
        learner,
        "the second replica is no learner: {}",
        states(&nodes)
    );
    // Contact runs on the learner too, which reports to be promoted, and
    // a learner is never ready; the first replica, alone a voter, is.
    let contact = until(10, || {
        node(&nodes, 1)
            .server
            .app
            .replica
            .contact_metrics()
            .is_some()
    });
    assert!(
        contact,
        "the learner's contact does not run: {}",
        states(&nodes)
    );
    for _ in 0..10 {
        assert_eq!(membership(&nodes), (1, 1), "{}", states(&nodes));
        assert_eq!(node(&nodes, 1).ops("/readyz").0, 503, "{}", states(&nodes));
        assert_eq!(node(&nodes, 0).ops("/readyz").0, 200, "{}", states(&nodes));
        std::thread::sleep(Duration::from_millis(100));
    }
    nodes[2] = Some(set.start(2, listeners[2].take()));
    let engines = (0..REPLICAS).map(|n| node(&nodes, n).engine()).collect();
    let (outputs, reader) = Outputs::read(set.domain.0.clone(), set.members.clone(), engines);
    let three = until(60, || membership(&nodes) == (REPLICAS, 0));
    assert!(
        three,
        "the set did not grow to three voters: {}",
        states(&nodes)
    );
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    // Each replica's status: committing, a leader or a follower, its
    // leader named by node id as it names itself.
    let mut leaders = std::collections::BTreeSet::new();
    let mut nodes_ids = std::collections::BTreeSet::new();
    for n in 0..REPLICAS {
        let node = node(&nodes, n);
        let (code, body) = node
            .runtime
            .block_on(get(node.server.api, "/api/v1/status"));
        assert_eq!(code, 200, "{body}");
        let status: Value = serde_json::from_str(&body).unwrap();
        let r = &status["replica"];
        assert_eq!(r["state"], "committing", "{status}");
        assert!(
            ["leader", "follower"].contains(&r["role"].as_str().unwrap()),
            "{status}"
        );
        nodes_ids.insert(r["node"].as_str().unwrap().to_owned());
        leaders.insert(r["leader"].as_str().unwrap().to_owned());
    }
    assert_eq!(leaders.len(), 1, "one leader: {leaders:?}");
    assert!(nodes_ids.is_superset(&leaders), "{nodes_ids:?} {leaders:?}");

    write(&nodes, 0, &ducking());
    let from = Outputs::now() + 1;
    // The largest envelope the duck group reached on a replica while in
    // auto, from three commands (120 ms, past the release) after it was
    // last latched on: keyed by ch1's fader alone.
    let latched = |i: usize| {
        (0..=i)
            .rev()
            .find(|k| k % 11 == 10)
            .is_some_and(|k| k % 22 == 10)
    };
    let mut keyed = 0.0f32;
    for i in 0..42 {
        write(&nodes, i, &command(i));
        std::thread::sleep(Duration::from_millis(40));
        if (i.saturating_sub(3)..=i).any(latched) {
            continue;
        }
        let engine = node(&nodes, i % REPLICAS).server.audio.clone().unwrap();
        let status = engine.engine().status();
        keyed = (status.ducks.iter()).fold(keyed, |e, d| e.max(d.envelope));
    }
    assert!(
        keyed > 0.5,
        "ch1's fader never keyed the duck group: {keyed}"
    );
    std::thread::sleep(Duration::from_millis(500));
    agree(&nodes, &outputs, from, 500);

    // A follower is killed; the other two go on through input switches.
    let leader = node(&nodes, 0).ops("/readyz").1["replica"]["leader"].clone();
    let killed = (0..REPLICAS)
        .find(|&n| node(&nodes, n).ops("/readyz").1["replica"]["id"] != leader)
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

    // The leader is killed; the other two elect another and go on.
    let leader = node(&nodes, 0).ops("/readyz").1["replica"]["leader"].clone();
    let killed = (0..REPLICAS)
        .find(|&n| node(&nodes, n).ops("/readyz").1["replica"]["id"] == leader)
        .expect("a leader");
    nodes[killed].take().unwrap().kill();
    let from = Outputs::now() + 1;
    for i in 98..126 {
        write(&nodes, i, &command(i));
        std::thread::sleep(Duration::from_millis(40));
    }
    std::thread::sleep(Duration::from_millis(500));
    agree(&nodes, &outputs, from, 300);
    nodes[killed] = Some(set.start(killed, None));
    outputs.restarted(killed, node(&nodes, killed).engine());
    steady(&nodes, killed);
    let from = Outputs::now() + 1;
    for i in 126..154 {
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

/// The legs of the latest block of `flow` 80 ms behind libmxl's clock,
/// planar, once it reads.
fn legs_of(domain: &Path, flow: &str, channels: u32) -> Option<Vec<Vec<f32>>> {
    let generation = attach_wide(domain, flow, channels)?;
    let n = B as usize;
    let mut buf = vec![0.0f32; channels as usize * n];
    let input = generation.input(0).unwrap();
    let k = Outputs::now();
    let o = input.read(&mut SlotState::default(), k, &GroupWait::Skipped, &mut buf);
    (o == Outcome::Copy).then(|| buf.chunks(n).map(|leg| leg.to_vec()).collect())
}

/// Every replica of a set with a 5.1 channel on a 5.1 bus writes the 6-leg
/// output as it mixes it and the stereo output as BS.775 Lo/Ro of the
/// bus, the same on all three, and each follows a bus fader written to
/// the set.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn every_replica_writes_the_fold_down_of_a_surround_bus() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    let shape = json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "sur", "name": "Surround", "width": 6, "layout": "5.1", "altInput": false },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 6, "layout": "5.1" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-51", "name": "PGM 5.1", "source": "pgm", "width": 6, "layout": "5.1" },
            { "id": "out-lr", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
        ],
    });
    // L R C LFE Ls Rs, each a distinct power of two.
    let legs = [0.5f32, 0.25, 0.125, 0.0625, 0.03125, 0.015625];
    let (set, listeners) = Set::with("fold", shape, &["out-51", "out-lr"], |domain| {
        let src = new_id();
        (
            vec![Source::start(domain, &src, legs.to_vec())],
            json!({ "sur": { "flow": src } }),
        )
    });
    let nodes: Vec<Option<Node>> = (listeners.into_iter().enumerate())
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    let flows = |output: &str| -> Vec<String> {
        (set.outputs[output]["flows"].as_array().unwrap().iter())
            .map(|f| f.as_str().unwrap().to_owned())
            .collect()
    };
    let (wide, lr) = (flows("out-51"), flows("out-lr"));
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let (lo, ro) = (
        legs[0] + h * legs[2] + h * legs[4],
        legs[1] + h * legs[2] + h * legs[5],
    );
    let near = |leg: &[f32], want: f32| leg.iter().all(|v| (v - want).abs() <= 1e-6);
    write(
        &nodes,
        0,
        &[
            ("/channels/sur/fader", json!(0)),
            ("/channels/sur/routes/pgm", json!(true)),
        ],
    );
    for (bus, g) in [(0.0, 1.0f32), (-6.0, 10f32.powf(-6.0 / 20.0))] {
        write(&nodes, 1, &[("/buses/pgm/fader", json!(bus))]);
        for n in 0..REPLICAS {
            let mut last = None;
            let read = until(20, || {
                let (Some(w), Some(s)) = (
                    legs_of(&set.domain.0, &wide[n], 6),
                    legs_of(&set.domain.0, &lr[n], 2),
                ) else {
                    return false;
                };
                last = Some((w.iter().map(|l| l[0]).collect::<Vec<_>>(), s[0][0], s[1][0]));
                w.iter().zip(legs).all(|(leg, want)| near(leg, want * g))
                    && near(&s[0], lo * g)
                    && near(&s[1], ro * g)
            });
            assert!(
                read,
                "replica {n} at bus fader {bus}: wanted {legs:?} * {g}, Lo {lo}, Ro {ro}; read {last:?}: {}",
                states(&nodes)
            );
        }
    }
}

/// A stereo channel's image and balance are replicated state: every
/// replica writes the legs it feeds both, or swapped, and the opposite
/// leg lowered by its pan.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn every_replica_applies_a_stereo_channels_image_and_balance() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    let shape = json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "st", "name": "Music", "width": 2, "layout": "stereo", "altInput": false },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
        ],
    });
    let legs = [0.5f32, 0.25];
    let (set, listeners) = Set::with("image", shape, &["out-pgm"], |domain| {
        let src = new_id();
        (
            vec![Source::start(domain, &src, legs.to_vec())],
            json!({ "st": { "flow": src } }),
        )
    });
    let nodes: Vec<Option<Node>> = (listeners.into_iter().enumerate())
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    let near = |leg: &[f32], want: f32| leg.iter().all(|v| (v - want).abs() <= 1e-6);
    write(
        &nodes,
        0,
        &[
            ("/channels/st/fader", json!(0)),
            ("/channels/st/routes/pgm", json!(true)),
        ],
    );
    // The image, the pan and the legs every replica writes.
    let steps: [(&str, f64, [f32; 2]); 5] = [
        ("normal", 0.0, [0.5, 0.25]),
        ("swap", 0.0, [0.25, 0.5]),
        ("leftToBoth", 0.0, [0.5, 0.5]),
        ("normal", -1.0, [0.5, 0.0]),
        ("normal", 0.5, [0.25, 0.25]),
    ];
    for (image, pan, want) in steps {
        write(
            &nodes,
            1,
            &[
                ("/channels/st/image", json!(image)),
                ("/channels/st/pan/lr", json!(pan)),
            ],
        );
        for n in 0..REPLICAS {
            let mut last = None;
            let read = until(20, || {
                let Some(out) = legs_of(&set.domain.0, &set.members[n], 2) else {
                    return false;
                };
                last = Some((out[0][0], out[1][0]));
                near(&out[0], want[0]) && near(&out[1], want[1])
            });
            assert!(
                read,
                "replica {n} with image {image}, pan {pan}: wanted {want:?}, read {last:?}: {}",
                states(&nodes)
            );
        }
    }
}

/// A mono channel's surround pan is replicated state: every replica
/// writes the legs of the same place in the 5.1 field.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn every_replica_places_a_mono_channel_in_the_surround_field() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    let shape = json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "m", "name": "Mic", "width": 1, "layout": "mono", "altInput": false },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 6, "layout": "5.1" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-51", "name": "PGM 5.1", "source": "pgm", "width": 6, "layout": "5.1" },
        ],
    });
    let (set, listeners) = Set::with("surpan", shape, &["out-51"], |domain| {
        let src = new_id();
        (
            vec![Source::start(domain, &src, vec![0.5])],
            json!({ "m": { "flow": src } }),
        )
    });
    let nodes: Vec<Option<Node>> = (listeners.into_iter().enumerate())
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let near = |leg: &[f32], want: f32| leg.iter().all(|v| (v - want).abs() <= 1e-6);
    write(
        &nodes,
        0,
        &[
            ("/channels/m/fader", json!(0)),
            ("/channels/m/routes/pgm", json!(true)),
        ],
    );
    // fr, lr and the legs L R C LFE Ls Rs every replica writes.
    // A new mono channel is the L/R pair; no spread puts it in C.
    write(&nodes, 0, &[("/channels/m/pan/spread", json!(0))]);
    let steps: [(f64, f64, [f32; 6]); 4] = [
        (-1.0, 0.0, [0.0, 0.0, 0.5, 0.0, 0.0, 0.0]),
        (-1.0, -1.0, [0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
        (1.0, 0.0, [0.0, 0.0, 0.0, 0.0, 0.5 * h, 0.5 * h]),
        (0.0, 0.0, [0.0, 0.0, 0.5 * h, 0.0, 0.25, 0.25]),
    ];
    for (fr, lr, want) in steps {
        write(
            &nodes,
            1,
            &[
                ("/channels/m/pan/frontRear", json!(fr)),
                ("/channels/m/pan/lr", json!(lr)),
            ],
        );
        for n in 0..REPLICAS {
            let mut last = None;
            let read = until(20, || {
                let Some(out) = legs_of(&set.domain.0, &set.members[n], 6) else {
                    return false;
                };
                last = Some(out.iter().map(|l| l[0]).collect::<Vec<_>>());
                out.iter().zip(want).all(|(leg, w)| near(leg, w))
            });
            assert!(
                read,
                "replica {n} at fr {fr}, lr {lr}: wanted {want:?}, read {last:?}: {}",
                states(&nodes)
            );
        }
    }
}

/// A replica that joins late replays the whole log from its first entry
/// onto the initial state of the shape, as its peers' states began: the
/// entries wrote a mono channel's left-right pan on a 5.1 bus and nothing
/// of the rest of its pan, and every replica writes the same legs, the
/// L/R pair the pan was before the field.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn a_replica_replaying_the_whole_log_writes_what_its_peers_write() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    let shape = json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "m", "name": "Mic", "width": 1, "layout": "mono", "altInput": false },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 6, "layout": "5.1" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-51", "name": "PGM 5.1", "source": "pgm", "width": 6, "layout": "5.1" },
        ],
    });
    let (set, listeners) = Set::with("replay", shape, &["out-51"], |domain| {
        let src = new_id();
        (
            vec![Source::start(domain, &src, vec![0.5])],
            json!({ "m": { "flow": src } }),
        )
    });
    let mut listeners: Vec<Option<std::net::TcpListener>> =
        listeners.into_iter().map(Some).collect();
    let mut nodes: Vec<Option<Node>> = (0..REPLICAS).map(|_| None).collect();
    // The set forms on the first replica, the second a learner it grows
    // by once a third has joined.
    for n in 0..2 {
        nodes[n] = Some(set.start(n, listeners[n].take()));
    }
    steady(&nodes, 0);
    // Entries that wrote the left-right position alone.
    write(
        &nodes,
        0,
        &[
            ("/channels/m/fader", json!(0)),
            ("/channels/m/routes/pgm", json!(true)),
            ("/channels/m/pan/lr", json!(0.3)),
        ],
    );
    nodes[2] = Some(set.start(2, listeners[2].take()));
    let voters = |nodes: &[Option<Node>]| {
        let raft = node(nodes, 0)
            .server
            .app
            .replica
            .raft()
            .metrics()
            .borrow()
            .clone();
        raft.membership_config.membership().voter_ids().count()
    };
    let grown = until(60, || voters(&nodes) == REPLICAS);
    assert!(grown, "the set did not grow to three: {}", states(&nodes));
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    let theta = 1.3 * std::f32::consts::FRAC_PI_4;
    let want = [theta.cos(), theta.sin(), 0.0, 0.0, 0.0, 0.0].map(|g| g * 0.5);
    for n in 0..REPLICAS {
        let mut last = None;
        let read = until(30, || {
            let Some(out) = legs_of(&set.domain.0, &set.members[n], 6) else {
                return false;
            };
            last = Some(out.iter().map(|l| l[0]).collect::<Vec<_>>());
            out.iter()
                .zip(want)
                .all(|(leg, w)| leg.iter().all(|v| (v - w).abs() <= 1e-6))
        });
        assert!(
            read,
            "replica {n}: wanted {want:?}, read {last:?}: {}",
            states(&nodes)
        );
    }
}

/// Posts a tally of `source` through the running replica `first` names,
/// or the next one, while the set answers otherwise than applied, as
/// [`write`] does.
fn post_tally(nodes: &[Option<Node>], first: usize, source: &str, state: &str) {
    let body = json!({ "source": source, "state": state });
    let running: Vec<&Node> = (0..REPLICAS)
        .map(|n| (first + n) % REPLICAS)
        .filter_map(|n| nodes[n].as_ref())
        .collect();
    let mut answers = Vec::new();
    for attempt in 0..30 {
        let node = running[attempt % running.len()];
        let (status, answer) = node
            .runtime
            .block_on(post(node.server.api, "/afv/tally", &body));
        if status == 200 {
            return;
        }
        answers.push(answer);
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{body} not applied: {answers:?}{}", states(nodes));
}

/// Direct outs, mix-minus exclusions, bus, aux and mix-minus masters,
/// aux sends, VCAs and the AFV stage a tally drives are replicated
/// state: through every step of [`routing::steps`], written to one
/// replica after another, every replica writes the legs the step gives
/// to its member flow of each output.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn every_replica_routes_alike() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    let ids: Vec<&str> = routing::OUTPUTS.iter().map(|(id, _)| *id).collect();
    let (set, listeners) = Set::with("routing", routing::shape(), &ids, |domain| {
        let (ch1, ch2) = (new_id(), new_id());
        (
            vec![
                Source::start(domain, &ch1, vec![routing::CH1]),
                Source::start(domain, &ch2, vec![routing::CH2]),
            ],
            json!({ "ch1": { "flow": ch1 }, "ch2": { "flow": ch2 } }),
        )
    });
    let nodes: Vec<Option<Node>> = (listeners.into_iter().enumerate())
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    // Each output's member flows, by replica.
    let flows: Vec<Vec<String>> = (ids.iter())
        .map(|id| {
            (set.outputs[id]["flows"].as_array().unwrap().iter())
                .map(|f| f.as_str().unwrap().to_owned())
                .collect()
        })
        .collect();
    let near = |leg: &[f32], want: f32| leg.iter().all(|v| (v - want).abs() <= 1e-6);
    for (i, step) in routing::steps().iter().enumerate() {
        match &step.change {
            routing::Change::Patch(changes) => write(&nodes, i, changes),
            routing::Change::Tally(source, state) => post_tally(&nodes, i, source, state),
        }
        std::thread::sleep(routing::SETTLE);
        for n in 0..REPLICAS {
            let mut last = Vec::new();
            // Twice in a row, so a block written while a ramp was still
            // moving does not pass a step.
            let mut held = 0;
            let read = until(20, || {
                last.clear();
                let mut all = true;
                let outputs = (flows.iter()).zip(routing::OUTPUTS);
                for ((flow, (_, width)), want) in outputs.zip(step.legs.each()) {
                    let Some(legs) = legs_of(&set.domain.0, &flow[n], width) else {
                        all = false;
                        continue;
                    };
                    all &= legs.iter().zip(want).all(|(leg, w)| near(leg, *w));
                    last.push(legs.iter().map(|leg| leg[0]).collect::<Vec<_>>());
                }
                held = if all { held + 1 } else { 0 };
                held == 2
            });
            assert!(
                read,
                "replica {n}, {}: wanted {:?}, read {last:?}: {}",
                step.what,
                step.legs,
                states(&nodes)
            );
        }
    }
}

/// An interrupt of the mix-minus output is one replicated command: every
/// replica's member flow of it carries the talkback channel alone, at
/// its level (ch1 is 6 dB above ch2) before its closed fader, in place of the mix, alike on all
/// of them, and the mix again once released.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn an_interrupt_changes_every_replicas_mix_minus_alike() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    let (set, listeners) = Set::new("talk");
    let nodes: Vec<Option<Node>> = (listeners.into_iter().enumerate())
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    let engines = (0..REPLICAS).map(|n| node(&nodes, n).engine()).collect();
    let (outputs, reader) = Outputs::read(set.domain.0.clone(), set.mm_members.clone(), engines);
    // The mix is ch2 alone: ch1's fader stays closed.
    write(
        &nodes,
        0,
        &[
            ("/channels/ch2/fader", json!(0)),
            ("/mixMinus/mm1/interrupt/source", json!("ch1")),
        ],
    );
    // The level every replica's output holds, alike, once the engine's
    // ramp after the last write is over.
    let settled = |what: &str| -> i64 {
        std::thread::sleep(Duration::from_millis(1500));
        let from = Outputs::now();
        let mut level = None;
        let alike = until(30, || {
            std::thread::sleep(Duration::from_millis(500));
            let each: Vec<BTreeSet<i64>> = (0..REPLICAS)
                .map(|n| levels(&outputs.blocks.lock().unwrap()[n], from))
                .collect();
            let one = each[0].len() == 1 && each.iter().all(|l| *l == each[0]);
            level = each[0].first().copied();
            one
        });
        assert!(
            alike,
            "{what}: no one level on every replica: {}",
            states(&nodes)
        );
        level.unwrap()
    };
    let mix = settled("the mix");
    assert!(mix > i64::MIN, "the mix is silent");
    write(
        &nodes,
        1,
        &[("/mixMinus/mm1/interrupt/active", json!(true))],
    );
    let talk = settled("the interrupt");
    assert!(
        talk > mix + 500,
        "the talkback ({talk}) is not 6 dB above the mix ({mix})"
    );
    write(
        &nodes,
        2,
        &[("/mixMinus/mm1/interrupt/active", json!(false))],
    );
    assert_eq!(settled("the release"), mix);
    outputs.stop.store(true, Ordering::Relaxed);
    reader.join().unwrap();
}

/// GET `route` on `api` over a plain socket; `None` when it does not
/// answer within 5 s.
fn get_within(api: std::net::SocketAddr, route: &str) -> Option<u16> {
    use std::io::{Read, Write};
    let mut tcp = std::net::TcpStream::connect(api).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let request = format!("GET {route} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    tcp.write_all(request.as_bytes()).unwrap();
    let mut answer = String::new();
    tcp.read_to_string(&mut answer).ok()?;
    answer.get(9..12)?.parse().ok()
}

/// The routes that read the replica beside its state keep answering on
/// every replica while the set commits writes and each replica's contact
/// loop runs: one that took samplez's contact lock under the log store's
/// lock deadlocked against that loop, which takes them the other way
/// round, and hung the replica for good.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn every_replica_answers_its_state_routes_while_the_set_commits() {
    let _one = ONE_SET.lock().unwrap_or_else(|e| e.into_inner());
    let (set, listeners) = Set::new("routes");
    let nodes: Vec<Option<Node>> = (listeners.into_iter().enumerate())
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    let until = Instant::now() + Duration::from_secs(10);
    let readers: Vec<_> = (0..REPLICAS)
        .map(|n| {
            let api = node(&nodes, n).server.api;
            std::thread::spawn(move || {
                let mut answered = 0;
                while Instant::now() < until {
                    for route in ["/api/v1/state", "/api/v1/replicas", "/api/v1/status"] {
                        let status = get_within(api, route);
                        assert_eq!(status, Some(200), "{route} on replica {n}");
                        answered += 1;
                    }
                }
                answered
            })
        })
        .collect();
    let mut i = 0;
    while Instant::now() < until {
        write(
            &nodes,
            i,
            &[("/channels/ch1/fader", json!(-((i % 40) as i64)))],
        );
        i += 1;
    }
    for reader in readers {
        assert!(reader.join().expect("a route hung or failed") > 0);
    }
}
