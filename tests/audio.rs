//! faderz mixing real audio: constant signals written into MXL flows on a
//! tmpfs domain, controls set over the REST API, and the program bus read
//! back from its output flow and its meters.

use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use faderz::config::Config;
use faderz::server::Server;
use futures_util::{SinkExt, StreamExt};
use samplez_mxl::input::{Generation, GroupWait, InputConfig, Outcome, SlotState, Timing};
use samplez_mxl::output::{self, Creation, OutputConfig};
use samplez_mxl::{Instance, MxlApi, time};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

const B: u64 = 48;
/// ch1 and the two legs of st1, as written into their flows.
const CH1: f32 = 0.5;
const ST1: [f32; 2] = [0.25, -0.25];

fn libmxl() -> PathBuf {
    let path = PathBuf::from(
        std::env::var("SAMPLEZ_TEST_LIBMXL").unwrap_or_else(|_| "/opt/mxl/lib/libmxl.so".into()),
    );
    assert!(path.is_file(), "libmxl not found at {}", path.display());
    path
}

fn api() -> Arc<MxlApi> {
    samplez_mxl::load(&libmxl()).unwrap()
}

/// A directory removed on drop.
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(base: &Path, name: &str) -> Dir {
    let dir = base.join(format!("faderz-audio-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Dir(dir)
}

/// An MXL domain on tmpfs, with a 1 s history.
fn domain() -> Dir {
    let shm = Path::new("/dev/shm");
    let d = dir(
        if shm.is_dir() {
            shm
        } else {
            Path::new(env!("CARGO_TARGET_TMPDIR"))
        },
        "mxl",
    );
    std::fs::write(
        d.0.join("options.json"),
        r#"{"urn:x-mxl:option:history_duration/v1.0": 1000000000}"#,
    )
    .unwrap();
    d
}

fn new_id() -> String {
    let r = RandomState::new();
    let h = format!("{:016x}{:016x}", r.hash_one(1u8), r.hash_one(2u8));
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// A producer writing `values`, one per channel, into every sample of a
/// flow as libmxl's clock passes each block. Dropping it deletes the flow.
struct Source {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Source {
    fn start(domain: &Path, id: &str, values: Vec<f32>) -> Source {
        let api = api();
        let instance = Instance::new(&api, domain).unwrap();
        let cfg = OutputConfig {
            flow_id: id,
            label: "source",
            group: "source",
            role: "Audio",
            channels: values.len() as u32,
            block: B,
            batch: B,
        };
        let Creation::Created { mut writer, .. } = output::create(&instance, &cfg).unwrap() else {
            panic!("source flow {id} not created");
        };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let data: Vec<f32> = values
                .iter()
                .flat_map(|v| std::iter::repeat_n(*v, B as usize))
                .collect();
            let mut next: Option<u64> = None;
            while !flag.load(Ordering::Relaxed) {
                let idx = time::timestamp_to_index(&api, time::now(&api));
                if let Some(due) = (idx / B).checked_sub(1) {
                    for k in next.unwrap_or(due)..=due {
                        writer.write_block(k, &data);
                    }
                    next = Some(due + 1);
                }
                let k = next.unwrap_or(0);
                time::sleep_until(&api, time::index_to_timestamp(&api, (k + 1) * B));
            }
            drop(writer);
            drop(instance);
        });
        Source {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}

fn shape() -> Value {
    json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "ch1", "name": "Pres 1", "width": 1, "layout": "mono", "altInput": false },
            { "id": "ch2", "name": "Pres 2", "width": 1, "layout": "mono", "altInput": false },
            { "id": "st1", "name": "Music", "width": 2, "layout": "stereo", "altInput": false },
        ],
        "buses": [
            { "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" },
        ],
        "auxes": [],
        "mixMinus": [{ "id": "mm1", "name": "N-1", "bus": "pgm", "width": 2, "layout": "stereo" }],
        "vcas": [],
        "monitorOutputs": [{ "id": "mon", "name": "Monitor", "width": 2, "layout": "stereo" }],
        "outputs": [
            { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
            { "id": "out-mm", "name": "N-1 out", "source": "mm1", "width": 2, "layout": "stereo" },
        ],
    })
}

struct Flows {
    ch1: String,
    ch2: String,
    st1: String,
    pgm: String,
    mm: String,
}

async fn start(data: &Path, domain: &Path, f: &Flows) -> Server {
    let listener = json!({ "bind": "127.0.0.1", "port": 0 });
    let config = json!({
        "listeners": { "api": listener, "ops": listener, "nmos": listener },
        "auth": { "mode": "none" },
        "api": { "allowedOrigins": ["http://faderz.test"] },
        "replica": { "name": "r1", "dataDir": data },
        "shape": shape(),
        "mxl": {
            "libraryPath": libmxl(),
            "domains": [{ "name": "d", "path": domain }],
            "inputs": {
                "ch1": { "flow": f.ch1 },
                "ch2": { "flow": f.ch2 },
                "st1": { "flow": f.st1 },
            },
            "outputs": {
                "out-pgm": { "flow": f.pgm },
                "out-mm": { "flow": f.mm },
            },
        },
        "rt": { "rtCpu": "none" },
    });
    let (config, shape, name) = Config::parse(&config.to_string(), || unreachable!()).unwrap();
    Server::start(&config, shape, name, None).await.unwrap()
}

/// Writes `changes` (path, value) in one PATCH /state.
async fn patch(server: &Server, changes: &[(&str, Value)]) {
    let changes: Vec<Value> = changes
        .iter()
        .map(|(p, v)| json!({ "path": p, "value": v }))
        .collect();
    let body =
        json!({ "commandId": faderz::app::uuid().unwrap(), "changes": &changes }).to_string();
    let mut tcp = TcpStream::connect(server.api).await.unwrap();
    let request = format!(
        "PATCH /api/v1/state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
         X-Samplez-Request: 1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    tcp.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    tcp.read_to_string(&mut answer).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{changes:?}: {answer}");
}

/// Reads stereo blocks of `flow` a little behind libmxl's clock.
struct Reader {
    generation: Generation,
    state: SlotState,
    buf: Vec<f32>,
}

impl Reader {
    fn new(domain: &Path, flow: &str) -> Reader {
        let mut generation = Generation::new(&api(), domain, 1).unwrap();
        let cfg = InputConfig {
            flow_id: flow,
            channels: 2,
            offset: 0,
        };
        let timing = Timing {
            block: B,
            margin: 0,
            max_catchup_blocks: 0,
        };
        generation.attach(0, &cfg, &timing).unwrap();
        Reader {
            generation,
            state: SlotState::default(),
            buf: vec![0.0; 2 * B as usize],
        }
    }

    /// A recent block, 100 ms behind the clock, when it was written:
    /// both legs, planar.
    fn recent(&mut self) -> Option<&[f32]> {
        let api = self.generation.api();
        let k = time::timestamp_to_index(api, time::now(api)) / B - 100;
        let input = self.generation.input(0).unwrap();
        let o = input.read(&mut self.state, k, &GroupWait::Skipped, &mut self.buf);
        (o == Outcome::Copy).then_some(&self.buf[..])
    }

    /// Waits until a whole block reads `left` and `right` exactly.
    async fn expect(&mut self, left: f32, right: f32, what: &str) {
        let n = B as usize;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = None;
        while Instant::now() < deadline {
            if let Some(b) = self.recent() {
                let (l, r) = b.split_at(n);
                if l.iter().all(|v| *v == left) && r.iter().all(|v| *v == right) {
                    return;
                }
                last = Some((l[n - 1], r[n - 1]));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{what}: expected ({left}, {right}), last read {last:?}");
    }
}

/// The program bus's left peak in the next meter frame, centi-dB.
async fn pgm_peak(server: &Server) -> i16 {
    let session = "session-m-0123456789abcdef";
    let url = format!("ws://{}/api/v1/ws?session={session}", server.api);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let hello = json!({ "type": "hello", "session": session, "kind": "ui", "mode": "primary" });
    ws.send(Message::text(hello.to_string())).await.unwrap();
    ws.send(Message::text(
        json!({ "type": "credit", "n": 8 }).to_string(),
    ))
    .await
    .unwrap();
    let mut frames = 0;
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("a message within 5 s")
            .unwrap()
            .unwrap();
        if let Message::Binary(f) = m {
            frames += 1;
            // The meters cover a 100 ms window: skip the first frames.
            if frames < 6 {
                continue;
            }
            assert_eq!(&f[..3], &[0x4d, 3, 2], "magic, version, replica id length");
            // After the 12-byte header and "r1": ch1 3 values, ch2 3, st1 5.
            let at = 14 + 2 * 11;
            return i16::from_le_bytes([f[at], f[at + 1]]);
        }
    }
}

/// Faders, pan, polarity, mute, routes and mix-minus exclusions reach the
/// output flows as gains of the input flows' samples, and the meters
/// follow the output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mix_reaches_the_output_flows_and_the_meters() {
    let domain = domain();
    let data = dir(Path::new(env!("CARGO_TARGET_TMPDIR")), "data");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let _ch1 = Source::start(&domain.0, &flows.ch1, vec![CH1]);
    let _st1 = Source::start(&domain.0, &flows.st1, ST1.to_vec());
    // ch2 reads a flow nobody writes: silent, never in the way.
    let server = start(&data.0, &domain.0, &flows).await;

    let deadline = Instant::now() + Duration::from_secs(10);
    while !domain.0.join(format!("{}.mxl-flow", flows.pgm)).exists() {
        assert!(Instant::now() < deadline, "the program output flow appears");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut pgm = Reader::new(&domain.0, &flows.pgm);
    let mut mm = Reader::new(&domain.0, &flows.mm);
    pgm.expect(0.0, 0.0, "every fader closed").await;

    // Hard left at 0 dB: cos 0 and sin 0 are exact.
    patch(
        &server,
        &[
            ("/channels/ch1/fader", json!(0)),
            ("/channels/ch1/pan/lr", json!(-1)),
        ],
    )
    .await;
    pgm.expect(CH1, 0.0, "ch1 open, hard left").await;
    mm.expect(CH1, 0.0, "the mix-minus follows the bus").await;

    patch(
        &server,
        &[
            ("/channels/st1/fader", json!(0)),
            ("/channels/st1/polarity/0", json!(true)),
        ],
    )
    .await;
    pgm.expect(CH1 - ST1[0], ST1[1], "st1 open, left leg inverted")
        .await;

    patch(&server, &[("/mixMinus/mm1/exclude", json!(["ch1"]))]).await;
    mm.expect(-ST1[0], ST1[1], "ch1 left out of the mix-minus")
        .await;

    patch(&server, &[("/channels/ch1/mute", json!(true))]).await;
    pgm.expect(-ST1[0], ST1[1], "ch1 muted").await;
    // -0.25: -12.04 dB FS.
    let peak = pgm_peak(&server).await;
    assert!(
        (-1206..=-1202).contains(&peak),
        "pgm left peak {peak} centi-dB"
    );

    patch(&server, &[("/channels/st1/routes/pgm", json!(false))]).await;
    pgm.expect(0.0, 0.0, "st1 off the program bus").await;

    server.stop().await;
    assert!(
        !domain.0.join(format!("{}.mxl-flow", flows.pgm)).exists(),
        "stopping deletes the output flows"
    );
}
