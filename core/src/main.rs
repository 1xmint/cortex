use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// Cortex: describe what you want; it builds it and proves it works.
#[derive(Parser)]
#[command(name = "cortex", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check that this machine can run Cortex safely.
    Doctor,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        None => ExitCode::SUCCESS,
        Some(Cmd::Doctor) => doctor(),
    }
}

fn doctor() -> ExitCode {
    match cortex::sandbox::self_test() {
        Ok(()) => {
            println!("sandbox: ok (bubblewrap + gVisor; secret unreadable; no network)");
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("sandbox: {e}");
            ExitCode::FAILURE
        }
    }
}
