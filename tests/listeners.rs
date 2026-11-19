//! The faderz binary's log of its listeners, which the bridge's
//! end-to-end tests take as faderz holding its own ports.

use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::json;

/// The `nmos listener on` line comes once the api, ops and nmos
/// listeners are all bound, after the lines of the other two: each of
/// the three addresses takes a connection when it is read.
#[test]
fn the_nmos_listener_is_logged_last_and_once_every_listener_is_bound() {
    let dir = std::env::temp_dir().join(format!("faderz-listeners-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let channels: Vec<_> = (1..=2)
        .map(|i| json!({ "id": format!("ch{i}"), "name": format!("Chan {i}"), "width": 1, "layout": "mono", "altInput": false }))
        .collect();
    let any = json!({ "bind": "127.0.0.1", "port": 0 });
    let config = json!({
        "listeners": { "api": any, "ops": any, "nmos": any },
        "replica": { "name": "r1", "dataDir": dir.join("data") },
        "auth": { "mode": "none" },
        "shape": {
            "sampleRate": 48000,
            "frameRate": { "numerator": 25, "denominator": 1 },
            "channels": channels,
            "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
            "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [], "outputs": [],
        },
    });
    let path = dir.join("config.json");
    std::fs::write(&path, config.to_string()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_faderz"))
        .arg("--config")
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = BufReader::new(child.stderr.take().unwrap());
    let addr = |line: &str, name: &str| -> Option<SocketAddr> {
        line.strip_prefix(&format!("faderz: {name} listener on "))?
            .parse()
            .ok()
    };
    let (mut api, mut ops) = (None, None);
    let mut seen = Vec::new();
    let mut nmos = None;
    for line in stderr.lines().map_while(Result::ok) {
        api = api.or_else(|| addr(&line, "api"));
        ops = ops.or_else(|| addr(&line, "ops"));
        if let Some(a) = addr(&line, "nmos") {
            nmos = Some(a);
            break;
        }
        seen.push(line);
    }
    // Checked while the process runs, as soon as the line is read.
    let reached = |a: Option<SocketAddr>| {
        a.is_some_and(|a| TcpStream::connect_timeout(&a, Duration::from_secs(1)).is_ok())
    };
    let listening = (reached(api), reached(ops), reached(nmos));
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        api.is_some() && ops.is_some() && nmos.is_some(),
        "api and ops listener lines before the nmos one: {api:?} {ops:?} {nmos:?} after {seen:?}"
    );
    assert_eq!(
        listening,
        (true, true, true),
        "listening on {api:?} {ops:?} {nmos:?}"
    );
}
