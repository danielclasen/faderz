//! faderz-bridge: drives Mackie Control surfaces as faderz consoles.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use faderz_bridge::bridge::{self, Credentials, Options, Static};
use faderz_bridge::config::{Auth, Config, Transport, Unit};
use faderz_bridge::mcu::{self, Device};
use faderz_bridge::midi::{Link, Port};
use faderz_bridge::{auth, midi, rtpmidi, session};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

const USAGE: &str = "usage: faderz-bridge --config <file>
       faderz-bridge --list-ports
       faderz-bridge --version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = match args.as_slice() {
        [flag] if flag == "--version" => {
            println!("faderz-bridge {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        [flag] if flag == "--list-ports" => return list_ports(),
        [flag, path] if flag == "--config" => PathBuf::from(path),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(level())
        .init();
    match start(&config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("faderz-bridge: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The log level from `FADERZ_BRIDGE_LOG` (error, warn, info, debug,
/// trace); info when unset.
fn level() -> tracing::Level {
    std::env::var("FADERZ_BRIDGE_LOG")
        .ok()
        .and_then(|l| l.parse().ok())
        .unwrap_or(tracing::Level::INFO)
}

fn list_ports() -> ExitCode {
    match midi::list() {
        Ok((ins, outs)) => {
            for name in ins {
                println!("in   {name}");
            }
            for name in outs {
                println!("out  {name}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("faderz-bridge: {e}");
            ExitCode::FAILURE
        }
    }
}

fn start(path: &std::path::Path) -> Result<(), String> {
    let config = Config::load(path)?;
    let session = session::load_or_create(&config.session_file())
        .map_err(|e| format!("{}: {e}", config.session_file().display()))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let mut ports = Vec::new();
        for u in &config.units {
            ports.push(open(u).map_err(|e| format!("unit {}: {e}", u.id))?);
        }
        let mut renewed = None;
        let credentials: Arc<dyn Credentials> = match &config.auth {
            Auth::None => Arc::new(Static(None)),
            Auth::Token { token: Some(t), .. } => Arc::new(Static(Some(t.clone()))),
            Auth::Token {
                token_file: Some(f),
                ..
            } => {
                let token =
                    std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
                Arc::new(Static(Some(token.trim().to_owned())))
            }
            Auth::Token { .. } => unreachable!("checked when parsed"),
            Auth::Device {
                issuer,
                client_id,
                scopes,
            } => {
                let http = auth::client()?;
                let provider = auth::discover_retrying(&http, issuer).await;
                let scope = std::iter::once("openid")
                    .chain(scopes.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" ");
                let sign_in = auth::Session {
                    http,
                    provider,
                    client_id: client_id.clone(),
                    scope,
                    file: config.tokens_file(),
                    retry: auth::START_RETRY,
                };
                let lcd = ports
                    .first()
                    .map(|p| p.output.clone())
                    .zip(config.unit_specs().first().map(|u| u.device));
                // A network surface that connects while the code is up
                // is shown it too.
                let shown: Arc<Mutex<Option<auth::DeviceCode>>> = Arc::default();
                let again = ports
                    .first()
                    .and_then(|p| p.link.clone())
                    .map(|link| show_on_connect(link, shown.clone(), lcd.clone()));
                let show = move |code: &auth::DeviceCode| {
                    *shown.lock().expect("not poisoned") = Some(code.clone());
                    show_code(code, lcd.as_ref());
                };
                let tokens = sign_in.sign_in(&show).await;
                if let Some(again) = again {
                    again.abort();
                }
                let tokens = tokens?;
                tracing::info!("signed in");
                let signed = auth::Signed::new(tokens);
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                renewed = Some(rx);
                let keeper = signed.clone();
                tokio::spawn(async move { sign_in.keep_fresh(keeper, tx, &show).await });
                signed
            }
        };
        let options = Options {
            endpoints: config.endpoints.clone(),
            session,
            credentials,
            units: config.unit_specs(),
            master: config.master.clone(),
            renewed,
        };
        bridge::run(options, ports, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
        Ok(())
    })
}

/// Opens a unit's transport.
fn open(u: &Unit) -> Result<Port, String> {
    match u.transport {
        Transport::Midi => {
            let input = u.midi_in.as_deref().expect("checked when parsed");
            midi::open(input, u.midi_out.as_deref().unwrap_or(input))
        }
        Transport::RtpMidi => rtpmidi::open(u.rtp_options()?.expect("checked when parsed")),
    }
}

/// Shows where to sign in and the code: on stderr, and on the first
/// unit's scribble strip.
fn show_code(code: &auth::DeviceCode, lcd: Option<&(UnboundedSender<Vec<u8>>, Device)>) {
    eprintln!(
        "faderz-bridge: to sign in, open {} and enter the code {}",
        code.verification_uri, code.user_code
    );
    if let Some(complete) = &code.verification_uri_complete {
        eprintln!("faderz-bridge: or open {complete}");
    }
    show_lcd(code, lcd);
}

/// Shows the code in `shown` on `lcd` again whenever `link` connects: a
/// network surface that connects after the code came shows it too.
fn show_on_connect(
    mut link: watch::Receiver<Link>,
    shown: Arc<Mutex<Option<auth::DeviceCode>>>,
    lcd: Option<(UnboundedSender<Vec<u8>>, Device)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut was = link.borrow_and_update().connected;
        while link.changed().await.is_ok() {
            let connected = link.borrow_and_update().connected;
            let code = shown.lock().expect("not poisoned").clone();
            if let (false, true, Some(code)) = (was, connected, code) {
                show_lcd(&code, lcd.as_ref());
            }
            was = connected;
        }
    })
}

fn show_lcd(code: &auth::DeviceCode, lcd: Option<&(UnboundedSender<Vec<u8>>, Device)>) {
    if let Some((out, device)) = lcd {
        let line = |text: String| format!("{text:<width$.width$}", width = mcu::LINE_CHARS);
        let _ = out.send(mcu::lcd(
            *device,
            0,
            &line(format!("Sign in: {}", code.verification_uri)),
        ));
        let _ = out.send(mcu::lcd(
            *device,
            mcu::LINE_CHARS,
            &line(format!("Code: {}", code.user_code)),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_surface_that_connects_during_a_sign_in_is_shown_the_code() {
        let (link, rx) = watch::channel(Link::default());
        let (out, mut lcd) = tokio::sync::mpsc::unbounded_channel();
        let shown = Arc::new(Mutex::new(None));
        let task = show_on_connect(rx, shown.clone(), Some((out, Device::Main)));
        let wait = || tokio::time::sleep(std::time::Duration::from_millis(50));
        // No code yet: nothing to show.
        link.send_modify(|l| l.connected = true);
        wait().await;
        assert!(lcd.try_recv().is_err());
        let code: auth::DeviceCode = serde_json::from_str(
            r#"{ "device_code": "d", "user_code": "WXYZ-1234",
                 "verification_uri": "https://idp.example/device", "expires_in": 600 }"#,
        )
        .unwrap();
        *shown.lock().unwrap() = Some(code);
        // Going away shows nothing; coming back shows both lines.
        link.send_modify(|l| l.connected = false);
        wait().await;
        assert!(lcd.try_recv().is_err());
        link.send_modify(|l| {
            l.connected = true;
            l.session += 1;
        });
        wait().await;
        let first = mcu::parse_lcd(&lcd.try_recv().unwrap()).unwrap();
        let second = mcu::parse_lcd(&lcd.try_recv().unwrap()).unwrap();
        assert!(first.2.starts_with("Sign in: https://idp.example/device"));
        assert_eq!(second.1, mcu::LINE_CHARS);
        assert!(second.2.starts_with("Code: WXYZ-1234"));
        // A resend while connected is no new connection.
        link.send_modify(|l| l.resync += 1);
        wait().await;
        assert!(lcd.try_recv().is_err());
        task.abort();
    }
}
