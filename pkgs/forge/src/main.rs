mod api;
mod cmd;
mod jj;
mod pr;
mod review;
mod target;
mod upload;

use std::path::PathBuf;
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
    /// Upload a file and print its URL (GitLab only)
    Upload { file: PathBuf },
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
    /// Open a merge/pull request
    Create {
        #[arg(long, value_name = "BRANCH", allow_hyphen_values = true)]
        head: String,
        #[arg(long, value_name = "BRANCH", allow_hyphen_values = true)]
        base: String,
        #[arg(long, allow_hyphen_values = true)]
        title: String,
        #[arg(long, allow_hyphen_values = true)]
        body: Option<String>,
        #[arg(long)]
        draft: bool,
    },
    /// Change the title, base branch or body of a merge/pull request (number or URL)
    Edit {
        pr: String,
        #[arg(long, allow_hyphen_values = true)]
        title: Option<String>,
        #[arg(long, value_name = "BRANCH", allow_hyphen_values = true)]
        base: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        body: Option<String>,
    },
    /// Close a merge/pull request (number or URL)
    Close { pr: String },
    /// Reopen a merge/pull request (number or URL)
    Reopen { pr: String },
    /// Show the latest pipeline / check state of a merge/pull request (number or URL)
    Checks { pr: String },
    /// List the comments and reviews of a merge/pull request (number or URL)
    Comments { pr: String },
    /// Print the last lines of a failed job's log as plain text (number or URL; job id from `checks`)
    JobLog {
        pr: String,
        job_id: u64,
        #[arg(long, default_value_t = 40, value_parser = clap::value_parser!(u32).range(1..))]
        tail: u32,
    },
    /// Create draft review comments from JSON on stdin (number or URL); the user publishes them
    ///
    /// Creates drafts only (GitLab draft notes, a GitHub pending review) and never publishes:
    /// the user publishes or deletes them in the web UI. GitLab refuses when you already have
    /// draft notes on the request.
    ///
    /// Input on stdin (unknown fields are rejected; at least one of summary, comments, replies):
    ///   {
    ///     "summary": "overall remark",
    ///     "comments": [{"path": "src/a.rs", "line": 12, "side": "new", "body": "text"}],
    ///     "replies": [{"thread": "ID", "body": "text", "resolve": false}]
    ///   }
    ///
    /// comments: inline comments on a line of the diff. `line` (>= 1) is the file line number on
    /// `side` ("new", the default, or "old" for a removed or unchanged line); the line must be
    /// shown in the diff. A ```suggestion block belongs only in inline comments on the new side.
    ///
    /// replies: `thread` is the `thread` value of `forge pr comments`. `resolve` resolves the
    /// thread (GitLab only; rejected on GitHub).
    ///
    /// --commit SHA reviews that version of the request (GitLab: the version whose head commit
    /// is SHA; GitHub: the commit the comments are placed on) instead of the latest.
    ///
    /// Prints {"url": ..., "drafts": N}. When a write fails, the error says how many drafts were
    /// already created.
    #[command(verbatim_doc_comment)]
    Review {
        pr: String,
        #[arg(long, value_name = "SHA")]
        commit: Option<String>,
    },
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
        Command::Pr {
            command:
                PrCommand::Create {
                    head,
                    base,
                    title,
                    body,
                    draft,
                },
        } => json(&pr::create(
            &target::resolve(remote)?,
            &head,
            &base,
            &title,
            body.as_deref(),
            draft,
        )?),
        Command::Pr {
            command:
                PrCommand::Edit {
                    pr,
                    title,
                    base,
                    body,
                },
        } => {
            let (target, number) = target::resolve_pr(remote, &pr)?;
            json(&pr::edit(
                &target,
                number,
                title.as_deref(),
                base.as_deref(),
                body.as_deref(),
            )?)
        }
        Command::Pr {
            command: PrCommand::Close { pr },
        } => {
            let (target, number) = target::resolve_pr(remote, &pr)?;
            json(&pr::close(&target, number)?)
        }
        Command::Pr {
            command: PrCommand::Reopen { pr },
        } => {
            let (target, number) = target::resolve_pr(remote, &pr)?;
            json(&pr::reopen(&target, number)?)
        }
        Command::Pr {
            command: PrCommand::Checks { pr },
        } => {
            let (target, number) = target::resolve_pr(remote, &pr)?;
            json(&pr::checks(&target, number)?)
        }
        Command::Pr {
            command: PrCommand::Comments { pr },
        } => {
            let (target, number) = target::resolve_pr(remote, &pr)?;
            json(&pr::comments(&target, number)?)
        }
        Command::Pr {
            command: PrCommand::JobLog { pr, job_id, tail },
        } => {
            let (target, _) = target::resolve_pr(remote, &pr)?;
            pr::job_log(&target, job_id, tail as usize)
        }
        Command::Pr {
            command: PrCommand::Review { pr, commit },
        } => {
            let (target, number) = target::resolve_pr(remote, &pr)?;
            let input = std::io::read_to_string(std::io::stdin())
                .map_err(|e| format!("cannot read stdin: {e}"))?;
            json(&review::submit(&target, number, commit.as_deref(), &input)?)
        }
        Command::Upload { file } => json(&upload::upload(&target::resolve(remote)?, &file)?),
    }
}

fn json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}
