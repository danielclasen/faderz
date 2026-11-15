//! `faderz --config <file>`: serves the mixer state until SIGINT or
//! SIGTERM.

use std::path::PathBuf;
use std::process::ExitCode;

use faderz::config::Config;
use faderz::server::Server;
use tokio::signal::unix::{SignalKind, signal};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let path = match args.as_slice() {
        [_, flag, path] if flag == "--config" => PathBuf::from(path),
        [_, flag] if flag == "--version" => {
            println!("faderz {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("usage: faderz --config <file>");
            return ExitCode::from(2);
        }
    };
    let (config, shape, name) = match Config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("faderz: config: {e}");
            return ExitCode::from(2);
        }
    };
    // openraft reports every election and membership step at INFO.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .init();
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
    match runtime.block_on(run(config, shape, name)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("faderz: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: Config, shape: faderz::shape::Shape, name: String) -> Result<(), String> {
    let mut term = signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
    let mut int = signal(SignalKind::interrupt()).map_err(|e| e.to_string())?;
    let mut server = Server::start(&config, shape, name).await?;
    eprintln!("faderz: api listener on {}", server.api);
    if let Some(machine) = server.machine {
        eprintln!("faderz: machine listener on {machine}");
    }
    eprintln!("faderz: ops listener on {}", server.ops);
    eprintln!("faderz: nmos listener on {}", server.nmos);
    let result = tokio::select! {
        e = server.failed() => Err(e),
        _ = term.recv() => Ok(()),
        _ = int.recv() => Ok(()),
    };
    server.stop().await;
    result
}
