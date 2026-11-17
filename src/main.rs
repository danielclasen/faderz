//! `faderz --config <file> [flags]`: serves the mixer state until SIGINT
//! or SIGTERM. `faderz --help` lists the flags.

use std::process::ExitCode;

use faderz::config::{self, Command, Config};
use faderz::server::Server;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, flags) = match config::command(&args) {
        Ok(Command::Run { config, flags }) => (config, flags),
        Ok(Command::Version) => {
            println!("faderz {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(Command::Help) => {
            print!("{}", config::usage());
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("faderz: {e}\n\n{}", config::usage());
            return ExitCode::from(2);
        }
    };
    let (config, shape, name) = match Config::load(&path, &flags) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("faderz: config: {e}");
            return ExitCode::from(2);
        }
    };
    for note in &config.notes {
        eprintln!("faderz: config: {note}");
    }
    // openraft reports every election and membership step at INFO.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .init();
    // SIGINT and SIGTERM are blocked before any thread exists, so every
    // thread inherits the mask, the RT thread too, and only sigwait below
    // receives them.
    // SAFETY: `set` is initialised by sigemptyset before use.
    let set = unsafe {
        let mut set = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    };
    let process = config
        .audio
        .as_ref()
        .map(|engine| samplez::rt::setup_process(&engine.rt));
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("faderz: runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let server = match runtime.block_on(Server::start(&config, shape, name, process)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("faderz: {e}");
            return ExitCode::FAILURE;
        }
    };
    // A flag or variable can override the file's mode; say which applies.
    eprintln!(
        "faderz: auth mode {} (from {})",
        format!("{:?}", config.auth.mode).to_ascii_lowercase(),
        config::auth_mode_source(&flags, |k| std::env::var(k).ok())
    );
    eprintln!("faderz: api listener on {}", server.api);
    if let Some(machine) = server.machine {
        eprintln!("faderz: machine listener on {machine}");
    }
    if let Some(peer) = server.peer {
        eprintln!("faderz: peer listener on {peer}");
    }
    eprintln!("faderz: ops listener on {}", server.ops);
    eprintln!("faderz: nmos listener on {}", server.nmos);
    match &server.audio {
        Some(audio) => eprintln!("faderz: rt {:?}", audio.engine().status().rt),
        None => eprintln!("faderz: no mxl section: no engine runs, nothing is mixed"),
    }
    let failure = server.failure();
    let mut sig = 0;
    // SAFETY: `set` is a valid signal set and `sig` is writable.
    unsafe { libc::sigwait(&set, &mut sig) };
    runtime.block_on(server.stop());
    match failure.lock().expect("failure lock poisoned").take() {
        Some(e) => {
            eprintln!("faderz: {e}");
            ExitCode::FAILURE
        }
        None => ExitCode::SUCCESS,
    }
}
