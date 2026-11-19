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
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "ducks": [{ "id": "talk", "name": "Talk" }],
        "outputs": [
            { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
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

/// The ch1 peak, in centi-dB, of the meter frame the replica `node`
/// sends a client after the first `skip`.
fn ch1_peak(node: &Node, skip: usize) -> i16 {
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
                // After the 12-byte header and the replica's name; ch1's
                // peak is the frame's first value.
                let at = 12 + usize::from(f[2]);
                return i16::from_le_bytes([f[at], f[at + 1]]);
            }
        }
    })
}

/// Each replica of a set meters its channels at their taps on the
/// replicated state, as a single replica does: ch1 meters after its fader
/// by default, so on every replica it reads its -6 dBFS sine once its
/// fader opens, digital silence once it closes, and 10 dB less at -10.
/// The faders start closed, so a replica metering at the gains it
/// started with reads silence throughout.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "three debug-build engines miss their deadlines on a shared host; CI runs it in release"
)]
fn every_replicas_meters_follow_the_faders() {
    /// A level of digital silence, -120 dB.
    const FLOOR: i16 = -12_000;
    let (set, listeners) = Set::new("taps");
    let nodes: Vec<Option<Node>> = (listeners.into_iter().enumerate())
        .map(|(n, l)| Some(set.start(n, Some(l))))
        .collect();
    for n in 0..REPLICAS {
        steady(&nodes, n);
    }
    for (fader, want, what) in [
        (0, -602, "open"),
        (-90, FLOOR, "closed"),
        (-10, -1602, "at -10 dB"),
    ] {
        write(&nodes, 0, &[("/channels/ch1/fader", json!(fader))]);
        for n in 0..REPLICAS {
            let mut last = 0;
            let read = until(10, || {
                // The meters cover a 100 ms window: the first frames are
                // skipped.
                last = ch1_peak(node(&nodes, n), 3);
                (last - want).abs() <= 10
            });
            assert!(
                read,
                "replica {n}'s ch1 meter with its fader {what}: {last}, not {want}: {}",
                states(&nodes)
            );
        }
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

    // The leader is killed; the other two elect another and go on.
    let leader = node(&nodes, 0).ops("/healthz").1["replica"]["leader"].clone();
    let killed = (0..REPLICAS)
        .find(|&n| node(&nodes, n).ops("/healthz").1["replica"]["id"] == leader)
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
