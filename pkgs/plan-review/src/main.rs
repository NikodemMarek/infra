mod client;
mod notice;
mod render;
mod server;
mod store;

use std::io::Read;
use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

use store::Store;

/// plan-review: show an HTML plan to the user in the browser, collect their feedback and send it back to the session.
///
/// publish snapshots the file as the next revision of its review and asks the user to open it (event review.requested).
/// The session is $AGENT_SESSION.
#[derive(Parser)]
#[command(name = "plan-review")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// publish an .html file (at most 2 MiB) as the next revision of its review
    Publish {
        /// the plan, an .html file
        file: PathBuf,
        /// title shown to the user
        title: String,
    },
    /// turn a review.requested event on stdin into a desktop notification that opens the review (used by the event handler)
    #[command(hide = true)]
    Intake,
    /// turn a review.submitted event on stdin into the line for the session-message handler (used by the event handler)
    #[command(hide = true)]
    Notice,
    /// list the reviews, newest first: URL, open or sent, session, revision, title
    List,
    /// serve the review pages and the API on 127.0.0.1:4388 (run by the plan-review container)
    #[command(hide = true)]
    Serve,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli.command) {
        eprintln!("plan-review: {e}");
        process::exit(1);
    }
}

fn read_event() -> Result<String, String> {
    let mut event = String::new();
    std::io::stdin()
        .read_to_string(&mut event)
        .map_err(|e| format!("reading stdin: {e}"))?;
    Ok(event)
}

fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Publish { file, title } => client::publish(&file, &title),
        Command::Intake => client::intake(&read_event()?),
        Command::Notice => {
            if let Some(line) = notice::notice(&read_event()?)? {
                println!("{line}");
            }
            Ok(())
        }
        Command::List => client::list(),
        Command::Serve => server::serve(&Store::from_env()?),
    }
}
