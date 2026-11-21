mod api;
mod cmd;
mod jj;
mod pr;
mod target;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::cmd::Result;

#[derive(Parser)]
#[command(about = "Read and manage merge/pull requests on GitLab and GitHub through glab / gh")]
struct Cli {
    /// jj git remote used to find the forge project
    #[arg(long, value_name = "NAME")]
    remote: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the resolved forge, host and project as JSON
    Repo,
    /// Merge/pull request commands
    Pr {
        #[command(subcommand)]
        command: PrCommand,
    },
}

#[derive(Subcommand)]
enum PrCommand {
    /// List merge/pull requests in all states
    List {
        /// Only requests from this source branch (newest created first)
        #[arg(long, value_name = "BRANCH")]
        head: Option<String>,
        #[arg(
            long,
            default_value_t = 100,
            value_parser = clap::value_parser!(u8).range(1..=100)
        )]
        limit: u8,
    },
    /// Show one merge/pull request (number or URL)
    View { pr: String },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("forge: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<String> {
    let remote = cli.remote.as_deref();
    match cli.command {
        Command::Repo => json(&target::resolve(remote)?),
        Command::Pr {
            command: PrCommand::List { head, limit },
        } => json(&pr::list(
            &target::resolve(remote)?,
            head.as_deref(),
            limit,
        )?),
        Command::Pr {
            command: PrCommand::View { pr },
        } => {
            let (target, number) = target::resolve_pr(remote, &pr)?;
            json(&pr::view(&target, number)?)
        }
    }
}

fn json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}
