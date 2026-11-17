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
/// Input b of ch2.
const CH2B: f32 = 0.125;

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

/// An MXL domain on tmpfs, with a 1 s history, named for its test.
fn domain(name: &str) -> Dir {
    let shm = Path::new("/dev/shm");
    let d = dir(
        if shm.is_dir() {
            shm
        } else {
            Path::new(env!("CARGO_TARGET_TMPDIR"))
        },
        &format!("mxl-{name}"),
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
        let channels = values.len();
        Source::signal(domain, id, channels, move |ch, _| values[ch])
    }

    /// A sine of `freq` Hz at amplitude `amp`, exact per sample index.
    fn sine(domain: &Path, id: &str, amp: f32, freq: u64) -> Source {
        Source::signal(domain, id, 1, move |_, idx| {
            let phase = (freq * idx % 48_000) as f64 / 48_000.0;
            amp * (2.0 * std::f64::consts::PI * phase).sin() as f32
        })
    }

    /// A producer writing `signal(channel, index)` into every sample.
    fn signal(
        domain: &Path,
        id: &str,
        channels: usize,
        signal: impl Fn(usize, u64) -> f32 + Send + 'static,
    ) -> Source {
        let api = api();
        let instance = Instance::new(&api, domain).unwrap();
        let cfg = OutputConfig {
            flow_id: id,
            label: "source",
            group: "source",
            role: "Audio",
            channels: channels as u32,
            block: B,
            batch: B,
        };
        let Creation::Created { mut writer, .. } = output::create(&instance, &cfg).unwrap() else {
            panic!("source flow {id} not created");
        };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let n = B as usize;
            let mut data = vec![0.0f32; channels * n];
            let mut next: Option<u64> = None;
            while !flag.load(Ordering::Relaxed) {
                let idx = time::timestamp_to_index(&api, time::now(&api));
                if let Some(due) = (idx / B).checked_sub(1) {
                    for k in next.unwrap_or(due)..=due {
                        for (ch, leg) in data.chunks_exact_mut(n).enumerate() {
                            for (i, v) in leg.iter_mut().enumerate() {
                                *v = signal(ch, k * B + i as u64);
                            }
                        }
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
            { "id": "ch2", "name": "Pres 2", "width": 1, "layout": "mono", "altInput": true },
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
    ch2b: String,
    st1: String,
    pgm: String,
    mm: String,
}

async fn start(data: &Path, domain: &Path, f: &Flows) -> Server {
    let inputs = json!({
        "ch1": { "flow": f.ch1 },
        "ch2": { "flow": f.ch2, "altFlow": f.ch2b },
        "st1": { "flow": f.st1 },
    });
    let outputs = json!({ "out-pgm": { "flow": f.pgm }, "out-mm": { "flow": f.mm } });
    start_with(data, domain, shape(), inputs, outputs).await
}

async fn start_with(
    data: &Path,
    domain: &Path,
    shape: Value,
    inputs: Value,
    outputs: Value,
) -> Server {
    let listener = json!({ "bind": "127.0.0.1", "port": 0 });
    let config = json!({
        "listeners": { "api": listener, "ops": listener, "nmos": listener },
        "auth": { "mode": "none" },
        "api": { "allowedOrigins": ["http://faderz.test"] },
        "replica": { "name": "r1", "dataDir": data },
        "shape": shape,
        "mxl": {
            "libraryPath": libmxl(),
            "domains": [{ "name": "d", "path": domain }],
            "inputs": inputs,
            "outputs": outputs,
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

/// GET `path` from `addr`: status and body.
async fn get(addr: std::net::SocketAddr, path: &str) -> (u16, String) {
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    tcp.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    tcp.read_to_string(&mut answer).await.unwrap();
    let status = answer[9..12].parse().unwrap();
    (
        status,
        answer
            .split_once("\r\n\r\n")
            .map_or("", |(_, b)| b)
            .to_owned(),
    )
}

/// Waits for the output flow `id` to appear.
async fn appears(domain: &Path, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !domain.join(format!("{id}.mxl-flow")).exists() {
        assert!(Instant::now() < deadline, "output flow {id} appears");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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

    /// The largest magnitude of each leg over the 50 blocks (50 ms) up to
    /// 100 ms behind the clock, when all were written.
    fn peaks(&mut self) -> Option<(f32, f32)> {
        let api = self.generation.api();
        let last = time::timestamp_to_index(api, time::now(api)) / B - 100;
        let input = self.generation.input(0).unwrap();
        let n = B as usize;
        let mut peaks = (0.0f32, 0.0f32);
        for k in last - 49..=last {
            let o = input.read(&mut self.state, k, &GroupWait::Skipped, &mut self.buf);
            if o != Outcome::Copy {
                return None;
            }
            let (l, r) = self.buf.split_at(n);
            peaks.0 = l.iter().fold(peaks.0, |p, v| p.max(v.abs()));
            peaks.1 = r.iter().fold(peaks.1, |p, v| p.max(v.abs()));
        }
        Some(peaks)
    }

    /// The left leg of every block the flow holds, up to 100 ms behind the
    /// clock, oldest first.
    fn history(&mut self) -> Vec<f32> {
        let api = self.generation.api();
        let last = time::timestamp_to_index(api, time::now(api)) / B - 100;
        let input = self.generation.input(0).unwrap();
        let n = B as usize;
        let mut left = Vec::new();
        // A reader's own state refuses blocks before the last it read.
        let mut state = SlotState::default();
        // The domain keeps 1 s.
        for k in last - 900..=last {
            let o = input.read(&mut state, k, &GroupWait::Skipped, &mut self.buf);
            if o == Outcome::Copy {
                left.extend_from_slice(&self.buf[..n]);
            }
        }
        left
    }

    /// Waits until the left leg's peak is within `tol` of `want`.
    async fn expect_peak(&mut self, want: f32, tol: f32, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = None;
        while Instant::now() < deadline {
            if let Some((l, _)) = self.peaks() {
                if (l - want).abs() <= tol {
                    return;
                }
                last = Some(l);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{what}: expected a peak of {want} +/- {tol}, last read {last:?}");
    }

    /// Waits until a whole block reads `left` and `right`, within the
    /// rounding of an f32 gain.
    async fn expect(&mut self, left: f32, right: f32, what: &str) {
        let n = B as usize;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = None;
        while Instant::now() < deadline {
            if let Some(b) = self.recent() {
                let (l, r) = b.split_at(n);
                let near = |v: &f32, want: f32| (v - want).abs() <= 1e-6;
                if l.iter().all(|v| near(v, left)) && r.iter().all(|v| near(v, right)) {
                    return;
                }
                last = Some((l[n - 1], r[n - 1]));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{what}: expected ({left}, {right}), last read {last:?}");
    }
}

/// The values of a meter frame after `skip` frames, centi-dB.
async fn meter_values(server: &Server, skip: usize) -> Vec<i16> {
    let session = "session-m-0123456789abcdef";
    let url = format!("ws://{}/api/v1/ws?session={session}", server.api);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let hello = json!({ "type": "hello", "session": session, "kind": "ui", "mode": "primary" });
    ws.send(Message::text(hello.to_string())).await.unwrap();
    ws.send(Message::text(
        json!({ "type": "credit", "n": 16 }).to_string(),
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
            if frames <= skip {
                continue;
            }
            assert_eq!(&f[..3], &[0x4d, 3, 2], "magic, version, replica id length");
            // After the 12-byte header and "r1".
            return f[14..]
                .chunks_exact(2)
                .map(|v| i16::from_le_bytes([v[0], v[1]]))
                .collect();
        }
    }
}

/// Values of the meter frame: ch1 3, ch2 3, st1 5, then pgm 14, mm1 14
/// and the monitor 2.
const CH1_PEAK: usize = 0;
const CH2_PEAK: usize = 3;
const ST1_PEAK: usize = 6;
const PGM_PEAK: usize = 11;
const MON_PEAK: usize = 39;
/// A level of digital silence, -120 dB.
const FLOOR: i16 = -12_000;

/// Faders, pan, polarity, mute, routes and mix-minus exclusions reach the
/// output flows as gains of the input flows' samples, and the meters
/// follow the output, every channel at its meter tap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mix_reaches_the_output_flows_and_the_meters() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let domain = domain("mix");
    let data = dir(Path::new(env!("CARGO_TARGET_TMPDIR")), "data");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let _ch1 = Source::start(&domain.0, &flows.ch1, vec![CH1]);
    let _ch2b = Source::start(&domain.0, &flows.ch2b, vec![CH2B]);
    let _st1 = Source::start(&domain.0, &flows.st1, ST1.to_vec());
    // ch2's input a reads a flow nobody writes: silent.
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.pgm).await;
    let mut pgm = Reader::new(&domain.0, &flows.pgm);
    let mut mm = Reader::new(&domain.0, &flows.mm);
    pgm.expect(0.0, 0.0, "every fader closed").await;
    // ch2's input a has no flow: not attached, while ch1's is.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, metrics) = get(server.ops, "/metrics").await;
        if metrics.contains("faderz_input_attached{channel=\"ch1\"} 1\n")
            && metrics.contains("faderz_input_attached{channel=\"ch2\"} 0\n")
        {
            break;
        }
        assert!(Instant::now() < deadline, "input health: {metrics}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

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
    // The meters cover a 100 ms window: the first frames are skipped.
    let v = meter_values(&server, 5).await;
    // -0.25: -12.04 dB FS.
    let near = |v: i16, want: i16| (v - want).abs() <= 2;
    assert!(near(v[PGM_PEAK], -1204), "pgm left peak in {v:?}");
    // Every channel is metered, no direct out needed: ch1 muted but at
    // its pre-fader tap, st1 at 0.25 on both legs, and ch2 on its silent
    // input reads silence, not nothing. The monitor has no output.
    assert!(near(v[CH1_PEAK], -602), "ch1 peak in {v:?}");
    assert!(
        near(v[ST1_PEAK], -1204) && near(v[ST1_PEAK + 1], -1204),
        "{v:?}"
    );
    assert_eq!(v[CH2_PEAK], FLOOR, "{v:?}");
    assert!(v[MON_PEAK] != i16::MIN, "{v:?}");
    // At a post-fader tap the muted channel reads silence.
    patch(&server, &[("/channels/ch1/meterTap", json!("postFader"))]).await;
    let v = meter_values(&server, 5).await;
    assert_eq!(v[CH1_PEAK], FLOOR, "ch1 peak post-fader in {v:?}");

    patch(&server, &[("/channels/st1/routes/pgm", json!(false))]).await;
    pgm.expect(0.0, 0.0, "st1 off the program bus").await;

    patch(
        &server,
        &[
            ("/channels/ch2/fader", json!(0)),
            ("/channels/ch2/pan/lr", json!(-1)),
        ],
    )
    .await;
    pgm.expect(0.0, 0.0, "ch2 open on its silent input a").await;
    patch(&server, &[("/channels/ch2/input", json!("b"))]).await;
    pgm.expect(CH2B, 0.0, "ch2 re-routed to input b").await;
    patch(&server, &[("/channels/ch2/input", json!("a"))]).await;
    pgm.expect(0.0, 0.0, "ch2 back on input a").await;
    patch(&server, &[("/channels/ch2/input", json!("b"))]).await;
    pgm.expect(CH2B, 0.0, "ch2 on input b again").await;

    // An entry the engine dropped: the engine plays a gain the state does
    // not have, until everything is sent again.
    let audio = server.audio.clone().unwrap();
    let api = api();
    let mut at = (time::timestamp_to_index(&api, time::now(&api)) + 4_800).next_multiple_of(B);
    while let Err(e) = audio.engine().set_gain(1, 0, 0.0, at) {
        assert!(matches!(e, samplez::GainError::TooNear { .. }), "{e}");
        at += 4_800;
    }
    let trim = samplez_dsp::StripParam::Trim { db: -20.0 };
    audio.engine().set_strip(1, trim, at).unwrap();
    pgm.expect(0.0, 0.0, "the engine diverged from the state")
        .await;
    audio.resend();
    pgm.expect(CH2B, 0.0, "a resend restores the state's gains and strips")
        .await;
    let (_, metrics) = get(server.ops, "/metrics").await;
    assert!(
        metrics.contains("faderz_engine_resends_total 1"),
        "{metrics}"
    );
    // Each channel's input health: ch1 read, ch2 on input b now and read
    // too, st1 read; their copied blocks counted.
    for ch in ["ch1", "ch2", "st1"] {
        let attached = format!("faderz_input_attached{{channel=\"{ch}\"}} 1\n");
        assert!(metrics.contains(&attached), "{ch}: {metrics}");
        let copied = format!("faderz_input_copied_blocks_total{{channel=\"{ch}\"}} ");
        let n: u64 = metrics
            .lines()
            .find_map(|l| l.strip_prefix(&copied))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{ch}: {metrics}"));
        assert!(n > 0, "{ch}: {metrics}");
    }
    assert!(
        metrics.contains("faderz_input_margin_needed_samples{channel=\"ch1\"} 0\n"),
        "{metrics}"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while get(server.ops, "/readyz").await.0 != 200 {
        assert!(Instant::now() < deadline, "ready again once resent");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Trimmed and inverted before the restart: the restarted engine plays
    // ch2 so from its first sample on.
    patch(
        &server,
        &[
            ("/channels/ch2/trim", json!(-6)),
            ("/channels/ch2/polarity/0", json!(true)),
        ],
    )
    .await;
    let trimmed = -CH2B * 10f32.powf(-6.0 / 20.0);
    pgm.expect(trimmed, 0.0, "ch2 trimmed and inverted").await;

    drop(audio);
    server.stop().await;
    assert!(
        !domain.0.join(format!("{}.mxl-flow", flows.pgm)).exists(),
        "stopping deletes the output flows"
    );

    // A restart reads the selected input b from its first block.
    let server = start(&data.0, &domain.0, &flows).await;
    let inputs = server.audio.as_ref().unwrap().engine().status().inputs;
    assert_eq!(inputs[1].flow_id, flows.ch2b, "ch2 starts on input b");
    appears(&domain.0, &flows.pgm).await;
    let mut pgm = Reader::new(&domain.0, &flows.pgm);
    pgm.expect(trimmed, 0.0, "the restarted state on input b")
        .await;
    // Every sample since the output began: silence, then the fade-in of
    // the processed signal, never the unprocessed one.
    let left = pgm.history();
    let first = left.iter().position(|v| *v != 0.0);
    // Silence before it: the history reaches back to the output's start.
    assert!(
        first.is_some_and(|i| i > 0),
        "the history holds the silent start and the fade-in: first non-zero at {first:?}"
    );
    let wrong = left.iter().find(|v| **v > 0.0 || **v < trimmed - 1e-6);
    assert!(wrong.is_none(), "unprocessed after the restart: {wrong:?}");
    server.stop().await;
}

/// Each channel reads the legs its `channels` select out of a wider
/// flow, each channel of which carries its own value: ch1 flow channel
/// 16 and st1 flow channels 7 and 8 of one 16-channel flow, and ch2 flow
/// channel 3 of its input a and, once switched, of its input b.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_read_the_legs_they_select_from_a_wide_flow() {
    let domain = domain("select");
    let data = dir(Path::new(env!("CARGO_TARGET_TMPDIR")), "data-select");
    let (a, b, pgm_id, mm_id) = (new_id(), new_id(), new_id(), new_id());
    let va = |ch: usize| (ch + 1) as f32 / 64.0;
    let vb = |ch: usize| -((ch + 1) as f32) / 128.0;
    let _a = Source::start(&domain.0, &a, (0..16).map(va).collect());
    let _b = Source::start(&domain.0, &b, (0..16).map(vb).collect());
    let inputs = json!({
        "ch1": { "flow": a, "channels": [16] },
        "ch2": { "flow": a, "altFlow": b, "channels": [3] },
        "st1": { "flow": a, "channels": [7, 8] },
    });
    let outputs = json!({ "out-pgm": { "flow": pgm_id }, "out-mm": { "flow": mm_id } });
    let server = start_with(&data.0, &domain.0, shape(), inputs, outputs).await;
    appears(&domain.0, &pgm_id).await;
    let mut pgm = Reader::new(&domain.0, &pgm_id);
    patch(&server, &[("/channels/st1/fader", json!(0))]).await;
    pgm.expect(va(6), va(7), "st1 on flow channels 7 and 8")
        .await;
    patch(
        &server,
        &[
            ("/channels/ch1/fader", json!(0)),
            ("/channels/ch1/pan/lr", json!(-1)),
            ("/channels/ch2/fader", json!(0)),
            ("/channels/ch2/pan/lr", json!(1)),
        ],
    )
    .await;
    pgm.expect(va(6) + va(15), va(7) + va(2), "ch1 on 16, ch2 on 3")
        .await;
    patch(&server, &[("/channels/ch2/input", json!("b"))]).await;
    pgm.expect(va(6) + va(15), va(7) + vb(2), "ch2 on 3 of input b")
        .await;
    server.stop().await;
}

/// A diff larger than the changes the engine holds at once waits for
/// room and lands whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_diff_beyond_the_engines_queue_lands_whole() {
    const N: usize = 40;
    let domain = domain("wide");
    let data = dir(Path::new(env!("CARGO_TARGET_TMPDIR")), "data-wide");
    let ids: Vec<String> = (0..N).map(|_| new_id()).collect();
    let pgm_id = new_id();
    let mut shape = shape();
    shape["channels"] = (0..N)
        .map(|i| json!({ "id": format!("c{i}"), "name": "c", "width": 1, "layout": "mono", "altInput": false }))
        .collect();
    shape["mixMinus"] = json!([]);
    shape["outputs"] = json!([
        { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
    ]);
    let inputs: serde_json::Map<String, Value> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (format!("c{i}"), json!({ "flow": id })))
        .collect();
    let _c0 = Source::start(&domain.0, &ids[0], vec![CH1]);
    let _c1 = Source::start(&domain.0, &ids[1], vec![CH2B]);
    let server = start_with(
        &data.0,
        &domain.0,
        shape,
        Value::Object(inputs),
        json!({ "out-pgm": { "flow": pgm_id } }),
    )
    .await;
    appears(&domain.0, &pgm_id).await;
    let mut pgm = Reader::new(&domain.0, &pgm_id);
    pgm.expect(0.0, 0.0, "every fader closed").await;
    // Centre pan: 2 N coefficients, beyond the engine's 64.
    let paths: Vec<String> = (0..N).map(|i| format!("/channels/c{i}/fader")).collect();
    let changes: Vec<(&str, Value)> = paths.iter().map(|p| (p.as_str(), json!(0))).collect();
    patch(&server, &changes).await;
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let want = (CH1 + CH2B) * h;
    pgm.expect(want, want, "every channel open").await;
    server.stop().await;
}

/// Each channel's strip processes its input: the high-pass takes out a
/// low tone, an EQ bell lifts its centre frequency, polarity inverts one
/// leg, the gate closes below its threshold and the compressor reduces
/// above it, each by exactly its settled gain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_strip_processes_each_channel() {
    let domain = domain("strip");
    let data = dir(Path::new(env!("CARGO_TARGET_TMPDIR")), "data-strip");
    let ids: Vec<String> = (0..6).map(|_| new_id()).collect();
    let mono =
        |id: &str| json!({ "id": id, "name": id, "width": 1, "layout": "mono", "altInput": false });
    let mut shape = shape();
    shape["channels"] = json!([
        mono("hpf"),
        mono("eq"),
        mono("gate"),
        mono("comp"),
        { "id": "st1", "name": "Music", "width": 2, "layout": "stereo", "altInput": false },
    ]);
    shape["mixMinus"] = json!([]);
    shape["outputs"] = json!([
        { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
    ]);
    let inputs = json!({
        "hpf": { "flow": ids[0] },
        "eq": { "flow": ids[1] },
        "gate": { "flow": ids[2] },
        "comp": { "flow": ids[3] },
        "st1": { "flow": ids[4] },
    });
    // 40 Hz and 1 kHz tones, -40 dBFS and -6 dBFS steady levels.
    let _hpf = Source::sine(&domain.0, &ids[0], 0.5, 40);
    let _eq = Source::sine(&domain.0, &ids[1], 0.1, 1_000);
    let _gate = Source::start(&domain.0, &ids[2], vec![0.01]);
    let _comp = Source::start(&domain.0, &ids[3], vec![0.5]);
    let _st1 = Source::start(&domain.0, &ids[4], ST1.to_vec());
    let server = start_with(
        &data.0,
        &domain.0,
        shape,
        inputs,
        json!({ "out-pgm": { "flow": ids[5] } }),
    )
    .await;
    appears(&domain.0, &ids[5]).await;
    let mut pgm = Reader::new(&domain.0, &ids[5]);
    // One channel at a time, hard left at 0 dB.
    let open = |id: &str, fader: i32| {
        [
            (format!("/channels/{id}/fader"), json!(fader)),
            (format!("/channels/{id}/pan/lr"), json!(-1)),
        ]
    };
    fn set(changes: &[(String, Value)]) -> Vec<(&str, Value)> {
        changes
            .iter()
            .map(|(p, v)| (p.as_str(), v.clone()))
            .collect()
    }

    patch(&server, &set(&open("hpf", 0))).await;
    pgm.expect_peak(0.5, 0.005, "the 40 Hz tone, high-pass off")
        .await;
    patch(
        &server,
        &[
            ("/channels/hpf/hpf/enabled", json!(true)),
            ("/channels/hpf/hpf/freq", json!(500)),
        ],
    )
    .await;
    // 12 dB per octave from 500 Hz: about -44 dB at 40 Hz.
    pgm.expect_peak(0.0, 0.005, "the 40 Hz tone under a 500 Hz high-pass")
        .await;
    patch(&server, &set(&open("hpf", -90))).await;

    patch(&server, &set(&open("eq", 0))).await;
    pgm.expect_peak(0.1, 0.001, "the 1 kHz tone, EQ off").await;
    patch(
        &server,
        &[
            ("/channels/eq/eq/enabled", json!(true)),
            ("/channels/eq/eq/bands/1/type", json!("peak")),
            ("/channels/eq/eq/bands/1/freq", json!(1000)),
            ("/channels/eq/eq/bands/1/gain", json!(12)),
            ("/channels/eq/eq/bands/1/q", json!(1)),
        ],
    )
    .await;
    // A bell has no phase shift at its centre: the samples scale by +12 dB.
    let lift = 10f32.powf(12.0 / 20.0);
    pgm.expect_peak(0.1 * lift, 0.002, "a +12 dB bell at 1 kHz")
        .await;
    // A near-limiter at -20 dBFS: after the bell it takes the tone down
    // by about 11 dB; before it, the tone only reaches its threshold.
    patch(
        &server,
        &[
            ("/channels/eq/comp/enabled", json!(true)),
            ("/channels/eq/comp/threshold", json!(-20)),
            ("/channels/eq/comp/ratio", json!(20)),
            ("/channels/eq/comp/attack", json!(0.1)),
            ("/channels/eq/comp/release", json!(50)),
        ],
    )
    .await;
    pgm.expect_peak(0.11, 0.03, "the compressor after the EQ")
        .await;
    patch(&server, &[("/channels/eq/dynamicsBeforeEq", json!(true))]).await;
    pgm.expect_peak(0.1 * lift, 0.002, "the compressor before the EQ")
        .await;
    patch(&server, &set(&open("eq", -90))).await;

    patch(&server, &set(&open("gate", 0))).await;
    pgm.expect(0.01, 0.0, "-40 dBFS, gate off").await;
    patch(
        &server,
        &[
            ("/channels/gate/gate/enabled", json!(true)),
            ("/channels/gate/gate/threshold", json!(-30)),
            ("/channels/gate/gate/range", json!(20)),
            ("/channels/gate/gate/attack", json!(1)),
            ("/channels/gate/gate/release", json!(50)),
        ],
    )
    .await;
    pgm.expect(0.001, 0.0, "-40 dBFS under a -30 dBFS gate, 20 dB range")
        .await;
    patch(&server, &set(&open("gate", -90))).await;

    patch(&server, &set(&open("comp", 0))).await;
    pgm.expect(0.5, 0.0, "-6 dBFS, compressor off").await;
    patch(
        &server,
        &[
            ("/channels/comp/comp/enabled", json!(true)),
            ("/channels/comp/comp/threshold", json!(-20)),
            ("/channels/comp/comp/ratio", json!(4)),
            ("/channels/comp/comp/attack", json!(1)),
            ("/channels/comp/comp/release", json!(50)),
            ("/channels/comp/comp/makeup", json!(0)),
        ],
    )
    .await;
    // 13.98 dB over the threshold at 4:1 leaves 3.49 dB over it.
    let level = 20.0 * 0.5f32.log10();
    let want = 0.5 * 10f32.powf((level + 20.0) * (0.25 - 1.0) / 20.0);
    pgm.expect_peak(want, 1e-4, "-6 dBFS into 4:1 from -20 dBFS")
        .await;
    patch(&server, &set(&open("comp", -90))).await;

    patch(&server, &[("/channels/st1/fader", json!(0))]).await;
    pgm.expect(ST1[0], ST1[1], "st1 as it is").await;
    patch(&server, &[("/channels/st1/polarity/1", json!(true))]).await;
    pgm.expect(ST1[0], -ST1[1], "st1's right leg inverted")
        .await;

    server.stop().await;
}
