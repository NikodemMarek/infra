use std::io::{ErrorKind, Write};
use std::process;

use clap::error::ErrorKind as UsageError;
use clap::{CommandFactory, Parser, Subcommand};
use tm::{ListArgs, Request};

/// Manage the markdown task lists (backlog and ISO-week files) in $TM_DIR.
#[derive(Parser)]
#[command(name = "tm", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Add a task to backlog.md (text without the checkbox)
    Add {
        #[arg(required = true, allow_hyphen_values = true)]
        text: Vec<String>,
    },
    /// Mark tasks done
    Done {
        /// file:line or a substring of the description
        #[arg(required = true)]
        refs: Vec<String>,
    },
    /// List tasks
    List {
        /// Case-insensitive substring of the description
        query: Option<String>,
        /// Scope: backlog or a week (current, next, YYYY-wNN)
        #[arg(long = "in")]
        scope: Option<String>,
        /// Only open tasks
        #[arg(long, conflicts_with = "done")]
        open: bool,
        /// Only done tasks
        #[arg(long)]
        done: bool,
        /// Tag (also matches sub-tags); repeat to require several
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Exact @proj value
        #[arg(long)]
        proj: Option<String>,
        /// Only tasks with a valid @due on or before YYYY-MM-DD
        #[arg(long)]
        due_before: Option<String>,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
    /// Summarize a week
    Week {
        /// current, next or YYYY-wNN (default current)
        weekspec: Option<String>,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
    /// Move backlog tasks into a week
    Take {
        /// file:line or a substring of the description
        #[arg(required = true)]
        refs: Vec<String>,
        /// Target week: current, next or YYYY-wNN
        #[arg(long)]
        week: Option<String>,
    },
    /// Copy a finished week's open tasks back to backlog.md
    Close {
        /// current, next or YYYY-wNN (default: latest finished week)
        weekspec: Option<String>,
    },
    /// Check backlog.md and the week files for format problems
    Lint,
}

fn main() {
    let output = tm::execute(request(Cli::parse().command));
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

fn request(command: Command) -> Request {
    match command {
        Command::Add { text } => {
            let text = text.join(" ");
            if text.trim().is_empty() {
                Cli::command()
                    .error(UsageError::TooFewValues, "the text must not be empty")
                    .exit();
            }
            Request::Add { text }
        }
        Command::Done { refs } => Request::Done { refs },
        Command::List {
            query,
            scope,
            open,
            done,
            tags,
            proj,
            due_before,
            json,
        } => Request::List(ListArgs {
            query,
            scope,
            open,
            done,
            tags,
            proj,
            due_before,
            json,
        }),
        Command::Week { weekspec, json } => Request::Week {
            week: weekspec,
            json,
        },
        Command::Take { refs, week } => Request::Take { refs, week },
        Command::Close { weekspec } => Request::Close { week: weekspec },
        Command::Lint => Request::Lint,
    }
}
