use clap::Parser;

/// Cortex: describe what you want; it builds it and proves it works.
#[derive(Parser)]
#[command(name = "cortex", version)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
