use std::io::{ErrorKind, Write};
use std::process;

use clap::Parser;
use tm::Request;

/// Manage the markdown task lists (backlog and ISO-week files) in $TM_DIR.
#[derive(Parser)]
#[command(name = "tm", version)]
struct Cli {
    /// Output JSON (list and week)
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Request,
}

fn main() {
    let cli = Cli::parse();
    let output = tm::execute(cli.command, cli.json);
    // A closed stdout (`tm list | head -1`) is not an error.
    if let Err(e) = std::io::stdout().write_all(output.stdout.as_bytes()) {
        if e.kind() == ErrorKind::BrokenPipe {
            process::exit(0);
        }
        eprintln!("tm: {e}");
        process::exit(1);
    }
    for line in &output.stderr {
        eprintln!("tm: {line}");
    }
    if output.failed {
        process::exit(1);
    }
}
