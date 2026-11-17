//! faderz-bridge: drives Mackie Control surfaces as faderz consoles.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use faderz_bridge::bridge::{self, Credentials, Options, Static};
use faderz_bridge::config::{Auth, Config};
use faderz_bridge::mcu::{self, Device};
use faderz_bridge::{auth, midi, session};
use tokio::sync::mpsc::UnboundedSender;

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
    let mut ports = Vec::new();
    for u in &config.units {
        let output = u.midi_out.as_deref().unwrap_or(&u.midi_in);
        ports.push(midi::open(&u.midi_in, output).map_err(|e| format!("unit {}: {e}", u.id))?);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
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
                let show = move |code: &auth::DeviceCode| show_code(code, lcd.as_ref());
                let tokens = sign_in.sign_in(&show).await?;
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
