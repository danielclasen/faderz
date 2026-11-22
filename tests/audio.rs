//! faderz mixing real audio: constant signals written into MXL flows on a
//! tmpfs domain, controls set over the REST API, and the program bus read
//! back from its output flow and its meters.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use faderz::config::Config;
use faderz::server::Server;
use futures_util::{SinkExt, StreamExt};
use samplez_mxl::input::{Generation, GroupWait, InputConfig, Outcome, SlotState, Timing};
use samplez_mxl::time;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

mod common;
use common::*;

/// ch1 and the two legs of st1, as written into their flows.
const CH1: f32 = 0.5;
const ST1: [f32; 2] = [0.25, -0.25];
/// Input b of ch2.
const CH2B: f32 = 0.125;

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
    start_engine(data, domain, shape, inputs, outputs, json!({})).await
}

/// [`start_with`] with the `engine` settings `engine`.
async fn start_engine(
    data: &Path,
    domain: &Path,
    shape: Value,
    inputs: Value,
    outputs: Value,
    engine: Value,
) -> Server {
    let listener = json!({ "bind": "127.0.0.1", "port": 0 });
    let config = json!({
        "engine": engine,
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
        Reader::wide(domain, flow, 2)
    }

    /// A reader of a flow of `channels` legs.
    fn wide(domain: &Path, flow: &str, channels: u32) -> Reader {
        let mut generation = Generation::new(&api(), domain, 1).unwrap();
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
        generation.attach(0, &cfg, &timing).unwrap();
        Reader {
            generation,
            state: SlotState::default(),
            buf: vec![0.0; channels as usize * B as usize],
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

    /// The block 100 ms behind libmxl's clock, written by now.
    fn behind(&self) -> u64 {
        let api = self.generation.api();
        time::timestamp_to_index(api, time::now(api)) / B - 100
    }

    /// The left leg of every block from block `from` on, in order, until
    /// a whole block reads `left` within the rounding of an f32 gain,
    /// within 10 s. A block not written is left out.
    async fn trace(&mut self, from: u64, left: f32) -> Vec<f32> {
        let n = B as usize;
        let mut state = SlotState::default();
        let mut trace = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut k = from;
        loop {
            let last = self.behind();
            let input = self.generation.input(0).unwrap();
            while k <= last {
                if input.read(&mut state, k, &GroupWait::Skipped, &mut self.buf) == Outcome::Copy {
                    let block = &self.buf[..n];
                    trace.extend_from_slice(block);
                    if block.iter().all(|v| (v - left).abs() <= 1e-6) {
                        return trace;
                    }
                }
                k += 1;
            }
            let end = trace.last();
            assert!(
                Instant::now() < deadline,
                "{left} not reached, last read {end:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
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
            assert_eq!(&f[..3], &[0x4d, 4, 2], "magic, version, replica id length");
            // After the 12-byte header and "r1".
            return f[14..]
                .chunks_exact(2)
                .map(|v| i16::from_le_bytes([v[0], v[1]]))
                .collect();
        }
    }
}

/// The values of the first meter frame of which `done` holds, within
/// 10 s: a meter reads a change once the 100 ms window it falls in is
/// over, and a momentary loudness once 400 ms were metered without a
/// slice missed since.
async fn meters_until(server: &Server, what: &str, done: impl Fn(&[i16]) -> bool) -> Vec<i16> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let v = meter_values(server, 1).await;
        if done(&v) {
            return v;
        }
        assert!(Instant::now() < deadline, "{what}: {v:?}");
    }
}

/// Values of the meter frame: ch1 3, ch2 3, st1 5, then pgm 14, mm1 14
/// and the monitor 2; the shape has no duck group.
const CH1_PEAK: usize = 0;
/// A channel's peaks are followed by its momentary loudness.
const CH1_MOMENTARY: usize = 2;
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
    let data = data("mix");
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
    // The engine creates the output flows one after another: out-mm can
    // appear after out-pgm.
    appears(&domain.0, &flows.pgm).await;
    appears(&domain.0, &flows.mm).await;
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
    // The status names each channel's input and each output as the
    // engine reports them, read once a second.
    let deadline = Instant::now() + Duration::from_secs(5);
    let engine = loop {
        let (code, body) = get(server.api, "/api/v1/status").await;
        assert_eq!(code, 200, "{body}");
        let status: Value = serde_json::from_str(&body).unwrap();
        let engine = status["engine"].clone();
        if engine["inputs"][0]["copiedBlocks"].as_u64() > Some(0) {
            break engine;
        }
        assert!(Instant::now() < deadline, "status: {status}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let inputs = engine["inputs"].as_array().unwrap();
    let names: Vec<&str> = inputs
        .iter()
        .map(|i| i["channel"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["ch1", "ch2", "st1"]);
    assert_eq!(
        (&inputs[0]["state"], &inputs[0]["flowId"]),
        (&json!("attached"), &json!(flows.ch1))
    );
    assert_eq!(inputs[1]["copiedBlocks"], 0, "ch2's input a has no flow");
    assert_eq!(
        engine["outputs"],
        json!([
            { "output": "out-pgm", "enabled": true, "realised": true, "reason": null },
            { "output": "out-mm", "enabled": true, "realised": true, "reason": null },
        ])
    );

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
    patch(&server, &[("/channels/ch1/meterTap", json!("preFader"))]).await;
    // -0.25: -12.04 dB FS.
    let near = |v: i16, want: i16| (v - want).abs() <= 2;
    // Every channel is metered, no direct out needed: ch1 muted but at
    // its pre-fader tap, st1 at 0.25 on both legs, and ch2 on its silent
    // input reads silence, not nothing; ch1's loudness is measured. The
    // monitor has no output.
    meters_until(&server, "the meters at ch1's pre-fader tap", |v| {
        near(v[PGM_PEAK], -1204)
            && near(v[CH1_PEAK], -602)
            && near(v[ST1_PEAK], -1204)
            && near(v[ST1_PEAK + 1], -1204)
            && v[CH2_PEAK] == FLOOR
            && v[MON_PEAK] != i16::MIN
            && v[CH1_MOMENTARY] != i16::MIN
            && v[CH1_MOMENTARY] != FLOOR
    })
    .await;
    // At a post-fader tap the muted channel reads silence, its loudness
    // too.
    patch(&server, &[("/channels/ch1/meterTap", json!("postFader"))]).await;
    meters_until(&server, "ch1 muted at its post-fader tap", |v| {
        v[CH1_PEAK] == FLOOR && v[CH1_MOMENTARY] == FLOOR
    })
    .await;

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
    // Every sample since the output began: silence, then the processed
    // signal, never the unprocessed one.
    let left = pgm.history();
    let first = left.iter().position(|v| *v != 0.0);
    // Silence before it: the history reaches back to the output's start.
    assert!(
        first.is_some_and(|i| i > 0),
        "the history holds the silent start: first non-zero at {first:?}"
    );
    let wrong = left.iter().find(|v| **v > 0.0 || **v < trimmed - 1e-6);
    assert!(wrong.is_none(), "unprocessed after the restart: {wrong:?}");
    server.stop().await;
}

/// With no output carrying the monitor, its meter is a matrix column of
/// its own, and it follows a solo as an output would: the PFL of ch1,
/// whose fader is closed, at -3 dB centred, then the source again once
/// every solo is off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_solo_reaches_the_monitor_meter() {
    let domain = domain("solo-meter");
    let data = data("solo-meter");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let _ch1 = Source::start(&domain.0, &flows.ch1, vec![CH1]);
    let _st1 = Source::start(&domain.0, &flows.st1, ST1.to_vec());
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.pgm).await;
    let near = |v: i16, want: i16| (v - want).abs() <= 2;
    // A patch before the engine is ready is refused with 503.
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(server.ops, "/readyz").await.0 != 200 {
        assert!(Instant::now() < deadline, "ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    patch(
        &server,
        &[
            ("/monitor/level", json!(0)),
            ("/channels/st1/fader", json!(0)),
        ],
    )
    .await;
    // The meter windows are 100 ms: a change shows once the window it
    // falls in is over, so each reading waits for its value.
    let reads = |what: &'static str, want: i16| {
        let server = &server;
        async move {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let v = meter_values(server, 1).await;
                if near(v[MON_PEAK], want) {
                    return;
                }
                assert!(Instant::now() < deadline, "{what} ({want}) in {v:?}");
            }
        }
    };
    reads("the source, pgm", -1204).await;
    patch(&server, &[("/channels/ch1/solo", json!("pfl"))]).await;
    reads("ch1 PFL", -903).await;
    patch(&server, &[("/channels/ch1/solo", json!("off"))]).await;
    reads("the source again", -1204).await;
    server.stop().await;
}

/// Frames until `done` holds of the meter values, or a minute passes.
async fn until_values(server: &Server, what: &str, done: impl Fn(&[i16]) -> bool) -> Vec<i16> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let v = meter_values(server, 1).await;
        if done(&v) {
            return v;
        }
        assert!(Instant::now() < deadline, "{what}: {v:?}");
    }
}

/// POST /loudness/reset restarts the loudness measurement of an output:
/// the highest momentary loudness since the last reset falls to the
/// level the output has now, from the louder one it had.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loudness_reset_restarts_the_measurement() {
    const MOMENTARY: usize = PGM_PEAK + 6;
    const MAX_MOMENTARY: usize = PGM_PEAK + 10;
    let domain = domain("loud-reset");
    let data = data("loud-reset");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    // A tone: a constant is no loudness, the K-weighting removes it.
    let _ch1 = Source::sine(&domain.0, &flows.ch1, 0.5, 440);
    let _st1 = Source::start(&domain.0, &flows.st1, ST1.to_vec());
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.pgm).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(server.ops, "/readyz").await.0 != 200 {
        assert!(Instant::now() < deadline, "ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    patch(&server, &[("/channels/ch1/fader", json!(0))]).await;
    // Not the integrated loudness: a meter slice the engine missed on a
    // starved host leaves it null until the next reset, by design.
    until_values(&server, "loud, measured", |v| v[MAX_MOMENTARY] != i16::MIN).await;
    // A steady tone: the maximum is settled when the momentary loudness
    // has reached it.
    let v = until_values(&server, "the maximum settles", |v| {
        v[MOMENTARY] != i16::MIN && v[MAX_MOMENTARY] - v[MOMENTARY] <= 30
    })
    .await;
    let loud = v[MAX_MOMENTARY];
    // 20 dB down: the maximum holds, the momentary level follows.
    patch(&server, &[("/channels/ch1/fader", json!(-20))]).await;
    let v = until_values(&server, "20 dB down", |v| {
        v[MOMENTARY] != i16::MIN && v[MOMENTARY] <= loud - 1800
    })
    .await;
    assert!(v[MAX_MOMENTARY] >= loud, "the maximum holds: {v:?}");
    let (status, body) = post(server.api, "/loudness/reset", &json!({ "ids": ["pgm"] })).await;
    assert_eq!(status, 204, "{body}");
    let v = until_values(&server, "the maximum restarts", |v| {
        v[MAX_MOMENTARY] != i16::MIN && v[MAX_MOMENTARY] <= loud - 1500
    })
    .await;
    assert!(v[MAX_MOMENTARY] <= loud - 1500, "{v:?}");
    let (status, _) = post(server.api, "/loudness/reset", &json!({ "ids": ["ch1"] })).await;
    assert_eq!(status, 404, "a channel has no loudness measurement");
    server.stop().await;
}

/// A measurement paused (`/loudness/<id>/running` false) takes in no
/// audio: the integrated loudness holds while the output gets 6 dB
/// louder, and moves towards the louder level once it runs again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paused_loudness_measurement_holds_until_it_runs_again() {
    const MOMENTARY: usize = PGM_PEAK + 6;
    const INTEGRATED: usize = PGM_PEAK + 8;
    let domain = domain("loud-pause");
    let data = data("loud-pause");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let _ch1 = Source::sine(&domain.0, &flows.ch1, 0.5, 440);
    let _st1 = Source::start(&domain.0, &flows.st1, ST1.to_vec());
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.pgm).await;
    patch(&server, &[("/channels/ch1/fader", json!(-6))]).await;
    let v = until_values(&server, "measured", |v| {
        v[INTEGRATED] != i16::MIN && v[MOMENTARY] != i16::MIN
    })
    .await;
    let quiet = v[MOMENTARY];
    // Paused, then 6 dB louder: once the momentary loudness, measured
    // whether running or not, shows it, the integrated loudness holds
    // over 75 frames.
    patch(&server, &[("/loudness/pgm/running", json!(false))]).await;
    patch(&server, &[("/channels/ch1/fader", json!(0))]).await;
    let v = until_values(&server, "6 dB louder", |v| {
        v[MOMENTARY] != i16::MIN && v[MOMENTARY] >= quiet + 550
    })
    .await;
    let held = v[INTEGRATED];
    for _ in 0..75 {
        let v = meter_values(&server, 1).await;
        assert_eq!(v[INTEGRATED], held, "paused: {v:?}");
    }
    patch(&server, &[("/loudness/pgm/running", json!(true))]).await;
    until_values(&server, "running again", |v| v[INTEGRATED] >= held + 100).await;
    server.stop().await;
}

/// A stereo channel's image and balance reach the programme flow: the
/// legs it feeds both, or swapped, and the opposite leg lowered by its
/// pan, which a mono channel's pan does not do.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stereo_channels_image_and_balance_reach_the_programme() {
    let domain = domain("image");
    let data = data("image");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let _st1 = Source::start(&domain.0, &flows.st1, ST1.to_vec());
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.pgm).await;
    let mut pgm = Reader::new(&domain.0, &flows.pgm);
    patch(&server, &[("/channels/st1/fader", json!(0))]).await;
    pgm.expect(ST1[0], ST1[1], "normal").await;
    for (image, l, r) in [
        ("swap", ST1[1], ST1[0]),
        ("leftToBoth", ST1[0], ST1[0]),
        ("rightToBoth", ST1[1], ST1[1]),
    ] {
        patch(&server, &[("/channels/st1/image", json!(image))]).await;
        pgm.expect(l, r, image).await;
    }
    patch(
        &server,
        &[
            ("/channels/st1/image", json!("normal")),
            ("/channels/st1/pan/lr", json!(-1)),
        ],
    )
    .await;
    pgm.expect(ST1[0], 0.0, "balanced hard left").await;
    patch(&server, &[("/channels/st1/pan/lr", json!(0.5))]).await;
    pgm.expect(ST1[0] * 0.5, ST1[1], "balanced half right")
        .await;
    server.stop().await;
}

/// Channel and VCA PFL and AFL reach the monitor output flow in place
/// of its source, PFL over AFL, and every solo off gives the source back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn solos_reach_the_monitor_output_flow() {
    let domain = domain("solo");
    let data = data("solo");
    let (ch1, ch2, ch2b, st1) = (new_id(), new_id(), new_id(), new_id());
    let (pgm_id, mm_id, mon_id) = (new_id(), new_id(), new_id());
    let _ch1 = Source::start(&domain.0, &ch1, vec![CH1]);
    let _st1 = Source::start(&domain.0, &st1, ST1.to_vec());
    let mut v = shape();
    v["vcas"] = json!([{ "id": "vca1", "name": "Music" }]);
    v["outputs"].as_array_mut().unwrap().push(
        json!({ "id": "out-mon", "name": "Mon out", "source": "mon", "width": 2, "layout": "stereo" }),
    );
    let inputs = json!({
        "ch1": { "flow": ch1 },
        "ch2": { "flow": ch2, "altFlow": ch2b },
        "st1": { "flow": st1 },
    });
    let outputs = json!({
        "out-pgm": { "flow": pgm_id },
        "out-mm": { "flow": mm_id },
        "out-mon": { "flow": mon_id },
    });
    let server = start_with(&data.0, &domain.0, v, inputs, outputs).await;
    appears(&domain.0, &mon_id).await;
    let mut mon = Reader::new(&domain.0, &mon_id);
    // A patch before the engine is ready is refused with 503.
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(server.ops, "/readyz").await.0 != 200 {
        assert!(Instant::now() < deadline, "ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let pfl = CH1 * std::f32::consts::FRAC_1_SQRT_2;
    patch(
        &server,
        &[
            ("/monitor/level", json!(0)),
            ("/channels/st1/fader", json!(0)),
        ],
    )
    .await;
    mon.expect(ST1[0], ST1[1], "the source, pgm").await;
    patch(&server, &[("/channels/ch1/solo", json!("pfl"))]).await;
    mon.expect(pfl, pfl, "ch1 PFL with its fader closed").await;
    // The monitor meter reads the meters of the output carrying it.
    let v = meter_values(&server, 5).await;
    assert!((v[MON_PEAK] + 903).abs() <= 2, "monitor peak in {v:?}");
    patch(&server, &[("/channels/ch1/solo", json!("afl"))]).await;
    mon.expect(0.0, 0.0, "ch1 AFL with its fader closed").await;
    patch(&server, &[("/channels/ch1/solo", json!("off"))]).await;
    mon.expect(ST1[0], ST1[1], "the source again").await;
    // The AFV stage lowers st1 on the source once the tally closes it.
    patch(
        &server,
        &[
            ("/afv/st1/enabled", json!(true)),
            ("/afv/st1/offLevel", json!(-40)),
            ("/afv/st1/source", json!("cam1")),
        ],
    )
    .await;
    tally(&server, "cam1", "preview").await;
    mon.expect(
        ST1[0] * 0.01,
        ST1[1] * 0.01,
        "the source with st1's AFV closed",
    )
    .await;
    // AFL is after the fader, not after the routes.
    patch(&server, &[("/channels/st1/routes/pgm", json!(false))]).await;
    mon.expect(0.0, 0.0, "st1 off the source").await;
    patch(
        &server,
        &[
            ("/vcas/vca1/members", json!(["st1"])),
            ("/vcas/vca1/solo", json!("afl")),
        ],
    )
    .await;
    // Before the AFV stage: its closed AFV does not lower it.
    mon.expect(ST1[0], ST1[1], "st1 AFL through its VCA").await;
    patch(&server, &[("/channels/ch1/solo", json!("pfl"))]).await;
    mon.expect(pfl, pfl, "ch1 PFL over st1 AFL").await;
    server.stop().await;
}

/// Each channel reads the legs its `channels` select out of a wider
/// flow, each channel of which carries its own value: ch1 flow channel
/// 16 and st1 flow channels 7 and 8 of one 16-channel flow, and ch2 flow
/// channel 3 of its input a and, once switched, of its input b.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_read_the_legs_they_select_from_a_wide_flow() {
    let domain = domain("select");
    let data = data("select");
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

/// A talkback interrupt puts its source channel on the mix-minus output
/// in place of the mix, before the channel's closed fader, and leaves the
/// programme alone; released, the output carries the mix again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_talkback_interrupt_replaces_the_mix_minus_until_released() {
    let domain = domain("talk");
    let data = data("talk");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let _ch1 = Source::start(&domain.0, &flows.ch1, vec![CH1]);
    let _st1 = Source::start(&domain.0, &flows.st1, ST1.to_vec());
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.mm).await;
    let mut pgm = Reader::new(&domain.0, &flows.pgm);
    let mut mm = Reader::new(&domain.0, &flows.mm);
    // ch1's fader stays closed; hard left, so the talkback is exact.
    patch(
        &server,
        &[
            ("/channels/st1/fader", json!(0)),
            ("/channels/ch1/pan/lr", json!(-1)),
            ("/mixMinus/mm1/interrupt/source", json!("ch1")),
        ],
    )
    .await;
    mm.expect(ST1[0], ST1[1], "the mix-minus carries the mix")
        .await;
    patch(&server, &[("/mixMinus/mm1/interrupt/active", json!(true))]).await;
    mm.expect(CH1, 0.0, "the talkback alone, before its closed fader")
        .await;
    pgm.expect(ST1[0], ST1[1], "the programme without the talkback")
        .await;
    patch(&server, &[("/mixMinus/mm1/interrupt/active", json!(false))]).await;
    mm.expect(ST1[0], ST1[1], "the mix back on release").await;
    server.stop().await;
}

/// Direct outs, mix-minus exclusions, bus, aux and mix-minus masters,
/// aux sends, VCAs and the AFV stage a tally drives reach the flows of
/// the outputs carrying them: through every step of [`routing::steps`]
/// each output holds the legs the step gives it, the outputs a step is
/// not about included.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_routing_reaches_every_output_flow() {
    let domain = domain("routing");
    let data = data("routing");
    let (ch1, ch2) = (new_id(), new_id());
    let _ch1 = Source::start(&domain.0, &ch1, vec![routing::CH1]);
    let _ch2 = Source::start(&domain.0, &ch2, vec![routing::CH2]);
    let flows: Vec<String> = routing::OUTPUTS.iter().map(|_| new_id()).collect();
    let outputs: serde_json::Map<String, Value> = (routing::OUTPUTS.iter().zip(&flows))
        .map(|((id, _), flow)| ((*id).to_owned(), json!({ "flow": flow })))
        .collect();
    let inputs = json!({ "ch1": { "flow": ch1 }, "ch2": { "flow": ch2 } });
    let server = start_with(
        &data.0,
        &domain.0,
        routing::shape(),
        inputs,
        Value::Object(outputs),
    )
    .await;
    for flow in &flows {
        appears(&domain.0, flow).await;
    }
    let mut readers: Vec<Reader> = (routing::OUTPUTS.iter().zip(&flows))
        .map(|((_, width), flow)| Reader::wide(&domain.0, flow, *width))
        .collect();
    // A patch before the engine is ready is refused with 503.
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(server.ops, "/readyz").await.0 != 200 {
        assert!(Instant::now() < deadline, "ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let n = B as usize;
    let near = |leg: &[f32], want: f32| leg.iter().all(|v| (v - want).abs() <= 1e-6);
    for step in routing::steps() {
        match &step.change {
            routing::Change::Patch(changes) => patch(&server, changes).await,
            routing::Change::Tally(source, state) => tally(&server, source, state).await,
        }
        tokio::time::sleep(routing::SETTLE).await;
        // Twice in a row, so a block written while a ramp was still
        // moving does not pass a step.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut held = 0;
        loop {
            let mut read = Vec::new();
            let mut all = true;
            for (reader, want) in readers.iter_mut().zip(step.legs.each()) {
                let Some(block) = reader.recent() else {
                    all = false;
                    continue;
                };
                all &= block.chunks(n).zip(want).all(|(leg, w)| near(leg, *w));
                read.push(block.chunks(n).map(|leg| leg[n - 1]).collect::<Vec<_>>());
            }
            held = if all { held + 1 } else { 0 };
            if held == 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{}: expected {:?}, last read {read:?}",
                step.what,
                step.legs
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    server.stop().await;
}

/// The monitor output plays its source as the monitor controls set it:
/// level, dim at its depth, cut, mono, speaker mutes and solos, and a
/// wider source folded down Lo/Ro or taken L and R alone, through every
/// step of [`monitor::steps`].
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_monitor_controls_reach_the_monitor_output_flow() {
    let domain = domain("monitor");
    let data = data("monitor");
    let (st, sur, mon) = (new_id(), new_id(), new_id());
    let _st = Source::start(&domain.0, &st, monitor::ST.to_vec());
    let _sur = Source::start(&domain.0, &sur, monitor::SUR.to_vec());
    let inputs = json!({ "st": { "flow": st }, "sur": { "flow": sur } });
    let outputs = json!({ "out-mon": { "flow": mon } });
    let server = start_with(&data.0, &domain.0, monitor::shape(), inputs, outputs).await;
    appears(&domain.0, &mon).await;
    let mut out = Reader::new(&domain.0, &mon);
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(server.ops, "/readyz").await.0 != 200 {
        assert!(Instant::now() < deadline, "ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Each step changes what the monitor plays: the block that reads it
    // is after the change.
    for step in monitor::steps() {
        patch(&server, &step.changes).await;
        out.expect(step.legs[0], step.legs[1], step.what).await;
    }
    server.stop().await;
}

/// POSTs `body` to `path`, which must succeed; the answer.
async fn posted(server: &Server, path: &str, body: &Value) -> String {
    let (status, answer) = post(server.api, path, body).await;
    assert!(
        (200..300).contains(&status),
        "{path} {body}: {status} {answer}"
    );
    answer
}

/// A scene recall brings back the mix stored in it, and an element made
/// recall-safe for its fader keeps the fader it has.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scene_recall_brings_the_stored_mix_back() {
    let domain = domain("scene");
    let data = data("scene");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let _ch1 = Source::start(&domain.0, &flows.ch1, vec![CH1]);
    let _st1 = Source::start(&domain.0, &flows.st1, ST1.to_vec());
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.pgm).await;
    let mut pgm = Reader::new(&domain.0, &flows.pgm);
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(server.ops, "/readyz").await.0 != 200 {
        assert!(Instant::now() < deadline, "ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let low = 10f32.powf(-20.0 / 20.0);
    let faders = |db: i32| {
        [
            ("/channels/ch1/fader", json!(db)),
            ("/channels/st1/fader", json!(db)),
        ]
    };
    patch(&server, &[("/channels/ch1/pan/lr", json!(-1))]).await;
    patch(&server, &faders(0)).await;
    pgm.expect(CH1 + ST1[0], ST1[1], "the mix stored").await;
    let id = faderz::app::uuid().unwrap();
    let stored = posted(
        &server,
        "/scenes",
        &json!({ "commandId": id, "name": "Show" }),
    )
    .await;
    let scene: Value = serde_json::from_str(&stored).unwrap();
    let path = format!("/scenes/{}/recall", scene["scene"]["id"].as_str().unwrap());
    async fn recall(server: &Server, path: &str) {
        let body = json!({ "commandId": faderz::app::uuid().unwrap(), "crossfadeMs": 0 });
        posted(server, path, &body).await;
    }
    patch(&server, &faders(-20)).await;
    pgm.expect((CH1 + ST1[0]) * low, ST1[1] * low, "both faders down")
        .await;
    recall(&server, &path).await;
    pgm.expect(CH1 + ST1[0], ST1[1], "the stored mix recalled")
        .await;
    patch(&server, &faders(-20)).await;
    patch(&server, &[("/recallSafe/st1", json!(["fader"]))]).await;
    pgm.expect((CH1 + ST1[0]) * low, ST1[1] * low, "both faders down again")
        .await;
    recall(&server, &path).await;
    pgm.expect(CH1 + ST1[0] * low, ST1[1] * low, "st1's fader kept")
        .await;
    server.stop().await;
}

/// A recall with a crossfade takes the programme through the levels
/// between the two mixes, rising throughout, and reaches the recalled
/// mix within the crossfade time; one without a crossfade is a cut,
/// within the engine's ramp length.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scene_recall_crossfades_over_its_time() {
    let domain = domain("xfade");
    let data = data("xfade");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let _ch1 = Source::start(&domain.0, &flows.ch1, vec![CH1]);
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.pgm).await;
    let mut pgm = Reader::new(&domain.0, &flows.pgm);
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(server.ops, "/readyz").await.0 != 200 {
        assert!(Instant::now() < deadline, "ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let rate = shape()["sampleRate"].as_u64().unwrap();
    // samplez's default rampSamples: a gain change without a fade ramps
    // over it.
    let ramp = 480;
    let (high, low) = (CH1, CH1 * 10f32.powf(-20.0 / 20.0));
    patch(&server, &[("/channels/ch1/pan/lr", json!(-1))]).await;
    patch(&server, &[("/channels/ch1/fader", json!(0))]).await;
    pgm.expect(high, 0.0, "the mix stored").await;
    let id = faderz::app::uuid().unwrap();
    let stored = posted(
        &server,
        "/scenes",
        &json!({ "commandId": id, "name": "Show" }),
    )
    .await;
    let scene: Value = serde_json::from_str(&stored).unwrap();
    let path = format!("/scenes/{}/recall", scene["scene"]["id"].as_str().unwrap());
    let between = |trace: &[f32]| {
        (trace.iter())
            .filter(|&&v| v > low + 1e-6 && v < high - 1e-6)
            .count() as u64
    };
    for ms in [400u64, 0] {
        patch(&server, &[("/channels/ch1/fader", json!(-20))]).await;
        pgm.expect(low, 0.0, "the fader down").await;
        let from = pgm.behind();
        let body = json!({ "commandId": faderz::app::uuid().unwrap(), "crossfadeMs": ms });
        posted(&server, &path, &body).await;
        let trace = pgm.trace(from, high).await;
        let rising = trace.windows(2).all(|w| w[1] >= w[0] - 1e-6);
        assert!(rising, "crossfade {ms} ms: the level falls on its way up");
        let mid = between(&trace);
        let len = ms * rate / 1000;
        if ms == 0 {
            assert!(mid <= ramp, "a cut: {mid} samples between the mixes");
            continue;
        }
        assert!(
            (len / 2..=len).contains(&mid),
            "crossfade {ms} ms, {len} samples: {mid} samples between the mixes"
        );
        let half = (low + high) / 2.0;
        assert!(
            trace
                .iter()
                .any(|&v| (v - half).abs() < (high - low) / 20.0),
            "crossfade {ms} ms: no level half way"
        );
    }
    server.stop().await;
}

/// A tally closing ch1's AFV stage holds the programme from the index it
/// names for holdMs, then lowers it to offLevel over fallMs; one opening
/// it raises it from that index over riseMs, and transitionMs takes the
/// place of both lengths. Each edge lands on its sample, whether the
/// tally names the index as atIndex or as a TAI time. A tally naming an
/// index already past counts in afv_late_total.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_afv_stage_holds_falls_and_rises_on_its_times() {
    let domain = domain("afv-times");
    // The transitions fit in the domain's history.
    std::fs::write(
        domain.0.join("options.json"),
        r#"{"urn:x-mxl:option:history_duration/v1.0": 4000000000}"#,
    )
    .unwrap();
    let data = data("afv-times");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let _ch1 = Source::start(&domain.0, &flows.ch1, vec![CH1]);
    let rate = shape()["sampleRate"].as_u64().unwrap();
    let samples = |ms: u64| ms * rate / 1000;
    let block = 48;
    let server = start_engine(
        &data.0,
        &domain.0,
        shape(),
        json!({
            "ch1": { "flow": flows.ch1 },
            "ch2": { "flow": flows.ch2, "altFlow": flows.ch2b },
            "st1": { "flow": flows.st1 },
        }),
        json!({ "out-pgm": { "flow": flows.pgm }, "out-mm": { "flow": flows.mm } }),
        // Every sample of each transition is read, which a debug engine
        // on a loaded host writes whole only with time in hand, as in
        // a_burst_ducks_the_programme_but_not_the_me.
        json!({
            "blockSize": block,
            "readerMarginSamples": samples(100),
            "maxCatchupBlocks": samples(100) / block,
        }),
    )
    .await;
    appears(&domain.0, &flows.pgm).await;
    let mut pgm = Reader::new(&domain.0, &flows.pgm);
    let (rise, hold, fall, transition) = (100, 200, 300, 50);
    let (high, low) = (CH1, CH1 * 10f32.powf(-20.0 / 20.0));
    patch(
        &server,
        &[
            ("/channels/ch1/pan/lr", json!(-1)),
            ("/channels/ch1/fader", json!(0)),
            ("/afv/ch1/enabled", json!(true)),
            ("/afv/ch1/source", json!("cam1")),
            ("/afv/ch1/offLevel", json!(-20)),
            ("/afv/ch1/riseMs", json!(rise)),
            ("/afv/ch1/holdMs", json!(hold)),
            ("/afv/ch1/fallMs", json!(fall)),
        ],
    )
    .await;
    pgm.expect(high, 0.0, "ch1 open").await;
    let api = api();
    let now = || time::timestamp_to_index(&api, time::now(&api));
    // The programme output carries the bus's limiter, late by its
    // look-ahead.
    let late = samplez_dsp::LIMITER_DELAY as u64;
    let near = |v: f32, want: f32| (v - want).abs() <= 1e-6;
    // The tally, its transitionMs, whether it names a TAI time, and the
    // hold and length its gain moves over.
    for (state, transition_ms, tai, wait, len) in [
        ("preview", None, false, hold, fall),
        ("program", None, false, 0, rise),
        ("preview", Some(transition), true, hold, transition),
        ("program", Some(transition), false, 0, transition),
    ] {
        let what = format!("{state}, transitionMs {transition_ms:?}");
        let (from, to) = if state == "program" {
            (low, high)
        } else {
            (high, low)
        };
        // Half a second ahead, off the block grid, or on a whole
        // millisecond for the time RFC 3339 carries.
        let mut at = now() + samples(500) + 7;
        let mut body = json!({ "source": "cam1", "state": state });
        if tai {
            at -= at % samples(1);
            let since = Duration::from_secs(at / rate)
                + Duration::from_nanos(at % rate * 1_000_000_000 / rate);
            let t = std::time::UNIX_EPOCH + since;
            body["at"] = json!(faderz::app::rfc3339(t));
        } else {
            body["atIndex"] = json!(at);
        }
        if let Some(t) = transition_ms {
            body["transitionMs"] = json!(t);
        }
        posted(&server, "/afv/tally", &body).await;
        let edge = at + samples(wait);
        let end = edge + samples(len);
        let (first, last) = (at - samples(100), end + samples(100));
        // Written by then, with the 100 ms the clock leads the output by.
        while now() <= last + late + samples(200) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let legs = span(&domain.0, &flows.pgm, first + late, last + late);
        for (n, (l, _)) in (first..).zip(&legs) {
            let i = n as i64 - at as i64;
            if n <= edge {
                assert!(near(*l, from), "{what}: {l} at {i} from the tally");
            } else if n >= end {
                assert!(near(*l, to), "{what}: {l} at {i} from the tally");
            } else {
                let (lo, hi) = (from.min(to), from.max(to));
                assert!(
                    lo < *l && *l < hi && !near(*l, from) && !near(*l, to),
                    "{what}: {l} at {i} from the tally, in the transition"
                );
            }
        }
    }
    let (_, metrics) = get(server.ops, "/metrics").await;
    assert!(metrics.contains("faderz_afv_late_total 0\n"), "{metrics}");
    // A second past: counted, and the stage closed at once.
    let body = json!({ "source": "cam1", "state": "preview", "atIndex": now() - samples(1000) });
    posted(&server, "/afv/tally", &body).await;
    let (_, metrics) = get(server.ops, "/metrics").await;
    assert!(metrics.contains("faderz_afv_late_total 1\n"), "{metrics}");
    pgm.expect(low, 0.0, "a late tally applied").await;
    server.stop().await;
}

/// A bus, a mix-minus and an aux each meter the tone they carry on its
/// peak, true peak, RMS and momentary loudness, and a duck group meters
/// its key, envelope and engagement while its key crosses the threshold,
/// through every step of [`metered::steps`].
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn element_and_duck_meters_read_their_signals() {
    let domain = domain("metered");
    let data = data("metered");
    let (tone, dc, key, bed) = (new_id(), new_id(), new_id(), new_id());
    let (pgm, mm) = (new_id(), new_id());
    let _tone = Source::sine(&domain.0, &tone, metered::TONE.0, metered::TONE.1);
    let _dc = Source::start(&domain.0, &dc, vec![metered::DC]);
    let _key = Source::start(&domain.0, &key, vec![metered::KEY]);
    // bed reads a flow nobody writes: silent.
    let inputs = json!({
        "tone": { "flow": tone },
        "dc": { "flow": dc },
        "key": { "flow": key },
        "bed": { "flow": bed },
    });
    let outputs = json!({ "out-pgm": { "flow": pgm }, "out-mm": { "flow": mm } });
    let server = start_with(&data.0, &domain.0, metered::shape(), inputs, outputs).await;
    appears(&domain.0, &mm).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(server.ops, "/readyz").await.0 != 200 {
        assert!(Instant::now() < deadline, "ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for step in metered::steps() {
        if !step.changes.is_empty() {
            patch(&server, &step.changes).await;
        }
        if !step.resets.is_empty() {
            // Every short-term window then holds the signal alone.
            tokio::time::sleep(Duration::from_millis(3500)).await;
            let (status, body) = post(
                server.api,
                "/loudness/reset",
                &json!({ "ids": step.resets }),
            )
            .await;
            assert_eq!(status, 204, "{body}");
        }
        // A momentary loudness takes 400 ms to measure.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let v = meter_values(&server, 1).await;
            let read = |&(i, want, tol): &(usize, i16, i16)| {
                (i32::from(v[i]) - i32::from(want)).abs() <= i32::from(tol)
            };
            if step.values.iter().all(read) {
                break;
            }
            let wrong: Vec<_> = (step.values.iter())
                .filter(|w| !read(w))
                .map(|&(i, want, _)| (i, want, v[i]))
                .collect();
            assert!(
                Instant::now() < deadline,
                "{}: (index, wanted, read) {wrong:?}",
                step.what
            );
        }
        let engine = server.audio.as_ref().unwrap().engine();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !metered::pre_strip_runs(&engine.status(), &step) {
            assert!(
                Instant::now() < deadline,
                "{}: the pre-strip meters running are not {:?}",
                step.what,
                step.pre_strip
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    server.stop().await;
}

/// A mono channel's pan places it in the field of a 5.1 bus: at the
/// front it is the centre leg alone, divergence spreads it to L and R,
/// fr takes it to the surrounds, and the LFE send is a gain of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mono_channels_pan_places_it_in_a_surround_field() {
    let domain = domain("surround-pan");
    let data = data("surround-pan");
    let (src, out51) = (new_id(), new_id());
    let _src = Source::start(&domain.0, &src, vec![0.5]);
    let shape = json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "m", "name": "Mic", "width": 1, "layout": "mono", "altInput": false },
        ],
        "buses": [
            { "id": "pgm", "name": "PGM", "kind": "program", "width": 6, "layout": "5.1" },
        ],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-51", "name": "PGM 5.1", "source": "pgm", "width": 6, "layout": "5.1" },
        ],
    });
    let inputs = json!({ "m": { "flow": src } });
    let outputs = json!({ "out-51": { "flow": out51 } });
    let server = start_with(&data.0, &domain.0, shape, inputs, outputs).await;
    appears(&domain.0, &out51).await;
    let mut wide = Reader::wide(&domain.0, &out51, 6);
    patch(
        &server,
        &[
            ("/channels/m/fader", json!(0)),
            ("/channels/m/routes/pgm", json!(true)),
        ],
    )
    .await;
    let (a, h) = (0.5f32, std::f32::consts::FRAC_1_SQRT_2);
    let n = B as usize;
    let near = |v: &f32, want: f32| (v - want).abs() <= 1e-6;
    // L R C LFE Ls Rs for each position of the field.
    let lfe = a * 10f32.powf(-6.0 / 20.0);
    let steps: [(&str, Value, [f32; 6]); 5] = [
        (
            "the L/R pair a new channel is",
            json!({}),
            [a * h, a * h, 0.0, 0.0, 0.0, 0.0],
        ),
        (
            "front centre",
            json!({ "spread": 0 }),
            [0.0, 0.0, a, 0.0, 0.0, 0.0],
        ),
        (
            "full divergence",
            json!({ "spread": 1 }),
            [a * h, a * h, 0.0, 0.0, 0.0, 0.0],
        ),
        (
            "rear",
            json!({ "spread": 0, "frontRear": 1 }),
            [0.0, 0.0, 0.0, 0.0, a * h, a * h],
        ),
        (
            "lfe send",
            json!({ "frontRear": -1, "lfeSend": -6 }),
            [0.0, 0.0, a, lfe, 0.0, 0.0],
        ),
    ];
    for (what, pan, want) in steps {
        let writes: Vec<(String, Value)> = pan
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (format!("/channels/m/pan/{k}"), v.clone()))
            .collect();
        let writes: Vec<(&str, Value)> = writes
            .iter()
            .map(|(p, v)| (p.as_str(), v.clone()))
            .collect();
        if !writes.is_empty() {
            patch(&server, &writes).await;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(b) = wide.recent()
                && b.chunks(n)
                    .zip(want)
                    .all(|(leg, w)| leg.iter().all(|v| near(v, w)))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{what}: the 5.1 flow carries {want:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    server.stop().await;
}

/// A 5.1 channel on a 5.1 bus, written by a 6-leg output and by a stereo
/// one: the stereo flow is the bus folded down by BS.775 Lo/Ro, centre
/// and surrounds at -3 dB and the LFE dropped, after the bus fader, and
/// follows the bus fader as it moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_output_narrower_than_its_bus_writes_the_fold_down() {
    let domain = domain("fold");
    let data = data("fold");
    let (src, out51, out_lr) = (new_id(), new_id(), new_id());
    // L R C LFE Ls Rs, each a distinct power of two.
    let legs = [0.5f32, 0.25, 0.125, 0.0625, 0.03125, 0.015625];
    let _src = Source::start(&domain.0, &src, legs.to_vec());
    let shape = json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "sur", "name": "Surround", "width": 6, "layout": "5.1", "altInput": false },
        ],
        "buses": [
            { "id": "pgm", "name": "PGM", "kind": "program", "width": 6, "layout": "5.1" },
        ],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-51", "name": "PGM 5.1", "source": "pgm", "width": 6, "layout": "5.1" },
            { "id": "out-lr", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
        ],
    });
    let inputs = json!({ "sur": { "flow": src } });
    let outputs = json!({ "out-51": { "flow": out51 }, "out-lr": { "flow": out_lr } });
    let server = start_with(&data.0, &domain.0, shape, inputs, outputs).await;
    appears(&domain.0, &out51).await;
    appears(&domain.0, &out_lr).await;
    let mut wide = Reader::wide(&domain.0, &out51, 6);
    let mut lr = Reader::new(&domain.0, &out_lr);
    patch(
        &server,
        &[
            ("/channels/sur/fader", json!(0)),
            ("/channels/sur/routes/pgm", json!(true)),
        ],
    )
    .await;
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let (lo, ro) = (
        legs[0] + h * legs[2] + h * legs[4],
        legs[1] + h * legs[2] + h * legs[5],
    );
    lr.expect(lo, ro, "Lo/Ro of the 5.1 bus").await;
    let n = B as usize;
    let near = |v: &f32, want: f32| (v - want).abs() <= 1e-6;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(b) = wide.recent()
            && b.chunks(n)
                .zip(legs)
                .all(|(leg, want)| leg.iter().all(|v| near(v, want)))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the 5.1 flow carries every leg as it is"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The fold follows the bus fader: -6 dB.
    patch(&server, &[("/buses/pgm/fader", json!(-6))]).await;
    let g = 10f32.powf(-6.0 / 20.0);
    lr.expect(lo * g, ro * g, "Lo/Ro after the bus fader").await;
    server.stop().await;
}

/// A bus's limiter holds the true peak of its output at the ceiling: a
/// tone whose true peak lies 3 dB above its samples, driven over the
/// ceiling, comes out at or below it with the limiter on, sample peaks
/// alone would not be, and as it goes in with the limiter off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bus_limiter_holds_the_true_peak_at_its_ceiling() {
    let domain = domain("limiter");
    let data = data("limiter");
    let (src, out) = (new_id(), new_id());
    let amp = 0.9f32;
    let _src = Source::off_crest(&domain.0, &src, 2, amp);
    let shape = json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "st", "name": "Music", "width": 2, "layout": "stereo", "altInput": false },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
        ],
    });
    let inputs = json!({ "st": { "flow": src } });
    let server = start_with(
        &data.0,
        &domain.0,
        shape,
        inputs,
        json!({ "out-pgm": { "flow": out } }),
    )
    .await;
    appears(&domain.0, &out).await;
    let mut pgm = Reader::new(&domain.0, &out);
    let ceiling_db = -6.0f32;
    let ceiling = 10f32.powf(ceiling_db / 20.0);
    patch(
        &server,
        &[
            ("/channels/st/fader", json!(0)),
            ("/channels/st/routes/pgm", json!(true)),
            ("/buses/pgm/limiter/ceiling", json!(ceiling_db)),
        ],
    )
    .await;
    // The output is the tone scaled, so its true peak is its sample peak
    // times sqrt(2).
    for enabled in [false, true, false] {
        patch(&server, &[("/buses/pgm/limiter/enabled", json!(enabled))]).await;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = None;
        loop {
            if let Some((l, r)) = pgm.peaks() {
                let tp = [l, r].map(|p| p * std::f32::consts::SQRT_2);
                last = Some(tp);
                if tp.into_iter().all(|p| limited(enabled, p, amp, ceiling)) {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "limiter enabled {enabled}: tone {amp}, ceiling {ceiling}, true peaks read {last:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    server.stop().await;
}

/// A channel's delay plays its input exactly that many samples late, and
/// a delay of 0 not late at all: both edges of a pulse on the input land
/// on the programme output at the limiter's look-ahead plus the delay,
/// to the sample, at a delay off the block grid and at the full line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_channel_delay_plays_its_input_exactly_that_late() {
    let domain = domain("delay");
    // The pulses are read back from the output's history.
    std::fs::write(
        domain.0.join("options.json"),
        r#"{"urn:x-mxl:option:history_duration/v1.0": 4000000000}"#,
    )
    .unwrap();
    let data = data("delay");
    let (src, out) = (new_id(), new_id());
    let rate: u64 = 48000;
    let block = 48u64;
    let pulse = Arc::new([AtomicU64::new(u64::MAX), AtomicU64::new(u64::MAX)]);
    let at = pulse.clone();
    let _src = Source::signal(&domain.0, &src, 1, move |_, idx| {
        let on = (at[0].load(Relaxed)..at[1].load(Relaxed)).contains(&idx);
        if on { 0.5 } else { 0.0 }
    });
    let shape = json!({
        "sampleRate": rate,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "ch", "name": "Pres", "width": 1, "layout": "mono", "altInput": false },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
        ],
    });
    // A line of 20 ms.
    let line = rate / 50;
    let server = start_engine(
        &data.0,
        &domain.0,
        shape,
        json!({ "ch": { "flow": src, "maxDelayMs": 20 } }),
        json!({ "out-pgm": { "flow": out } }),
        // As for the duck test: a debug engine on a loaded host computes
        // each window whole only with time in hand.
        json!({
            "blockSize": block,
            "readerMarginSamples": rate / 10,
            "maxCatchupBlocks": rate / 10 / block,
        }),
    )
    .await;
    appears(&domain.0, &out).await;
    patch(
        &server,
        &[
            ("/buses/pgm/fader", json!(0)),
            ("/channels/ch/fader", json!(0)),
            ("/channels/ch/routes/pgm", json!(true)),
        ],
    )
    .await;
    let api = api();
    let engine = server.audio.as_ref().unwrap().engine();
    let skipped = || engine.counters().overrun_skipped_samples.load(Relaxed);
    let late = samplez_dsp::LIMITER_DELAY as u64;
    let now = || time::timestamp_to_index(&api, time::now(&api));
    // 0, a delay off the block grid, the full line, and 0 again.
    for delay in [0, rate / 100 + 7, line, 0] {
        patch(&server, &[("/channels/ch/delay", json!(delay))]).await;
        let skipped_before = skipped();
        // A pulse of 100 ms from 0.5 s on, off the block grid: the delay
        // has applied and its crossfade ended long before.
        let start = now() + rate / 2 + 7;
        let end = start + rate / 10;
        pulse[1].store(end, Relaxed);
        pulse[0].store(start, Relaxed);
        let (from, to) = (start - rate / 10, end + line + late + rate / 10);
        // Written by then, with the 100 ms the clock leads the output by.
        while now() <= to + rate / 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        pulse[0].store(u64::MAX, Relaxed);
        assert_eq!(
            skipped(),
            skipped_before,
            "delay {delay}: samples skipped, the engine was starved for over 100 ms"
        );
        let heard: Vec<u64> = (span(&domain.0, &out, from, to).iter())
            .enumerate()
            .filter(|(_, s)| s.0.abs() > 0.1)
            .map(|(i, _)| from + i as u64)
            .collect();
        let (first, last) = (heard.first().copied(), heard.last().copied());
        assert_eq!(
            (first, last, heard.len() as u64),
            (
                Some(start + late + delay),
                Some(end - 1 + late + delay),
                end - start
            ),
            "delay {delay}: the pulse [{start}, {end}) is heard from {first:?} to {last:?}"
        );
    }
    server.stop().await;
}

/// A direct out at the input tap carries its channel's input after the
/// channel delay and before the strip, and one at preFader after it
/// ([`direct::check`]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_direct_out_at_the_input_tap_is_delayed_but_not_processed() {
    let domain = domain("direct-input");
    // The pulses are read back from the outputs' history.
    std::fs::write(
        domain.0.join("options.json"),
        r#"{"urn:x-mxl:option:history_duration/v1.0": 4000000000}"#,
    )
    .unwrap();
    let data = data("direct-input");
    let (src, at_input, pre_fader) = (new_id(), new_id(), new_id());
    let rate = direct::RATE;
    let block = 48u64;
    let pulse = Arc::new([AtomicU64::new(u64::MAX), AtomicU64::new(u64::MAX)]);
    let at = pulse.clone();
    let _src = Source::signal(&domain.0, &src, 1, move |_, idx| {
        let on = (at[0].load(Relaxed)..at[1].load(Relaxed)).contains(&idx);
        if on { direct::PULSE } else { 0.0 }
    });
    let server = start_engine(
        &data.0,
        &domain.0,
        direct::shape(),
        direct::inputs(&src),
        json!({ "do-in": { "flow": at_input }, "do-pre": { "flow": pre_fader } }),
        // As for the channel delay test.
        json!({
            "blockSize": block,
            "readerMarginSamples": rate / 10,
            "maxCatchupBlocks": rate / 10 / block,
        }),
    )
    .await;
    appears(&domain.0, &at_input).await;
    appears(&domain.0, &pre_fader).await;
    patch(&server, &direct::changes()).await;
    let api = api();
    let engine = server.audio.as_ref().unwrap().engine();
    let skipped = engine.counters().overrun_skipped_samples.load(Relaxed);
    let now = || time::timestamp_to_index(&api, time::now(&api));
    // A pulse of 100 ms from 0.5 s on, off the block grid: the changes
    // have applied and the delay's crossfade ended long before.
    let start = now() + rate / 2 + 7;
    let end = start + rate / 10;
    pulse[1].store(end, Relaxed);
    pulse[0].store(start, Relaxed);
    let (from, to) = (start - rate / 10, end + direct::DELAY + rate / 10);
    // Written by then, with the 100 ms the clock leads the output by.
    while now() <= to + rate / 2 {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    pulse[0].store(u64::MAX, Relaxed);
    assert_eq!(
        engine.counters().overrun_skipped_samples.load(Relaxed),
        skipped,
        "samples skipped, the engine was starved for over 100 ms"
    );
    direct::check(
        &span_left(&domain.0, &at_input, 1, from, to),
        &span_left(&domain.0, &pre_fader, 1, from, to),
        from,
        start,
        end,
    );
    server.stop().await;
}

/// A bus's delay plays every output carrying it exactly that many samples
/// late, a mono fold-down included, and a mix-minus output of the bus not
/// late at all: both edges of a pulse land on each programme output at
/// the limiter's look-ahead plus the delay, to the sample, and on the
/// mix-minus output at once, at a delay off the block grid and at the
/// full line. Started again on its data, as a restarted pod is, the
/// server plays the stored delay from its start, with no write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bus_delay_plays_every_output_carrying_it_exactly_that_late() {
    let domain = domain("busdelay");
    // The pulses are read back from the outputs' history.
    std::fs::write(
        domain.0.join("options.json"),
        r#"{"urn:x-mxl:option:history_duration/v1.0": 4000000000}"#,
    )
    .unwrap();
    let data = data("busdelay");
    let (src, out, mono, mm) = (new_id(), new_id(), new_id(), new_id());
    let rate: u64 = 48000;
    let pulse = Arc::new([AtomicU64::new(u64::MAX), AtomicU64::new(u64::MAX)]);
    let at = pulse.clone();
    let _src = Source::signal(&domain.0, &src, 1, move |_, idx| {
        let on = (at[0].load(Relaxed)..at[1].load(Relaxed)).contains(&idx);
        if on { 0.5 } else { 0.0 }
    });
    // A line of 20 ms on both outputs of the bus.
    let line = rate / 50;
    let boot = async || {
        let block = 48u64;
        let server = start_engine(
            &data.0,
            &domain.0,
            json!({
                "sampleRate": rate,
                "frameRate": { "numerator": 25, "denominator": 1 },
                "channels": [
                    { "id": "ch", "name": "Pres", "width": 1, "layout": "mono", "altInput": false },
                ],
                "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
                "auxes": [],
                "mixMinus": [{ "id": "mm1", "name": "N-1", "bus": "pgm", "width": 2, "layout": "stereo" }],
                "vcas": [], "monitorOutputs": [],
                "outputs": [
                    { "id": "out-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
                    { "id": "out-pgm-m", "name": "PGM mono", "source": "pgm", "width": 1, "layout": "mono" },
                    { "id": "out-mm", "name": "N-1", "source": "mm1", "width": 2, "layout": "stereo" },
                ],
            }),
            json!({ "ch": { "flow": src } }),
            json!({
                "out-pgm": { "flow": out, "maxDelayMs": 20 },
                "out-pgm-m": { "flow": mono, "maxDelayMs": 20 },
                "out-mm": { "flow": mm },
            }),
            // As for the channel delay test: a debug engine on a loaded
            // host computes each window whole only with time in hand.
            json!({
                "blockSize": block,
                "readerMarginSamples": rate / 10,
                "maxCatchupBlocks": rate / 10 / block,
            }),
        )
        .await;
        for id in [&out, &mono, &mm] {
            appears(&domain.0, id).await;
        }
        server
    };
    let mut server = boot().await;
    patch(
        &server,
        &[
            ("/buses/pgm/fader", json!(0)),
            ("/channels/ch/fader", json!(0)),
            ("/channels/ch/routes/pgm", json!(true)),
        ],
    )
    .await;
    let api = api();
    let late = samplez_dsp::LIMITER_DELAY as u64;
    let now = || time::timestamp_to_index(&api, time::now(&api));
    let off_grid = rate / 100 + 7;
    // 0, a delay off the block grid, the full line, 0 again and off the
    // grid again, each written; then a restart on that last one.
    let steps = [0, off_grid, line, 0, off_grid].map(|d| (Some(d), d));
    for (write, delay) in steps.into_iter().chain([(None, off_grid)]) {
        match write {
            Some(d) => patch(&server, &[("/buses/pgm/delay", json!(d))]).await,
            None => {
                server.stop().await;
                server = boot().await;
            }
        }
        let engine = server.audio.as_ref().unwrap().engine();
        let skipped = || engine.counters().overrun_skipped_samples.load(Relaxed);
        let skipped_before = skipped();
        let start = now() + rate / 2 + 7;
        let end = start + rate / 10;
        pulse[1].store(end, Relaxed);
        pulse[0].store(start, Relaxed);
        let (from, to) = (start - rate / 10, end + line + late + rate / 10);
        while now() <= to + rate / 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        pulse[0].store(u64::MAX, Relaxed);
        let what = if write.is_some() {
            "written"
        } else {
            "restarted"
        };
        assert_eq!(
            skipped(),
            skipped_before,
            "delay {delay}, {what}: samples skipped, the engine was starved for over 100 ms"
        );
        for (flow, legs, lag, output) in [
            (&out, 2, late + delay, "PGM"),
            (&mono, 1, late + delay, "PGM mono"),
            (&mm, 2, 0, "N-1"),
        ] {
            let heard: Vec<u64> = (span_left(&domain.0, flow, legs, from, to).iter())
                .enumerate()
                .filter(|(_, s)| s.abs() > 0.1)
                .map(|(i, _)| from + i as u64)
                .collect();
            let (first, last) = (heard.first().copied(), heard.last().copied());
            assert_eq!(
                (first, last, heard.len() as u64),
                (Some(start + lag), Some(end - 1 + lag), end - start),
                "{output}, delay {delay}, {what}: the pulse [{start}, {end}) is heard from {first:?} to {last:?}"
            );
        }
    }
    server.stop().await;
}

/// A bus's and a mix-minus output's strip, flat or off, leaves every
/// sample as no strip would, bit for bit: a state stored before the
/// strips were applied plays as it did. Through a mono channel panned
/// hard left at 0 dB, the left leg of the programme output is the source
/// exactly, the limiter's look-ahead late, and that of the mix-minus
/// output the source exactly: at start, with every EQ band, gate and
/// compressor set but off and the dynamics first, and with the EQ on and
/// every band at 0 dB.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flat_or_disabled_output_strip_leaves_every_sample() {
    let domain = domain("flatstrip");
    // The spans are read back from the outputs' history.
    std::fs::write(
        domain.0.join("options.json"),
        r#"{"urn:x-mxl:option:history_duration/v1.0": 4000000000}"#,
    )
    .unwrap();
    let data = data("flatstrip");
    let (src, out, mm) = (new_id(), new_id(), new_id());
    let rate: u64 = 48000;
    let block = 48u64;
    // Exact in f32 and under any threshold the dynamics are set to below:
    // a sawtooth of 997 steps of 1/2048, from -498/2048.
    let source = |idx: u64| ((idx % 997) as f32 - 498.0) / 2048.0;
    let _src = Source::signal(&domain.0, &src, 1, move |_, idx| source(idx));
    let shape = json!({
        "sampleRate": rate,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "ch", "name": "Pres", "width": 1, "layout": "mono", "altInput": false },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [],
        "mixMinus": [{ "id": "mm1", "name": "N-1", "bus": "pgm", "width": 2, "layout": "stereo" }],
        "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
            { "id": "out-mm", "name": "N-1", "source": "mm1", "width": 2, "layout": "stereo" },
        ],
    });
    let server = start_engine(
        &data.0,
        &domain.0,
        shape,
        json!({ "ch": { "flow": src } }),
        json!({ "out-pgm": { "flow": out }, "out-mm": { "flow": mm } }),
        // As for the delay tests: a debug engine on a loaded host computes
        // each window whole only with time in hand.
        json!({
            "blockSize": block,
            "readerMarginSamples": rate / 10,
            "maxCatchupBlocks": rate / 10 / block,
        }),
    )
    .await;
    for id in [&out, &mm] {
        appears(&domain.0, id).await;
    }
    patch(
        &server,
        &[
            ("/buses/pgm/fader", json!(0)),
            ("/channels/ch/fader", json!(0)),
            ("/channels/ch/pan/lr", json!(-1)),
            ("/channels/ch/routes/pgm", json!(true)),
        ],
    )
    .await;
    let api = api();
    let engine = server.audio.as_ref().unwrap().engine();
    let skipped = || engine.counters().overrun_skipped_samples.load(Relaxed);
    let late = samplez_dsp::LIMITER_DELAY as u64;
    let now = || time::timestamp_to_index(&api, time::now(&api));
    // Every leaf of an element's EQ and dynamics, set or flat.
    let strip = |e: &str, eq: bool, gain: i32| -> Vec<(String, Value)> {
        let mut leaves = vec![
            (format!("/{e}/eq/enabled"), json!(eq)),
            (format!("/{e}/gate/enabled"), json!(false)),
            (format!("/{e}/gate/threshold"), json!(-10)),
            (format!("/{e}/gate/range"), json!(60)),
            (format!("/{e}/comp/enabled"), json!(false)),
            (format!("/{e}/comp/threshold"), json!(-40)),
            (format!("/{e}/comp/ratio"), json!(20)),
            (format!("/{e}/comp/makeup"), json!(6)),
            (format!("/{e}/dynamicsBeforeEq"), json!(true)),
        ];
        for band in 0..4 {
            leaves.push((format!("/{e}/eq/bands/{band}/type"), json!("peak")));
            leaves.push((
                format!("/{e}/eq/bands/{band}/freq"),
                json!(250 * (band + 1)),
            ));
            leaves.push((format!("/{e}/eq/bands/{band}/gain"), json!(gain)));
            leaves.push((format!("/{e}/eq/bands/{band}/q"), json!(1)));
        }
        leaves
    };
    let cases: [(&str, Vec<(String, Value)>); 3] = [
        ("flat and off at start", Vec::new()),
        (
            "set and off",
            [
                strip("buses/pgm", false, 12),
                strip("mixMinus/mm1", false, -9),
            ]
            .concat(),
        ),
        (
            "EQ on, every band at 0 dB",
            [strip("buses/pgm", true, 0), strip("mixMinus/mm1", true, 0)].concat(),
        ),
    ];
    for (what, leaves) in cases {
        let leaves: Vec<(&str, Value)> = (leaves.iter())
            .map(|(p, v)| (p.as_str(), v.clone()))
            .collect();
        if !leaves.is_empty() {
            patch(&server, &leaves).await;
        }
        let skipped_before = skipped();
        // Half a second on, any ramp a write started is long over.
        let from = now() + rate / 2 + 7;
        let to = from + rate / 5;
        while now() <= to + rate / 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            skipped(),
            skipped_before,
            "{what}: samples skipped, the engine was starved for over 100 ms"
        );
        for (flow, lag, output) in [(&out, late, "PGM"), (&mm, 0, "N-1")] {
            let left = span_left(&domain.0, flow, 2, from, to);
            let wrong = (left.iter().enumerate())
                .map(|(i, v)| (from + i as u64, *v))
                .find(|&(idx, v)| v.to_bits() != source(idx - lag).to_bits());
            assert!(
                wrong.is_none(),
                "{output}, {what}: sample (index, value) {wrong:?} is not the source's {:?}",
                wrong.map(|(idx, _)| source(idx - lag))
            );
        }
    }
    server.stop().await;
}

/// The left leg of `flow` over the 100 ms ending 100 ms behind libmxl's
/// clock: its sample peak and its rising zero crossings per second.
fn tone_of(domain: &Path, flow: &str, rate: u64) -> (f32, u64) {
    tone_legs(domain, flow, rate)[0]
}

/// [`tone_of`] of both legs of `flow`.
fn tone_legs(domain: &Path, flow: &str, rate: u64) -> [(f32, u64); 2] {
    let api = api();
    let to = time::timestamp_to_index(&api, time::now(&api)) - rate / 10;
    let legs = span(domain, flow, to - rate / 10, to);
    [0, 1].map(|l| {
        let leg: Vec<f32> = (legs.iter())
            .map(|s| if l == 0 { s.0 } else { s.1 })
            .collect();
        let peak = leg.iter().fold(0.0f32, |p, v| p.max(v.abs()));
        let rising = leg.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        (peak, rising as u64 * 10)
    })
}

/// The line-up tone (`/tone`) replaces the mix of each output whose
/// source it targets with a 1 kHz sine at its sample-peak level on every
/// leg, after the element's fader: the programme carries the tone and
/// none of its mix, a fader move in a write heard elsewhere leaves it as
/// it is, the mix-minus output beside it keeps its mix, and off brings
/// the programme's mix back. Its engine input is no channel: the shape,
/// the state and the status list the channels alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_line_up_tone_replaces_the_mix_of_its_targets() {
    let domain = domain("line-up");
    let data = data("line-up");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let rate = shape()["sampleRate"].as_u64().unwrap();
    let _st1 = Source::start(&domain.0, &flows.st1, ST1.to_vec());
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.mm).await;
    let mut pgm = Reader::new(&domain.0, &flows.pgm);
    let mut mm = Reader::new(&domain.0, &flows.mm);
    patch(&server, &[("/channels/st1/fader", json!(0))]).await;
    pgm.expect(ST1[0], ST1[1], "the programme carries its mix")
        .await;
    mm.expect(ST1[0], ST1[1], "the mix-minus carries its mix")
        .await;

    let gain = |db: f32| 10f32.powf(db / 20.0);
    // On both legs within a crossing of 1 kHz over the 100 ms read, and
    // at the level: the mix, a constant, would lift the peak.
    let tone = async |what: &str| {
        let heard = || {
            (tone_legs(&domain.0, &flows.pgm, rate).iter())
                .all(|(peak, hz)| hz.abs_diff(1000) <= 10 && (peak - gain(-20.0)).abs() <= 1e-4)
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !heard() {
            assert!(
                Instant::now() < deadline,
                "{what}: 1 kHz at -20 dBFS not heard; read {:?}",
                tone_legs(&domain.0, &flows.pgm, rate)
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    patch(
        &server,
        &[
            ("/tone/level", json!(-20)),
            ("/tone/targets", json!(["pgm"])),
            ("/tone/on", json!(true)),
        ],
    )
    .await;
    tone("the tone in place of the programme's mix").await;
    mm.expect(ST1[0], ST1[1], "the mix-minus keeps its mix")
        .await;
    // The programme's fader in one write with st1's, which the
    // mix-minus carries: once that is heard, so is the fader.
    patch(
        &server,
        &[
            ("/buses/pgm/fader", json!(-20)),
            ("/channels/st1/fader", json!(-6)),
        ],
    )
    .await;
    let st1 = ST1.map(|v| v * gain(-6.0));
    mm.expect(st1[0], st1[1], "st1's fader on the mix-minus")
        .await;
    tone("the tone, the programme's fader moved").await;
    patch(&server, &[("/tone/on", json!(false))]).await;
    let pgm_mix = st1.map(|v| v * gain(-20.0));
    pgm.expect(
        pgm_mix[0],
        pgm_mix[1],
        "off: the programme's mix, after its fader",
    )
    .await;

    let engine = server.audio.as_ref().unwrap().engine().status();
    let tone_input = engine.inputs.last().unwrap();
    assert_eq!(
        (tone_input.name.as_str(), tone_input.state),
        ("/tone/2", samplez::InputState::Tone)
    );
    let channels = ["ch1", "ch2", "st1"];
    let (code, body) = get(server.api, "/api/v1/state").await;
    assert_eq!(code, 200, "{body}");
    let doc: Value = serde_json::from_str(&body).unwrap();
    let ids: Vec<&str> = (doc["shape"]["channels"].as_array().unwrap().iter())
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, channels, "the shape's channels");
    let keys: Vec<&String> = doc["state"]["channels"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(keys, channels, "the state's channels");
    let (code, body) = get(server.api, "/api/v1/status").await;
    assert_eq!(code, 200, "{body}");
    let status: Value = serde_json::from_str(&body).unwrap();
    let inputs: Vec<&str> = (status["engine"]["inputs"].as_array().unwrap().iter())
        .map(|i| i["channel"].as_str().unwrap())
        .collect();
    assert_eq!(inputs, channels, "the status's channel inputs");
    server.stop().await;
}

/// Polls `flow`, `legs` wide, from sample index `from` on, until each
/// window of the line-up tone's ident cycle over `legs` legs has been
/// read: in window `w`, leg `w mod (legs + 1)` is cut and every other leg
/// carries the tone at sample peak `peak`, so the last window of the
/// cycle sounds on every leg. A block is judged only when every tone
/// index it can carry, whatever the limiter's delay, lies in one window
/// away from the ident's fades.
async fn idents(domain: &Path, flow: &str, legs: usize, from: u64, peak: f32, what: &str) {
    let rate = shape()["sampleRate"].as_u64().unwrap();
    let (window, edge) = (rate / 4, rate / 100);
    let late = samplez_dsp::LIMITER_DELAY as u64;
    let cycle = legs as u64 + 1;
    let mut reader = Reader::wide(domain, flow, legs as u32);
    let api = api();
    let n = B as usize;
    let mut seen = std::collections::BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while seen.len() < cycle as usize {
        assert!(Instant::now() < deadline, "{what}: windows read {seen:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let k = time::timestamp_to_index(&api, time::now(&api)) / B - 100;
        let (lo, hi) = (k * B - late - edge, k * B + B + edge);
        if k * B < from || lo / window != (hi - 1) / window {
            continue;
        }
        let input = reader.generation.input(0).unwrap();
        let read = input.read(&mut reader.state, k, &GroupWait::Skipped, &mut reader.buf);
        if read != Outcome::Copy {
            continue;
        }
        let w = lo / window;
        let cut = (w % cycle) as usize;
        for (leg, samples) in reader.buf.chunks(n).enumerate() {
            let got = samples.iter().fold(0.0f32, |p, v| p.max(v.abs()));
            let want = if leg == cut { 0.0 } else { peak };
            assert!(
                (got - want).abs() <= 1e-4,
                "{what}: block {k}, window {w}: leg {leg} peaks at {got}, not {want}"
            );
        }
        seen.insert(w % cycle);
    }
}

/// With the ident on, each target of the line-up tone cuts its own legs
/// in turn, a stereo programme and a 5.1 clean feed at once: the
/// programme L, then R, then both sound; the clean feed each of its six
/// legs, then all of them. A monitor playing the programme carries what
/// the programme's output does, the tone and its ident, at the
/// monitor's level, and none of the mix it replaces.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_line_up_target_idents_its_own_legs() {
    let domain = domain("line-up-ident");
    let data = data("line-up-ident");
    let (ch, pgm, s51, mon) = (new_id(), new_id(), new_id(), new_id());
    let _ch = Source::start(&domain.0, &ch, vec![CH1]);
    let output = |id: &str, source: &str, width: u32, layout: &str| json!({ "id": id, "name": id, "source": source, "width": width, "layout": layout });
    let shape = json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "ch", "name": "Pres", "width": 1, "layout": "mono", "altInput": false },
        ],
        "buses": [
            { "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" },
            { "id": "s51", "name": "Clean 5.1", "kind": "cleanFeed", "width": 6, "layout": "5.1" },
        ],
        "auxes": [], "mixMinus": [], "vcas": [],
        "monitorOutputs": [{ "id": "mon", "name": "Monitor", "width": 2, "layout": "stereo" }],
        "outputs": [
            output("out-pgm", "pgm", 2, "stereo"),
            output("out-51", "s51", 6, "5.1"),
            output("out-mon", "mon", 2, "stereo"),
        ],
    });
    let server = start_with(
        &data.0,
        &domain.0,
        shape,
        json!({ "ch": { "flow": ch } }),
        json!({
            "out-pgm": { "flow": pgm },
            "out-51": { "flow": s51 },
            "out-mon": { "flow": mon },
        }),
    )
    .await;
    for flow in [&pgm, &s51, &mon] {
        appears(&domain.0, flow).await;
    }
    patch(
        &server,
        &[
            ("/channels/ch/fader", json!(0)),
            ("/channels/ch/routes/pgm", json!(true)),
            ("/channels/ch/routes/s51", json!(true)),
            ("/buses/pgm/fader", json!(0)),
            ("/buses/s51/fader", json!(0)),
            ("/monitor/source", json!("pgm")),
            ("/monitor/level", json!(0)),
        ],
    )
    .await;
    let h = std::f32::consts::FRAC_1_SQRT_2;
    Reader::new(&domain.0, &mon)
        .expect(CH1 * h, CH1 * h, "the monitor plays the programme's mix")
        .await;
    patch(
        &server,
        &[
            ("/tone/level", json!(-20)),
            ("/tone/targets", json!(["pgm", "s51"])),
            ("/tone/ident", json!(true)),
            ("/tone/on", json!(true)),
        ],
    )
    .await;
    // Past the ramps the writes start.
    let api = api();
    let from = time::timestamp_to_index(&api, time::now(&api)) + 48000 / 2;
    let peak = 10f32.powf(-20.0 / 20.0);
    idents(&domain.0, &pgm, 2, from, peak, "the stereo programme").await;
    idents(&domain.0, &s51, 6, from, peak, "the 5.1 clean feed").await;
    idents(
        &domain.0,
        &mon,
        2,
        from,
        peak,
        "the monitor on the programme",
    )
    .await;
    let engine = server.audio.as_ref().unwrap().engine().status();
    let tones: Vec<&str> = (engine.inputs.iter())
        .filter(|i| i.state == samplez::InputState::Tone)
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(tones, ["/tone/2", "/tone/6"], "a generator per width");
    server.stop().await;
}

/// A tone channel plays samplez's generator as its state sets it, with
/// no flow to read: the programme, which carries a stereo tone channel
/// leg to leg, holds its frequency at its sample peak level, and a level
/// and then a frequency written are heard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tone_channel_plays_its_frequency_at_its_level() {
    let domain = domain("tone-input");
    let data = data("tone-input");
    let out = new_id();
    let rate: u64 = 48000;
    let shape = json!({
        "sampleRate": rate,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "tn", "name": "Tone", "width": 2, "layout": "stereo", "altInput": false },
        ],
        "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
        "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
        "outputs": [
            { "id": "out-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
        ],
    });
    let server = start_with(
        &data.0,
        &domain.0,
        shape,
        json!({ "tn": { "tone": { "frequency": 1000, "level": -6 } } }),
        json!({ "out-pgm": { "flow": out } }),
    )
    .await;
    appears(&domain.0, &out).await;
    patch(
        &server,
        &[
            ("/buses/pgm/fader", json!(0)),
            ("/channels/tn/fader", json!(0)),
            ("/channels/tn/routes/pgm", json!(true)),
        ],
    )
    .await;
    let gain = |db: f32| 10f32.powf(db / 20.0);
    // Within a crossing of the frequency over the 100 ms read, and at
    // the level: the generator reaches its crest at either frequency.
    let heard = |want_hz: u64, want_db: f32| {
        let (peak, hz) = tone_of(&domain.0, &out, rate);
        hz.abs_diff(want_hz) <= 10 && (peak - gain(want_db)).abs() <= 1e-4
    };
    for (path, value, hz, db) in [
        (None, Value::Null, 1000, -6.0),
        (Some("level"), json!(-20), 1000, -20.0),
        (Some("frequency"), json!(440), 440, -20.0),
    ] {
        if let Some(leaf) = path {
            let path = format!("/channels/tn/tone/{leaf}");
            patch(&server, &[(path.as_str(), value)]).await;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !heard(hz, db) {
            assert!(
                Instant::now() < deadline,
                "{hz} Hz at {db} dBFS not heard; read {:?}",
                tone_of(&domain.0, &out, rate)
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let status = server.audio.as_ref().unwrap().engine().status();
    assert_eq!(status.inputs[0].state, samplez::InputState::Tone);
    server.stop().await;
}

/// A diff larger than the changes the engine holds at once waits for
/// room and lands whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_diff_beyond_the_engines_queue_lands_whole() {
    const N: usize = 40;
    let domain = domain("wide");
    let data = data("wide");
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
    let data = data("strip");
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

/// A bus's EQ and compressor process every output carrying the bus, a
/// mono fold-down included, in their order; the mix-minus output and the
/// monitor, which sum the bus's channels, do not hear them. A mix-minus
/// output's own EQ processes its output alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bus_strip_processes_every_output_carrying_it() {
    let domain = domain("busstrip");
    let data = data("busstrip");
    let ids: Vec<String> = (0..5).map(|_| new_id()).collect();
    let mut shape = shape();
    shape["channels"] =
        json!([{ "id": "ch", "name": "Tone", "width": 1, "layout": "mono", "altInput": false }]);
    shape["outputs"] = json!([
        { "id": "out-pgm", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
        { "id": "out-pgm-m", "name": "PGM mono", "source": "pgm", "width": 1, "layout": "mono" },
        { "id": "out-mm", "name": "N-1", "source": "mm1", "width": 2, "layout": "stereo" },
        { "id": "out-mon", "name": "Monitor", "source": "mon", "width": 2, "layout": "stereo" },
    ]);
    let _tone = Source::sine(&domain.0, &ids[0], 0.1, 1_000);
    let server = start_with(
        &data.0,
        &domain.0,
        shape,
        json!({ "ch": { "flow": ids[0] } }),
        json!({
            "out-pgm": { "flow": ids[1] },
            "out-pgm-m": { "flow": ids[2] },
            "out-mm": { "flow": ids[3] },
            "out-mon": { "flow": ids[4] },
        }),
    )
    .await;
    for id in &ids[1..] {
        appears(&domain.0, id).await;
    }
    let mut pgm = Reader::new(&domain.0, &ids[1]);
    let mut mono = Reader::wide(&domain.0, &ids[2], 1);
    let mut mm = Reader::new(&domain.0, &ids[3]);
    let mut mon = Reader::new(&domain.0, &ids[4]);
    // Hard left at 0 dB: the bus's left leg, which the mono output sums
    // with the right at -6 dB.
    patch(
        &server,
        &[
            ("/channels/ch/fader", json!(0)),
            ("/channels/ch/pan/lr", json!(-1)),
        ],
    )
    .await;
    pgm.expect_peak(0.1, 0.001, "the 1 kHz tone, bus EQ off")
        .await;
    mono.expect_peak(0.05, 0.001, "the fold-down, bus EQ off")
        .await;
    mm.expect_peak(0.1, 0.001, "the mix-minus, bus EQ off")
        .await;
    // The monitor plays the bus at its own level.
    let monitor = 0.1 * 10f32.powf(-10.0 / 20.0);
    mon.expect_peak(monitor, 0.001, "the monitor, bus EQ off")
        .await;

    patch(
        &server,
        &[
            ("/buses/pgm/eq/enabled", json!(true)),
            ("/buses/pgm/eq/bands/1/type", json!("peak")),
            ("/buses/pgm/eq/bands/1/freq", json!(1000)),
            ("/buses/pgm/eq/bands/1/gain", json!(12)),
            ("/buses/pgm/eq/bands/1/q", json!(1)),
        ],
    )
    .await;
    // A bell has no phase shift at its centre: the samples scale by +12 dB.
    let lift = 10f32.powf(12.0 / 20.0);
    pgm.expect_peak(0.1 * lift, 0.002, "a +12 dB bus bell")
        .await;
    mono.expect_peak(0.05 * lift, 0.002, "the fold-down after a +12 dB bus bell")
        .await;
    // Read after the bus's outputs moved, so over the same blocks.
    mm.expect_peak(0.1, 0.001, "the mix-minus under a bus bell")
        .await;
    mon.expect_peak(monitor, 0.001, "the monitor under a bus bell")
        .await;

    // A near-limiter at -20 dBFS: after the bell it takes the tone down
    // by about 11 dB, the fold-down's by about 5 dB; before it, the tone
    // only reaches its threshold, the fold-down's stays below it.
    patch(
        &server,
        &[
            ("/buses/pgm/comp/enabled", json!(true)),
            ("/buses/pgm/comp/threshold", json!(-20)),
            ("/buses/pgm/comp/ratio", json!(20)),
            ("/buses/pgm/comp/attack", json!(0.1)),
            ("/buses/pgm/comp/release", json!(50)),
        ],
    )
    .await;
    pgm.expect_peak(0.11, 0.03, "the bus compressor after the EQ")
        .await;
    mono.expect_peak(0.11, 0.03, "the fold-down's compressor after the EQ")
        .await;
    patch(&server, &[("/buses/pgm/dynamicsBeforeEq", json!(true))]).await;
    pgm.expect_peak(0.1 * lift, 0.002, "the bus compressor before the EQ")
        .await;
    mono.expect_peak(
        0.05 * lift,
        0.002,
        "the fold-down's compressor before the EQ",
    )
    .await;
    mm.expect_peak(0.1, 0.001, "the mix-minus under the bus's processing")
        .await;

    // The mix-minus output's own EQ: -12 dB at 1 kHz, on it alone.
    patch(
        &server,
        &[
            ("/mixMinus/mm1/eq/enabled", json!(true)),
            ("/mixMinus/mm1/eq/bands/1/type", json!("peak")),
            ("/mixMinus/mm1/eq/bands/1/freq", json!(1000)),
            ("/mixMinus/mm1/eq/bands/1/gain", json!(-12)),
            ("/mixMinus/mm1/eq/bands/1/q", json!(1)),
        ],
    )
    .await;
    mm.expect_peak(0.1 / lift, 0.001, "a -12 dB mix-minus bell")
        .await;
    pgm.expect_peak(0.1 * lift, 0.002, "the bus under a mix-minus bell")
        .await;
    mon.expect_peak(monitor, 0.001, "the monitor under a mix-minus bell")
        .await;
    server.stop().await;
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

/// The next text message of `ws` of type `kind`, checked against the
/// contract, and the types of the text messages before it; pings are
/// answered meanwhile, as the server closes a connection that does not.
async fn next_of(ws: &mut Ws, kind: &str) -> (Value, Vec<String>) {
    let mut before = Vec::new();
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap_or_else(|_| panic!("a {kind} message within 5 s, after {before:?}"))
            .unwrap_or_else(|| panic!("open until a {kind} message, after {before:?}"))
            .unwrap();
        let t = match m {
            Message::Text(t) => t,
            Message::Close(c) => panic!("closed before a {kind} message, after {before:?}: {c:?}"),
            _ => continue,
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        faderz::contract::check(&faderz::contract::contract().server_message, &v)
            .unwrap_or_else(|e| panic!("{e}: {v}"));
        if v["type"] == kind {
            return (v, before);
        }
        if v["type"] == "ping" {
            let pong = json!({ "type": "pong", "t": v["t"] });
            ws.send(Message::text(pong.to_string())).await.unwrap();
            continue;
        }
        before.push(v["type"].as_str().unwrap().to_owned());
    }
}

/// The engine's counters reach a primary and a standby connection once
/// the hello is answered, then once a second, under this replica's id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_counters_reach_every_connection() {
    let domain = domain("counters");
    let data = data("counters");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let _ch1 = Source::start(&domain.0, &flows.ch1, vec![CH1]);
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.pgm).await;
    for (mode, session) in [
        ("primary", "session-p-0123456789abcdef"),
        ("standby", "session-s-0123456789abcdef"),
    ] {
        let url = format!("ws://{}/api/v1/ws?session={session}", server.api);
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let hello = json!({ "type": "hello", "session": session, "kind": "ui", "mode": mode });
        ws.send(Message::text(hello.to_string())).await.unwrap();
        let (first, before) = next_of(&mut ws, "counters").await;
        assert!(
            before.iter().any(|t| t == "snapshot"),
            "{mode}: after the hello's answer: {before:?}"
        );
        assert_eq!(first["replica"]["id"], "r1", "{mode}");
        let values = first["values"].as_object().unwrap();
        for name in [
            "deadline_miss_total",
            "overrun_skipped_samples_total",
            "clock_steps_total",
            "input_late_blocks_total",
            "input_too_late_blocks_total",
            "command_late_total",
            "strip_refused_total",
            "resends_total",
            "afv_late_total",
        ] {
            assert!(values[name].is_u64(), "{mode}: {name} in {values:?}");
        }
        // Every channel's input beside them, in shape order; ch2 and st1
        // read flows nobody writes, so the engine reads neither.
        let inputs = first["inputs"].as_array().unwrap();
        let channels: Vec<&str> = inputs
            .iter()
            .map(|i| i["channel"].as_str().unwrap())
            .collect();
        assert_eq!(channels, ["ch1", "ch2", "st1"], "{mode}");
        for i in &inputs[1..] {
            assert_ne!(i["state"], "attached", "{mode}: {i}");
        }
        // Each loudness measurement, the bus's and the mix-minus's.
        let loudness = first["loudness"].as_array().unwrap();
        let ids: Vec<&str> = loudness.iter().map(|l| l["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["pgm", "mm1"], "{mode}");
        for l in loudness {
            assert!(l["gaps"].is_u64() && l["lost"].is_u64(), "{mode}: {l}");
        }
        // Then once a second: two more within 3 s, not in one burst.
        let (_, _) = next_of(&mut ws, "counters").await;
        let at = Instant::now();
        let (_, _) = next_of(&mut ws, "counters").await;
        let gap = at.elapsed();
        assert!(
            gap >= Duration::from_millis(500) && gap <= Duration::from_secs(3),
            "{mode}: {gap:?} between two counters messages"
        );
    }
    server.stop().await;
}

/// The `loudness` entry of `id` in the `counters` message a new
/// connection to `server` gets after its hello's answer.
async fn loudness_of(server: &Server, id: &str) -> Value {
    // The counters are read once a second: a poll needs no more.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let session = "session-g-0123456789abcdef";
    let url = format!("ws://{}/api/v1/ws?session={session}", server.api);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let hello = json!({ "type": "hello", "session": session, "kind": "ui", "mode": "primary" });
    ws.send(Message::text(hello.to_string())).await.unwrap();
    let (m, _) = next_of(&mut ws, "counters").await;
    let all = m["loudness"].as_array().unwrap().clone();
    all.into_iter()
        .find(|l| l["id"] == id)
        .unwrap_or_else(|| panic!("no loudness of {id} in {m}"))
}

/// The `counters` message names each loudness measurement's start and
/// whether it restarted: without a reset in the state none did; a reset
/// of pgm starts pgm's there. An input that stops while it feeds the
/// programme is taken down by the engine's log: its slices count as
/// gaps, and the integrated loudness carries on over them, valid and at
/// the level it had. Started again on its data, the server never applied
/// the reset the state counts, so pgm's measurement restarted, and mm1's,
/// never reset, did not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_input_counts_gaps_and_the_integrated_loudness_holds() {
    const INTEGRATED: usize = PGM_PEAK + 8;
    const FLAGS: usize = PGM_PEAK + 13;
    let domain = domain("loud-gaps");
    let data = data("loud-gaps");
    let flows = Flows {
        ch1: new_id(),
        ch2: new_id(),
        ch2b: new_id(),
        st1: new_id(),
        pgm: new_id(),
        mm: new_id(),
    };
    let ch1 = Source::sine(&domain.0, &flows.ch1, 0.5, 440);
    let server = start(&data.0, &domain.0, &flows).await;
    appears(&domain.0, &flows.pgm).await;
    patch(&server, &[("/channels/ch1/fader", json!(0))]).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut pgm = loudness_of(&server, "pgm").await;
    while pgm["since"].is_null() {
        assert!(Instant::now() < deadline, "pgm measures: {pgm}");
        pgm = loudness_of(&server, "pgm").await;
    }
    assert_eq!(pgm["restarted"], false, "no reset in the state: {pgm}");
    // RFC 3339 UTC with milliseconds, as milliseconds since the epoch
    // (days from the civil date, H. Hinnant).
    let ms = |t: &Value| -> i64 {
        let t = t.as_str().unwrap();
        let n = |r: std::ops::Range<usize>| t[r].parse::<i64>().unwrap();
        let (m, d) = (n(5..7), n(8..10));
        let y = n(0..4) - i64::from(m <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        ((days * 86_400 + (n(11..13) * 60 + n(14..16)) * 60 + n(17..19)) * 1000) + n(20..23)
    };
    let asked = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let (status, body) = post(server.api, "/loudness/reset", &json!({ "ids": ["pgm"] })).await;
    assert_eq!(status, 204, "{body}");
    // Every read from the reset on, the ones before the engine placed it
    // included, shows pgm not restarted: a reset counted and not yet
    // placed is not a missed one.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        pgm = loudness_of(&server, "pgm").await;
        assert_eq!(
            pgm["restarted"], false,
            "read while the reset is placed: {pgm}"
        );
        if ms(&pgm["since"]) >= asked {
            break;
        }
        assert!(Instant::now() < deadline, "the reset starts pgm: {pgm}");
    }
    // Past the pending time, it started where the reset starts it.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let pgm_now = loudness_of(&server, "pgm").await;
    assert_eq!(pgm_now["restarted"], false, "{pgm_now}");

    // A steady tone: the integrated loudness settles.
    let mut level = i16::MIN;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let v = meter_values(&server, 1).await;
        if v[INTEGRATED] != i16::MIN && (v[INTEGRATED] - level).abs() <= 5 {
            break;
        }
        level = v[INTEGRATED];
        assert!(
            Instant::now() < deadline,
            "integrated loudness settles: {v:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // ch1 stops: the engine prunes it, its log takes it down, and the
    // programme's slices are gaps from then on.
    let gaps = pgm_now["gaps"].as_u64().unwrap();
    drop(ch1);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut pgm = pgm_now;
    while pgm["gaps"].as_u64().unwrap() <= gaps + 100 {
        assert!(Instant::now() < deadline, "no gaps counted: {pgm}");
        pgm = loudness_of(&server, "pgm").await;
    }
    assert_eq!(pgm["restarted"], false, "a gap restarts nothing: {pgm}");
    let v = meter_values(&server, 1).await;
    assert_eq!(v[FLAGS] & 1, 1, "integrated loudness valid: {v:?}");
    assert!(
        (v[INTEGRATED] - level).abs() <= 20,
        "integrated loudness {} after the gap, {level} before",
        v[INTEGRATED]
    );
    server.stop().await;

    let server = start(&data.0, &domain.0, &flows).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut pgm = loudness_of(&server, "pgm").await;
    while pgm["since"].is_null() {
        assert!(Instant::now() < deadline, "pgm measures again: {pgm}");
        pgm = loudness_of(&server, "pgm").await;
    }
    assert_eq!(
        pgm["restarted"], true,
        "pgm's reset was not applied here: {pgm}"
    );
    let mm1 = loudness_of(&server, "mm1").await;
    assert_eq!(mm1["restarted"], false, "mm1 was never reset: {mm1}");
    server.stop().await;
}

/// Both legs of every sample of `flow` from sample `from` up to `to`,
/// by sample index from `from`; every block must have been written.
fn span(domain: &Path, flow: &str, from: u64, to: u64) -> Vec<(f32, f32)> {
    let mut reader = Reader::new(domain, flow);
    let input = reader.generation.input(0).unwrap();
    let mut state = SlotState::default();
    let n = B as usize;
    let mut legs = Vec::new();
    for k in from / B..=to / B {
        let o = input.read(&mut state, k, &GroupWait::Skipped, &mut reader.buf);
        assert_eq!(o, Outcome::Copy, "block {k} of {flow}");
        let (l, r) = reader.buf.split_at(n);
        legs.extend(l.iter().copied().zip(r.iter().copied()));
    }
    let skip = (from % B) as usize;
    legs.drain(..skip);
    legs.truncate((to - from) as usize);
    legs
}

/// The first leg of `flow`, `legs` wide, from sample `from` up to `to`,
/// by sample index from `from`; every block must have been written.
fn span_left(domain: &Path, flow: &str, legs: u32, from: u64, to: u64) -> Vec<f32> {
    let mut reader = Reader::wide(domain, flow, legs);
    let input = reader.generation.input(0).unwrap();
    let mut state = SlotState::default();
    let n = B as usize;
    let mut left = Vec::new();
    for k in from / B..=to / B {
        let o = input.read(&mut state, k, &GroupWait::Skipped, &mut reader.buf);
        assert_eq!(o, Outcome::Copy, "block {k} of {flow}");
        left.extend_from_slice(&reader.buf[..n]);
    }
    left.drain(..(from % B) as usize);
    left.truncate((to - from) as usize);
    left
}

/// The commentary key, the ambience under it, mono and stereo.
const COMM: f32 = 0.5;
const AMB: [f32; 2] = [0.25, -0.25];
/// The duck group's settings: depth dB, attack, hold and release ms.
const DEPTH: f32 = 10.0;
const ATTACK_MS: u64 = 20;
const HOLD_MS: u64 = 200;
const RELEASE_MS: u64 = 300;
/// Samples per ms at 48 kHz.
const PER_MS: u64 = 48;
/// The detector decides once every 8 samples (samplez-dsp
/// `GAIN_INTERVAL`), and a sample's key acts from the next interval on:
/// an edge of the envelope lands within two intervals of the time the
/// settings give, whatever the block size.
const EDGE: u64 = 2 * 8;

/// A burst on the commentary channel ducks the ambience in the
/// programme output by the group's depth over its attack, holds it for
/// the hold time after the burst ends and recovers over the release,
/// each edge within [`EDGE`] samples, while the M&E output, carrying the
/// same ambience but no destination of the group, keeps every sample's
/// bits.
async fn a_burst_ducks_the_programme_but_not_the_me(block: u32) {
    let name = format!("duck-{block}");
    let domain = domain(&name);
    // The burst, hold and release fit in the domain's history.
    std::fs::write(
        domain.0.join("options.json"),
        r#"{"urn:x-mxl:option:history_duration/v1.0": 4000000000}"#,
    )
    .unwrap();
    let data = data(&name);
    let ids: Vec<String> = (0..4).map(|_| new_id()).collect();
    let stereo = |id: &str, source: &str| json!({ "id": id, "name": id, "source": source, "width": 2, "layout": "stereo" });
    let shape = json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": [
            { "id": "comm", "name": "Comm", "width": 1, "layout": "mono", "altInput": false },
            { "id": "amb", "name": "Amb", "width": 2, "layout": "stereo", "altInput": false },
        ],
        "buses": [
            { "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" },
            { "id": "me", "name": "M&E", "kind": "me", "width": 2, "layout": "stereo" },
        ],
        "auxes": [], "mixMinus": [], "vcas": [],
        "ducks": [{ "id": "talk", "name": "Commentary" }],
        "monitorOutputs": [{ "id": "mon", "name": "Monitor", "width": 2, "layout": "stereo" }],
        "outputs": [stereo("out-pgm", "pgm"), stereo("out-me", "me")],
    });
    // The commentary reads COMM over the samples [start, end), else
    // silence; set once the mix is up.
    let burst = Arc::new([AtomicU64::new(u64::MAX), AtomicU64::new(u64::MAX)]);
    let at = burst.clone();
    let _comm = Source::signal(&domain.0, &ids[0], 1, move |_, idx| {
        let on = (at[0].load(Relaxed)..at[1].load(Relaxed)).contains(&idx);
        if on { COMM } else { 0.0 }
    });
    let _amb = Source::start(&domain.0, &ids[1], AMB.to_vec());
    let server = start_engine(
        &data.0,
        &domain.0,
        shape,
        json!({ "comm": { "flow": ids[0] }, "amb": { "flow": ids[1] } }),
        json!({ "out-pgm": { "flow": ids[2] }, "out-me": { "flow": ids[3] } }),
        // The edges are read from one window of 2 s, which a debug engine
        // on a loaded host computes whole only with time in hand: inputs
        // read 100 ms late, and a backlog of 100 ms computed rather than
        // skipped. maxCatchupBlocks is set far off its default of two
        // blocks (2 ms at block 48) on purpose: this test is about the
        // duck, not about a late wake. A skip past the default is
        // samplez's engine's to test, and its tests cover it.
        json!({
            "blockSize": block,
            "readerMarginSamples": 100 * PER_MS,
            "maxCatchupBlocks": 100 * PER_MS / u64::from(block),
        }),
    )
    .await;
    appears(&domain.0, &ids[2]).await;
    appears(&domain.0, &ids[3]).await;
    // The commentary hard left in the programme only, keying the group
    // pre-fader; the ambience in both.
    patch(
        &server,
        &[
            ("/buses/pgm/fader", json!(0)),
            ("/buses/me/fader", json!(0)),
            ("/channels/comm/fader", json!(0)),
            ("/channels/comm/pan/lr", json!(-1)),
            ("/channels/comm/routes/pgm", json!(true)),
            ("/channels/comm/routes/me", json!(false)),
            ("/channels/amb/fader", json!(0)),
            ("/channels/amb/routes/pgm", json!(true)),
            ("/channels/amb/routes/me", json!(true)),
            ("/ducks/talk/channels/comm/trigger", json!(true)),
            ("/ducks/talk/channels/amb/target", json!(true)),
            ("/ducks/talk/channels/amb/depth", json!(DEPTH)),
            ("/ducks/talk/destinations/pgm", json!(true)),
            ("/ducks/talk/destinations/me", json!(false)),
            ("/ducks/talk/keyTap", json!("preFader")),
            // The key is a constant: no high-pass to take it out.
            ("/ducks/talk/keyHpf/enabled", json!(false)),
            ("/ducks/talk/attackMs", json!(ATTACK_MS)),
            ("/ducks/talk/holdMs", json!(HOLD_MS)),
            ("/ducks/talk/releaseMs", json!(RELEASE_MS)),
        ],
    )
    .await;
    let mut pgm = Reader::new(&domain.0, &ids[2]);
    let mut me = Reader::new(&domain.0, &ids[3]);
    pgm.expect(AMB[0], AMB[1], "the ambience, no commentary")
        .await;
    me.expect(AMB[0], AMB[1], "the M&E").await;

    let api = api();
    let (hold, release) = (HOLD_MS * PER_MS, RELEASE_MS * PER_MS);
    let near = |v: f32, want: f32| (v - want).abs() <= 1e-6;
    let engine = server.audio.as_ref().unwrap().engine();
    let skipped = || engine.counters().overrun_skipped_samples.load(Relaxed);
    let skipped_before = skipped();
    // A burst of 0.8 s from 0.5 s on, off the block grid.
    let now = time::timestamp_to_index(&api, time::now(&api));
    let (start, end) = (now + 500 * PER_MS + 7, now + 1300 * PER_MS + 7);
    burst[1].store(end, Relaxed);
    burst[0].store(start, Relaxed);
    let (from, to) = (start - 100 * PER_MS, end + hold + release + 100 * PER_MS);
    // Written by then, with the 100 ms the clock leads the output by.
    while time::timestamp_to_index(&api, time::now(&api)) <= to + 200 * PER_MS {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // libmxl zero-fills what an overrun skips, in both outputs.
    assert_eq!(
        skipped(),
        skipped_before,
        "block {block}: samples skipped, the engine was starved for over 100 ms"
    );
    // Both outputs carry a bus, so both have its limiter and are late by
    // its look-ahead.
    let late = samplez_dsp::LIMITER_DELAY as u64;
    let p = span(&domain.0, &ids[2], from + late, to + late);
    let m = span(&domain.0, &ids[3], from + late, to + late);
    // The commentary on the left leg (both legs ducked alike, so L + R)
    // over exactly the burst's samples, and silence elsewhere.
    let comm = |s: &(f32, f32)| s.0 + s.1;
    for (i, s) in p.iter().enumerate() {
        let want = if (start..end).contains(&(from + i as u64)) {
            COMM
        } else {
            0.0
        };
        assert!(
            near(comm(s), want),
            "block {block}: sample {i} carries a commentary of {}, not {want}",
            comm(s)
        );
    }
    let after = |what: &str, from: usize, f: &dyn Fn(f32, f32) -> bool| {
        (from..p.len())
            .find(|&i| f(p[i].0, p[i].1))
            .unwrap_or_else(|| panic!("block {block}: {what} not after sample {from}"))
    };
    // The burst's edges in the output: the commentary on the left leg.
    let on = after("the burst", 0, &|l, _| l > COMM);
    let off = after("the burst's end", on, &|l, _| l < COMM);
    // The ambience on the right leg starts down within an edge of the
    // burst and reaches its depth over the attack.
    let down = after("the duck", 0, &|_, r| !near(r, AMB[1]));
    let ducked = p[off - 1].1;
    let deep = after("the depth", down, &|_, r| r == ducked);
    let within = |what: &str, got: usize, want: u64| {
        assert!(
            (got as u64).abs_diff(want) <= EDGE,
            "block {block}: {what} took {got} samples, {want} +/- {EDGE}"
        );
    };
    assert!(
        (on..=on + EDGE as usize).contains(&down),
        "block {block}: the burst at {on}, the duck from {down}"
    );
    within("the attack", deep - down, ATTACK_MS * PER_MS);
    let depth = 20.0 * (ducked / AMB[1]).log10();
    assert!(
        (depth + DEPTH).abs() <= 0.5,
        "block {block}: ducked by {depth} dB, not -{DEPTH}"
    );
    assert!(
        p[deep..off].iter().all(|s| s.1 == ducked),
        "block {block}: steady at depth"
    );
    // After the burst: the hold, then the release back to the ambience.
    let up = after("the release", off, &|_, r| r != ducked);
    let back = after("the recovery", up, &|_, r| near(r, AMB[1]));
    within("the hold", up - off, hold);
    within("the release", back - up, release);
    assert!(
        p[back..].iter().all(|s| near(s.1, AMB[1])),
        "block {block}: recovered"
    );
    // The M&E carries the same ambience and never moves, bit for bit.
    let me_bits = |s: &(f32, f32)| (s.0.to_bits(), s.1.to_bits());
    let want = (AMB[0].to_bits(), AMB[1].to_bits());
    assert!(
        m.iter().all(|s| me_bits(s) == want),
        "block {block}: the M&E moved"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_ducks_the_programme_at_block_48() {
    a_burst_ducks_the_programme_but_not_the_me(48).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_ducks_the_programme_at_block_55() {
    a_burst_ducks_the_programme_but_not_the_me(55).await;
}
