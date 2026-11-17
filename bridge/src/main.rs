//! faderz-bridge: a faderz client that drives Mackie Control surfaces.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "--version" => {
            println!("faderz-bridge {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: faderz-bridge --version");
            ExitCode::from(2)
        }
    }
}
