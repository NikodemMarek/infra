use std::io::{ErrorKind, Write};
use std::process;

use clap::{Parser, Subcommand};
use tm::Request;

mod serve;

/// Manage the markdown task list $TM_DIR/tasks.md.
#[derive(Parser)]
#[command(name = "tm", version)]
struct Cli {
    /// Output JSON (list and week)
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(flatten)]
    Task(Request),
    /// Serve the task list over HTTP
    Serve {
        /// Address to listen on
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,
    },
}

fn main() {
    let cli = Cli::parse();
    let request = match cli.command {
        Command::Task(request) => request,
        Command::Serve { listen } => {
            if let Err(e) = serve::serve(&listen) {
                eprintln!("tm: {e}");
                process::exit(1);
            }
            return;
        }
    };
    let output = tm::execute(request, cli.json);
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
