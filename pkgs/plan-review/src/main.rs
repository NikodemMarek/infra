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
    /// list the reviews, newest first: URL, open or sent, session, revision, title
    List,
    /// serve the review pages on 127.0.0.1:4388 (run by the plan-review user service)
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

fn run(command: Command) -> Result<(), String> {
    let store = Store::from_env()?;
    match command {
        Command::Publish { file, title } => {
            let session = std::env::var("AGENT_SESSION").unwrap_or_default();
            let file = std::path::absolute(file).map_err(|e| e.to_string())?;
            store.publish(&session, &file, &title)
        }
        Command::Intake => {
            let mut event = String::new();
            std::io::stdin()
                .read_to_string(&mut event)
                .map_err(|e| format!("reading stdin: {e}"))?;
            store.intake(&event)
        }
        Command::List => {
            for line in store.list()? {
                println!("{line}");
            }
            Ok(())
        }
        Command::Serve => server::serve(&store),
    }
}
