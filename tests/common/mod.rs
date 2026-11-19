//! Helpers of the integration tests: MXL domains and producers, the
//! REST API, and the parts of a replica set the pod would give.

#![allow(dead_code)]

pub mod registry;

use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use faderz::server::Server;
use futures_util::future::BoxFuture;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use samplez_mxl::output::{self, Creation, OutputConfig};
use samplez_mxl::{Instance, MxlApi, time};
use samplez_replica::bootstrap::{Marker, Markers};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const B: u64 = 48;

pub fn libmxl() -> PathBuf {
    let path = PathBuf::from(
        std::env::var("SAMPLEZ_TEST_LIBMXL").unwrap_or_else(|_| "/opt/mxl/lib/libmxl.so".into()),
    );
    assert!(path.is_file(), "libmxl not found at {}", path.display());
    path
}

pub fn api() -> Arc<MxlApi> {
    samplez_mxl::load(&libmxl()).unwrap()
}

/// A directory removed on drop.
pub struct Dir(pub PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn dir(base: &Path, name: &str) -> Dir {
    let dir = base.join(format!("faderz-audio-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Dir(dir)
}

/// An MXL domain on tmpfs, with a 1 s history, named for its test.
pub fn domain(name: &str) -> Dir {
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

pub fn new_id() -> String {
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

/// A random id of the form BCP-007-03 takes for MXL ids: version 4,
/// RFC 4122 variant.
pub fn mxl_id() -> String {
    let mut id = new_id().into_bytes();
    (id[14], id[19]) = (b'4', b'8');
    String::from_utf8(id).unwrap()
}

/// A producer writing `values`, one per channel, into every sample of a
/// flow as libmxl's clock passes each block. Dropping it deletes the flow.
pub struct Source {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Source {
    pub fn start(domain: &Path, id: &str, values: Vec<f32>) -> Source {
        let channels = values.len();
        Source::signal(domain, id, channels, move |ch, _| values[ch])
    }

    /// A sine of `freq` Hz at amplitude `amp`, exact per sample index.
    pub fn sine(domain: &Path, id: &str, amp: f32, freq: u64) -> Source {
        Source::signal(domain, id, 1, move |_, idx| {
            let phase = (freq * idx % 48_000) as f64 / 48_000.0;
            amp * (2.0 * std::f64::consts::PI * phase).sin() as f32
        })
    }

    /// A producer writing `signal(channel, index)` into every sample.
    pub fn signal(
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

/// Writes `changes` (path, value) in one PATCH /state.
pub async fn patch(server: &Server, changes: &[(&str, Value)]) {
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

/// POST one source's tally change to the server.
pub async fn tally(server: &Server, source: &str, state: &str) {
    let body = json!({ "source": source, "state": state }).to_string();
    let mut tcp = TcpStream::connect(server.api).await.unwrap();
    let request = format!(
        "POST /api/v1/afv/tally HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
         X-Samplez-Request: 1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    tcp.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    tcp.read_to_string(&mut answer).await.unwrap();
    assert!(
        answer.starts_with("HTTP/1.1 200"),
        "{source} {state}: {answer}"
    );
}

/// POST `body` to `path` of `addr`: status and body.
pub async fn post(addr: std::net::SocketAddr, path: &str, body: &Value) -> (u16, String) {
    let body = body.to_string();
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST /api/v1{path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
         X-Samplez-Request: 1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
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

/// GET `path` from `addr`: status and body.
pub async fn get(addr: std::net::SocketAddr, path: &str) -> (u16, String) {
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

/// The marker ConfigMap, in memory.
#[derive(Default)]
pub struct Memory(Mutex<Option<Marker>>);

impl Markers for Memory {
    fn name(&self) -> &str {
        "memory"
    }

    fn get(&self) -> BoxFuture<'_, Result<Option<Marker>, String>> {
        let marker = *self.0.lock().unwrap();
        Box::pin(async move { Ok(marker) })
    }

    fn create(&self, marker: Marker) -> BoxFuture<'_, Result<bool, String>> {
        let mut held = self.0.lock().unwrap();
        let created = held.is_none();
        if created {
            *held = Some(marker);
        }
        Box::pin(async move { Ok(created) })
    }
}

/// A TCP port free now, below the kernel's ephemeral range: a socket a
/// test binds to port 0 is never given it, so it stays free until the
/// listener it is meant for binds it, where a port taken from port 0
/// and let go can be given to another test first. The process hands
/// each out once.
pub fn reserved_port() -> u16 {
    static TAKEN: Mutex<std::collections::BTreeSet<u16>> =
        Mutex::new(std::collections::BTreeSet::new());
    let low = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .ok()
        .and_then(|r| r.split_whitespace().next()?.parse::<u16>().ok())
        .unwrap_or(32768);
    let span = u64::from(low.saturating_sub(10_000).max(1));
    let mut taken = TAKEN.lock().unwrap();
    loop {
        let draw = RandomState::new().hash_one(std::time::Instant::now()) % span;
        let port = 10_000 + u16::try_from(draw).unwrap();
        if !taken.contains(&port) && std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            taken.insert(port);
            return port;
        }
    }
}

/// A directory under the target directory, removed when dropped.
pub fn scratch(name: &str) -> Dir {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("faderz-set-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Dir(dir)
}

/// A CA, and per replica a certificate it issued for `localhost`: the
/// `replica.peer.tls` of each.
pub fn certificates(at: &Path, replicas: usize) -> Vec<Value> {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
    let ca_file = at.join("ca.crt");
    std::fs::write(&ca_file, ca.pem()).unwrap();
    (0..replicas)
        .map(|n| {
            let key = KeyPair::generate().unwrap();
            let cert = CertificateParams::new(vec!["localhost".into()])
                .unwrap()
                .signed_by(&key, &ca)
                .unwrap();
            let (cert_file, key_file) = (at.join(format!("{n}.crt")), at.join(format!("{n}.key")));
            std::fs::write(&cert_file, cert.pem()).unwrap();
            std::fs::write(&key_file, key.serialize_pem()).unwrap();
            json!({ "mode": "files", "certFile": cert_file, "keyFile": key_file,
                    "clientCaFile": ca_file })
        })
        .collect()
}
