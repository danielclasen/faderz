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
    for warning in config.warnings() {
        tracing::warn!("{warning}");
    }
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
        let mut sign_in_shown = None;
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
                    writer: auth::store,
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
                    *shown.lock().expect("shown lock poisoned") = Some(code.clone());
                    show_code(code, lcd.as_ref());
                };
                let tokens = sign_in.sign_in(&show).await;
                if let Some(again) = again {
                    again.abort();
                }
                let tokens = tokens?;
                tracing::info!("signed in");
                let signed = auth::Signed::new(tokens);
                // While the bridge runs, a new sign-in goes through the
                // surface, so that what it sends again keeps the code up;
                // the next token takes it down.
                let (lines, lines_rx) = watch::channel(None);
                sign_in_shown = Some(lines_rx);
                let lines = Arc::new(lines);
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let (fwd, fwd_rx) = tokio::sync::mpsc::unbounded_channel();
                renewed = Some(fwd_rx);
                let shown = lines.clone();
                tokio::spawn(async move {
                    while rx.recv().await.is_some() {
                        shown.send_replace(None);
                        if fwd.send(()).is_err() {
                            break;
                        }
                    }
                });
                let show = move |code: &auth::DeviceCode| {
                    print_code(code);
                    lines
                        .send_replace(Some(sign_in_lines(&code.verification_uri, &code.user_code)));
                };
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
            keys: config.keys.clone(),
            clock_offset: config.clock.map(|c| c.utc_offset),
            renewed,
            sign_in: sign_in_shown,
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
    print_code(code);
    show_lcd(code, lcd);
}

/// Prints where to sign in and the code on stderr.
fn print_code(code: &auth::DeviceCode) {
    eprintln!(
        "faderz-bridge: to sign in, open {} and enter the code {}",
        code.verification_uri, code.user_code
    );
    if let Some(complete) = &code.verification_uri_complete {
        eprintln!("faderz-bridge: or open {complete}");
    }
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
            let code = shown.lock().expect("shown lock poisoned").clone();
            if let (false, true, Some(code)) = (was, connected, code) {
                show_lcd(&code, lcd.as_ref());
            }
            was = connected;
        }
    })
}

fn show_lcd(code: &auth::DeviceCode, lcd: Option<&(UnboundedSender<Vec<u8>>, Device)>) {
    if let Some((out, device)) = lcd {
        let [top, bottom] = sign_in_lines(&code.verification_uri, &code.user_code);
        let _ = out.send(mcu::lcd(*device, 0, &top));
        let _ = out.send(mcu::lcd(*device, mcu::LINE_CHARS, &bottom));
        let _ = out.send(mcu::colours(*device, [mcu::WHITE; 8]));
    }
}

/// The characters per line the sign-in URL gets: the last five strips.
const URL_CHARS: usize = mcu::LINE_CHARS - 3 * mcu::STRIP_CHARS;

/// The scribble strip's two lines for a sign-in, laid out on its 8 cells
/// of 7 characters, each its own display: the code on the first two
/// strips, split at its hyphen, "Sign in" on the third, and the URL
/// without "https://" over both lines of the last five, broken after a
/// slash where the rest then fits. Text past the cells is cut; the log
/// has it all.
fn sign_in_lines(uri: &str, code: &str) -> [String; 2] {
    let w = mcu::STRIP_CHARS;
    let cell = |text: &str| format!("{text:<w$.w$}");
    let (first, second) = match code.split_once('-') {
        Some((a, b)) if a.len() < w && b.len() <= w => (format!("{a}-"), b.to_owned()),
        _ => (
            code.chars().take(w).collect(),
            code.chars().skip(w).collect(),
        ),
    };
    let url: Vec<char> = uri
        .strip_prefix("https://")
        .unwrap_or(uri)
        .chars()
        .collect();
    let width = URL_CHARS;
    let at = if url.len() <= width {
        url.len()
    } else {
        // A slash break that pushes the rest past the bottom line would
        // cut a URL that fits.
        url[..width]
            .iter()
            .rposition(|&c| c == '/')
            .map(|i| i + 1)
            .filter(|&at| url.len() - at <= width)
            .unwrap_or(width)
    };
    let top: String = url[..at].iter().collect();
    let bottom: String = url[at..].iter().collect();
    let line = |text: String| format!("{text:<n$.n$}", n = mcu::LINE_CHARS);
    [
        line(format!(
            "{}{}{}{top}",
            cell("Code:"),
            cell(""),
            cell("Sign in")
        )),
        line(format!(
            "{}{}{}{bottom}",
            cell(&first),
            cell(&second),
            cell("")
        )),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cells of a line, one per strip.
    fn cells(line: &str) -> Vec<&str> {
        let w = mcu::STRIP_CHARS;
        (0..mcu::LINE_CHARS / w)
            .map(|s| &line[s * w..s * w + w])
            .collect()
    }

    /// The URL's part of a line: the last five strips.
    fn url(line: &str) -> &str {
        line[3 * mcu::STRIP_CHARS..].trim_end()
    }

    #[test]
    fn a_sign_in_fits_the_scribble_strips_cell_by_cell() {
        let [top, bottom] = sign_in_lines(
            "https://idp.broadcast-facility.example.org/realms/mix/device",
            "TTBL-FLPH",
        );
        assert_eq!(top.len(), mcu::LINE_CHARS);
        assert_eq!(bottom.len(), mcu::LINE_CHARS);
        assert_eq!(&cells(&top)[..3], ["Code:  ", "       ", "Sign in"]);
        assert_eq!(&cells(&bottom)[..3], ["TTBL-  ", "FLPH   ", "       "]);
        // The URL over the last five strips, broken after a slash.
        assert_eq!(url(&top), "idp.broadcast-facility.example.org/");
        assert_eq!(url(&bottom), "realms/mix/device");
    }

    #[test]
    fn a_slash_just_past_the_top_line_goes_to_the_bottom_line() {
        let host = "auth.broadcast-facility.example.org";
        assert_eq!(host.len(), URL_CHARS);
        let [top, bottom] = sign_in_lines(&format!("https://{host}/device"), "AB-CD");
        assert_eq!(url(&top), host);
        assert_eq!(url(&bottom), "/device");
    }

    #[test]
    fn a_url_that_fits_is_not_broken_at_a_slash_that_would_cut_it() {
        let [top, bottom] = sign_in_lines(
            "https://idp.example/device-authorization-for-the-console-bridge",
            "AB-CD",
        );
        assert_eq!(url(&top), "idp.example/device-authorization-fo");
        assert_eq!(url(&bottom), "r-the-console-bridge");
    }

    #[test]
    fn a_plain_http_url_keeps_its_scheme() {
        let [top, _] = sign_in_lines("http://127.0.0.1:8080/device", "AB-CD");
        assert_eq!(url(&top), "http://127.0.0.1:8080/device");
    }

    #[test]
    fn a_code_is_split_at_its_first_hyphen_only_where_both_halves_fit() {
        let code = |code: &str| {
            let [_, bottom] = sign_in_lines("https://idp.example/device", code);
            [cells(&bottom)[0].to_owned(), cells(&bottom)[1].to_owned()]
        };
        assert_eq!(code("ABCDEF-GHI"), ["ABCDEF-", "GHI    "]);
        // No room for the hyphen after a left half of 7: split by cell.
        assert_eq!(code("ABCDEFG-HI"), ["ABCDEFG", "-HI    "]);
        assert_eq!(code("ABCDEFGH-IJ"), ["ABCDEFG", "H-IJ   "]);
        assert_eq!(code("AB-CD-EF"), ["AB-    ", "CD-EF  "]);
        // Past two cells the code is cut; the log has it whole.
        assert_eq!(code("ABCDEFGHIJKLMNOP"), ["ABCDEFG", "HIJKLMN"]);
    }

    #[test]
    fn a_code_or_url_that_does_not_fit_is_cut_rather_than_spilled() {
        let [top, bottom] = sign_in_lines("https://a.example/x", "ABCDEFGHIJ");
        assert_eq!(&cells(&bottom)[..2], ["ABCDEFG", "HIJ    "]);
        assert_eq!(url(&top), "a.example/x");
        assert_eq!(url(&bottom), "");
        let width = URL_CHARS;
        let long = format!("https://{}", "x".repeat(80));
        let [top, bottom] = sign_in_lines(&long, "AB-CD");
        assert_eq!(url(&top), "x".repeat(width));
        assert_eq!(url(&bottom), "x".repeat(width));
        assert_eq!(&cells(&bottom)[..2], ["AB-    ", "CD     "]);
        // A character past ASCII is cut at a character, never in it.
        let [top, _] = sign_in_lines(&format!("https://{}", "\u{e9}".repeat(40)), "AB-CD");
        assert_eq!(top.chars().count(), mcu::LINE_CHARS);
    }

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
        *shown.lock().expect("shown lock poisoned") = Some(code);
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
        assert_eq!(first.1, 0);
        assert_eq!(second.1, mcu::LINE_CHARS);
        assert_eq!(
            [first.2, second.2],
            sign_in_lines("https://idp.example/device", "WXYZ-1234")
        );
        assert_eq!(
            lcd.try_recv().unwrap(),
            mcu::colours(Device::Main, [mcu::WHITE; 8]),
            "the code is not left on the last session's colours"
        );
        // A resend while connected is no new connection.
        link.send_modify(|l| l.resync += 1);
        wait().await;
        assert!(lcd.try_recv().is_err());
        task.abort();
    }
}
