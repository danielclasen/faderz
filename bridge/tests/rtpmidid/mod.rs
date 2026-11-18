//! The RTP-MIDI transport against rtpmidid, the RTP-MIDI daemon of a
//! Linux laptop: the emulator plays a surface on a raw MIDI device (a
//! pseudo-terminal), rtpmidid carries that device over an RTP-MIDI
//! session with the bridge, and the bridge drives faderz. Needs rtpmidid
//! (`RTPMIDID_BIN`, else on the PATH) and a C compiler with the ALSA
//! headers for `noseq.c`, which lets rtpmidid start without the ALSA
//! sequencer.

use super::*;

use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::FromRawFd;
use std::path::Path;
use std::sync::OnceLock;

use faderz_bridge::midi;
use faderz_bridge::rtpmidi::{self, Cidr};

/// `noseq.c`, built once.
fn noseq() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("noseq-{}", session::random_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("noseq.so");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/rtpmidid/noseq.c");
        let status = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
            .args(["-shared", "-fPIC", "-Wall", "-Werror", "-o"])
            .arg(&out)
            .arg(&source)
            .arg("-lasound")
            .status()
            .expect("a C compiler");
        assert!(status.success(), "building noseq.c");
        out
    })
}

/// A pseudo-terminal in raw mode: the master end, and the path of the
/// device rtpmidid opens.
fn pty() -> (File, PathBuf, File) {
    // SAFETY: plain libc calls on file descriptors owned here.
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(master >= 0);
        assert_eq!(libc::grantpt(master), 0);
        assert_eq!(libc::unlockpt(master), 0);
        let mut name = [0 as libc::c_char; 128];
        assert_eq!(libc::ptsname_r(master, name.as_mut_ptr(), name.len()), 0);
        let path = std::ffi::CStr::from_ptr(name.as_ptr())
            .to_str()
            .unwrap()
            .to_owned();
        // The terminal settings belong to the device: raw, so that no
        // byte is echoed or translated, kept while this end is open.
        let slave = File::options().read(true).write(true).open(&path).unwrap();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&slave);
        let mut t: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(fd, &mut t), 0);
        libc::cfmakeraw(&mut t);
        assert_eq!(libc::tcsetattr(fd, libc::TCSANOW, &t), 0);
        (File::from_raw_fd(master), PathBuf::from(path), slave)
    }
}

/// The octets after a status: complete MIDI messages, running status
/// and SysEx included.
fn messages(bytes: &mut Vec<u8>, running: &mut Option<u8>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let Some(&first) = bytes.first() else {
            return out;
        };
        if first == 0xf0 {
            let Some(end) = bytes.iter().position(|&b| b == 0xf7) else {
                return out;
            };
            out.push(bytes.drain(..=end).collect());
            continue;
        }
        let (status, at) = if first & 0x80 != 0 {
            (first, 1)
        } else if let Some(s) = *running {
            (s, 0)
        } else {
            bytes.remove(0);
            continue;
        };
        let n = match status {
            0xc0..=0xdf | 0xf1 | 0xf3 => 1,
            0x80..=0xef | 0xf2 => 2,
            _ => 0,
        };
        if bytes.len() < at + n {
            return out;
        }
        if status < 0xf0 {
            *running = Some(status);
        }
        let mut m = vec![status];
        m.extend(bytes.drain(..at + n).skip(at));
        out.push(m);
    }
}

/// A surface on the pseudo-terminal's master end.
fn surface(master: File) -> Emulator {
    let (port, to_bridge, mut from_bridge) = midi::pair();
    let mut reader = master.try_clone().unwrap();
    std::thread::spawn(move || {
        let (mut bytes, mut running, mut buf) = (Vec::new(), None, [0u8; 1024]);
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
            for m in messages(&mut bytes, &mut running) {
                if to_bridge.send(m).is_err() {
                    return;
                }
            }
        }
    });
    let mut writer = master;
    std::thread::spawn(move || {
        while let Some(m) = from_bridge.blocking_recv() {
            if writer.write_all(&m).is_err() {
                break;
            }
        }
    });
    Emulator::over(Device::Main, port)
}

/// An rtpmidid process with its own control socket, on the raw MIDI
/// device `device`.
struct Rtpmidid {
    child: Child,
    dir: PathBuf,
}

impl Rtpmidid {
    /// Starts rtpmidid with a `[rawmidi]` section of `keys`.
    fn start(device: &Path, keys: &str) -> Rtpmidid {
        let id = session::random_id().unwrap();
        let dir = std::env::temp_dir().join(format!("rtpmidid-{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        let control = dir.join("control.sock");
        let ini = format!(
            "[general]\nalsa_name=rtpmidid-{}\ncontrol={}\nlog_level=info\n\n\
             [rawmidi]\ndevice={}\nname=laptop\n{keys}",
            &id[..8],
            control.display(),
            device.display()
        );
        let path = dir.join("rtpmidid.ini");
        std::fs::write(&path, ini).unwrap();
        let log = File::create(dir.join("log")).unwrap();
        let bin = std::env::var("RTPMIDID_BIN").unwrap_or_else(|_| "rtpmidid".into());
        let child = Command::new(bin)
            .arg("--ini")
            .arg(&path)
            .env("LD_PRELOAD", noseq())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("rtpmidid on the PATH, or RTPMIDID_BIN");
        let mut r = Rtpmidid { child, dir };
        for _ in 0..100 {
            if control.exists() {
                return r;
            }
            if let Ok(Some(status)) = r.child.try_wait() {
                panic!("rtpmidid exited: {status}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("rtpmidid did not come up");
    }

    /// Ends it without a bye, as a laptop that loses power.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Rtpmidid {
    fn drop(&mut self) {
        self.kill();
        if std::thread::panicking() {
            let mut log = String::new();
            if let Ok(mut f) = File::open(self.dir.join("log")) {
                let _ = f.read_to_string(&mut log);
            }
            eprintln!("rtpmidid's log:\n{log}");
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A control port whose data port above it is free too.
fn udp_ports() -> u16 {
    loop {
        let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let p = s.local_addr().unwrap().port();
        if p < 65535 && std::net::UdpSocket::bind(("127.0.0.1", p + 1)).is_ok() {
            return p;
        }
    }
}

/// A bridge with one X-Touch on an RTP-MIDI session; returns its
/// session's handle and what stops it.
fn bridge(f: &Faderz, rtp: rtpmidi::Options) -> (String, oneshot::Sender<()>) {
    let session = session::random_id().unwrap();
    let handle = session::handle(&session);
    let port = rtpmidi::open(rtp).unwrap();
    let options = Options {
        endpoints: vec![f.endpoint()],
        session,
        credentials: Arc::new(Static(None)),
        units: vec![UnitSpec {
            id: "xt".into(),
            name: "X-Touch".into(),
            device: Device::Main,
        }],
        master: None,
        renewed: None,
    };
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(bridge::run(options, vec![port], async {
        let _ = stopped.await;
    }));
    (handle, stop)
}

fn rtp(port: u16, allow: &[&str], invite: Option<u16>) -> rtpmidi::Options {
    rtpmidi::Options {
        bind: "127.0.0.1".parse().unwrap(),
        port,
        name: format!("faderz {port}"),
        allow: allow.iter().map(|a| a.parse::<Cidr>().unwrap()).collect(),
        invite: invite.map(|p| ([127, 0, 0, 1], p).into()),
        max_bytes_per_second: 32_000,
    }
}

async fn connected(screen: &mut Screen, handle: &str) {
    screen
        .until(|m| {
            console(m, "xt").is_some_and(|c| c["connected"] == true && c["handle"] == handle)
        })
        .await;
}

#[test]
fn the_midi_stream_reader_splits_messages() {
    let mut bytes = vec![0x90, 1, 2, 3, 4, 0xf0, 5, 6, 0xf7, 0xe0, 7];
    let mut running = None;
    assert_eq!(
        messages(&mut bytes, &mut running),
        vec![vec![0x90, 1, 2], vec![0x90, 3, 4], vec![0xf0, 5, 6, 0xf7]]
    );
    assert_eq!(bytes, vec![0xe0, 7]);
    bytes.push(8);
    assert_eq!(messages(&mut bytes, &mut running), vec![vec![0xe0, 7, 8]]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtpmidid_joins_the_bridge_and_a_new_session_gets_the_whole_state() {
    let f = Faderz::start().await;
    let mut screen = Screen::connect(&f).await;
    let port = udp_ports();
    let (handle, _stop) = bridge(&f, rtp(port, &["127.0.0.1"], None));
    let (master, device, _slave) = pty();
    let mut s = surface(master);
    let mut laptop = Rtpmidid::start(
        &device,
        &format!("hostname=127.0.0.1\nremote_udp_port={port}\n"),
    );
    connected(&mut screen, &handle).await;

    // Names over SysEx, from the bridge to the surface.
    assert!(
        s.until(WAIT, |e| e.cell(0, 0) == "Chan 1 "
            && e.cell(0, 7) == "Chan 8 ")
            .await
    );
    // Touch and fader from the surface to faderz.
    s.touch(0);
    s.move_fader(0, taper::db_to_fader(-10.0));
    screen
        .until(|m| m.get("/channels/ch1/fader") == Some(&json!(-10.0)))
        .await;
    s.untouch(0);
    screen
        .until(|m| !m.owners.contains_key("/channels/ch1/fader"))
        .await;
    s.press(Key::Mute(2));
    screen
        .until(|m| m.get("/channels/ch3/mute") == Some(&json!(true)))
        .await;
    // A screen's writes move the motor and light the LED.
    screen
        .command("set", "/channels/ch2/fader", Some(json!(-20)))
        .await;
    let motor = taper::db_to_fader(-20.0);
    assert!(
        s.until(WAIT, |e| e.faders[1] == motor
            && e.cell(1, 1) == "-20.0  "
            && e.led(Key::Mute(2)) == Led::On)
            .await
    );

    // The laptop's daemon dies without a bye and a new one starts a new
    // session: the surface gets the whole state again, though nothing in
    // it changed.
    laptop.kill();
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.pump();
    s.faders = [0; 9];
    s.leds.clear();
    s.lcd = [b' '; 2 * faderz_bridge::mcu::LINE_CHARS];
    let _laptop = Rtpmidid::start(
        &device,
        &format!("hostname=127.0.0.1\nremote_udp_port={port}\n"),
    );
    assert!(
        s.until(WAIT, |e| e.cell(0, 0) == "Chan 1 "
            && e.faders[1] == motor
            && e.cell(1, 1) == "-20.0  "
            && e.led(Key::Mute(2)) == Led::On)
            .await
    );
    s.touch(3);
    s.move_fader(3, taper::db_to_fader(-6.0));
    screen
        .until(|m| m.get("/channels/ch4/fader") == Some(&json!(-6.0)))
        .await;
    s.untouch(3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bridge_invites_rtpmidid() {
    let f = Faderz::start().await;
    let mut screen = Screen::connect(&f).await;
    let theirs = udp_ports();
    let (master, device, _slave) = pty();
    let mut s = surface(master);
    let _desk = Rtpmidid::start(&device, &format!("local_udp_port={theirs}\n"));
    let ours = udp_ports();
    let (handle, _stop) = bridge(&f, rtp(ours, &[], Some(theirs)));
    connected(&mut screen, &handle).await;
    assert!(s.until(WAIT, |e| e.cell(0, 0) == "Chan 1 ").await);

    s.touch(0);
    s.move_fader(0, taper::db_to_fader(-3.0));
    screen
        .until(|m| m.get("/channels/ch1/fader") == Some(&json!(-3.0)))
        .await;
    s.untouch(0);
    screen
        .command("set", "/channels/ch1/mute", Some(json!(true)))
        .await;
    assert!(s.until(WAIT, |e| e.led(Key::Mute(0)) == Led::On).await);
}
