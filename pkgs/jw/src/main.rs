mod agent;
mod cmd;
mod hook;
mod jj;
mod msg;
mod poll;
mod procs;
mod reap;
mod record;
mod report;
mod session;
mod shpool;
mod systemd;
mod worker;
mod workspace;
mod worktree;

use std::process;

use clap::{Args, Parser, Subcommand};

/// jw: quick jj workspaces for the repos under one root (~/projects unless the package sets another), at <repo>.agents/<name> (the layout the claude wrapper uses too).
///
/// new, cd, rm and clone print the directory to go to on stdout; the `jw` fish function cds there.
/// pick prints the chosen repo's name on stdout. Everything else goes to stderr.
///
/// agent always starts Claude in a new workspace (s-*, like `jw new`) in a shpool session of the same name.
/// resume continues one of the repo's earlier Claude sessions the same way, in the workspace it ran in; resume --session NAME
/// continues the last Claude session of the agent session NAME under that name.
#[derive(Parser)]
#[command(name = "jw", verbatim_doc_comment)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// create a workspace in a repo and go there
    #[command(visible_alias = "n")]
    New(NewArgs),
    /// go to a workspace (default: pick one, this repo's first)
    #[command(visible_alias = "c")]
    Cd(CdArgs),
    /// forget a workspace and delete its directory (its work stays)
    Rm(RmArgs),
    /// clone a repo into the projects root and go there
    Clone(CloneArgs),
    /// remove the workspaces whose stack is merged and broken ones in all repos, and the idle detached Claude shpool sessions in merged ones (killing a session kills its background jobs)
    Prune(PruneArgs),
    /// stop the finished sessions (jw done, or merged MRs) once idle and detached, and remove their merged workspaces (after jw done also the worktrees the session created, unless they hold work); stop the detached Claude shpool sessions idle or stuck on a permission prompt for 4 hours (their workspaces stay; resume with jw resume --session NAME)
    Reap,
    /// publish failed pipelines, new comments and reviews and merges of agent sessions' MRs as forge.* events
    Poll,
    /// turn a forge event on stdin into the JSON line `jw msg` delivers (used by the forge event handler)
    #[command(hide = true)]
    ForgeNotice,
    /// turn a claude turn-end event on stdin into the JSON line `jw msg` delivers to the session it reports to (used by the report event handler)
    #[command(hide = true)]
    ReportNotice,
    /// print the jj workspace and stack of the current directory as JSON
    Info,
    /// pick a repo (the current one first) and print its name
    Pick,
    /// run Claude in a new workspace of a repo (default: this one; outside a repo the one the prompt starts with, else pick one): in a shpool session, or with --reports-to as a headless worker that needs a prompt
    #[command(visible_alias = "a")]
    Agent(AgentArgs),
    /// resume a Claude session of a repo (any of its workspaces; default: this one) in a shpool session, in its workspace (recreated on trunk() if it is gone)
    #[command(visible_alias = "r")]
    Resume(ResumeArgs),
    /// start a new first mate, a session that supervises the others
    Mate,
    /// attach to a Claude shpool session (default: pick one, this repo's first); a unique prefix of the name is enough
    Attach(AttachArgs),
    /// send a message to a Claude session: delivered right away, between the tool calls of a busy session or as a new turn of an idle one; fails if an interactive session is not running (a headless worker queues it for its next turn or starts one)
    Msg(MsgArgs),
    /// read, list or update the records of agent sessions (kind, title, description, url, workspace, mrs, worktrees), a JSON file in $XDG_STATE_HOME/agent-sessions that agent-inbox and the notifications show
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// mark this agent session finished: it is stopped once it is idle and detached, and the worktrees it created are removed
    Done,
    /// Claude Code hooks (a hook's JSON object on stdin) that run isolated worktrees in jj workspaces
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// the runner of headless sessions (the jw-worker-NAME unit runs it)
    #[command(hide = true)]
    Worker {
        #[command(subcommand)]
        action: WorkerAction,
    },
}

#[derive(Subcommand)]
enum WorkerAction {
    /// run the queued messages of the headless session NAME as claude turns
    Turn {
        name: String,
        /// first compact the session if it is still idle at result SEQ (what `worker compact` starts)
        #[arg(long, value_name = "SEQ")]
        compact: Option<u64>,
    },
    /// start a compaction turn for the headless session NAME if it has been idle since result SEQ
    Compact { name: String, seq: u64 },
}

#[derive(Subcommand)]
enum HookAction {
    /// WorktreeCreate: create the workspace (git worktree outside a jj repo) and print its directory
    Create,
    /// SubagentStop: squash the stopping agent's work into its base change, or report a conflict or problem
    Stop,
    /// WorktreeRemove: integrate an agent workspace and delete it, keeping one that holds changes
    Remove,
    /// statusline: name a session's jj workspace after the session title
    Title,
}

#[derive(Args)]
struct NewArgs {
    /// repo under the projects root (default: pick one, the current one first)
    repo: Option<String>,
    /// revision, bookmark or bookmark@origin to start from (default: trunk()); when given in this repo, it is resolved in the current workspace
    #[arg(short, long)]
    revision: Option<String>,
}

#[derive(Args)]
struct CdArgs {
    name: Option<String>,
}

#[derive(Args)]
struct RmArgs {
    name: Option<String>,
}

#[derive(Args)]
struct CloneArgs {
    url: String,
    /// directory name under the projects root (default: from the url)
    name: Option<String>,
}

#[derive(Args)]
struct PruneArgs {
    /// don't ask; answer yes to every question
    #[arg(long)]
    yes: bool,
}

#[derive(Args)]
#[command(trailing_var_arg = true)]
struct AgentArgs {
    /// start the session in the background
    #[arg(short, long)]
    detach: bool,
    /// revision, bookmark or bookmark@origin to start the new workspace from (default: trunk()); when given in this repo, it is resolved in the current workspace
    #[arg(short, long)]
    revision: Option<String>,
    /// the session this one reports to: its turn ends reach NAME as messages (default: it reports to the user)
    #[arg(long, value_name = "NAME")]
    reports_to: Option<String>,
    /// run interactively in shpool even with --reports-to
    #[arg(long)]
    interactive: bool,
    /// [repo under the projects root] [prompt...]; after a leading -- everything is the prompt
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Args)]
struct ResumeArgs {
    /// start the session in the background
    #[arg(short, long)]
    detach: bool,
    /// resume the last Claude session of the agent session NAME under its name (not with a repo; refused while it runs)
    #[arg(long, value_name = "NAME")]
    session: Option<String>,
    /// repo under the projects root (default: this one, else pick one)
    repo: Option<String>,
}

#[derive(Args)]
struct AttachArgs {
    name: Option<String>,
}

#[derive(Args)]
#[command(trailing_var_arg = true)]
struct MsgArgs {
    /// who the message is from (default: this session, else the user)
    #[arg(long = "from")]
    sender: Option<String>,
    /// the session's name, its workspace name or its shpool session name
    name: String,
    /// words of the message (default: read stdin); a leading -- is dropped
    #[arg(allow_hyphen_values = true)]
    text: Vec<String>,
}

#[derive(Subcommand)]
enum SessionAction {
    /// print the record as JSON; fails if there is none
    Get(SessionGetArgs),
    /// set the given fields of the record, creating it; --mr adds to the list; refused in GitLab event sessions
    Set(SessionSetArgs),
    /// set the status of this session's record (hooks run this); not for done, see jw done; allowed in GitLab event sessions
    Status(SessionStatusArgs),
    /// record a workspace this session created, so `jw reap` drops it after jw done if it holds no work (hooks run this); allowed in GitLab event sessions
    Worktree(SessionWorktreeArgs),
    /// print the records as JSON lines sorted by name, each with its name, whether it is running (its shpool session; a headless worker's unit) and the numbers of its unacked results
    List(SessionListArgs),
    /// print the results of a headless worker's finished turns that are not acked, as JSON lines by number
    Results(SessionResultsArgs),
    /// delete the results of session NAME up to SEQ
    Ack(SessionAckArgs),
}

#[derive(Args)]
struct SessionResultsArgs {
    /// session name (default: $AGENT_SESSION)
    #[arg(short, long)]
    session: Option<String>,
}

#[derive(Args)]
struct SessionAckArgs {
    name: String,
    seq: u64,
}

#[derive(Args)]
struct SessionListArgs {
    /// only the sessions that report to the session NAME
    #[arg(long, value_name = "NAME")]
    reports_to: Option<String>,
}

#[derive(Args)]
struct SessionGetArgs {
    /// session name (default: $AGENT_SESSION)
    #[arg(short, long)]
    session: Option<String>,
}

#[derive(Args)]
struct SessionSetArgs {
    /// session name (default: $AGENT_SESSION)
    #[arg(short, long)]
    session: Option<String>,
    /// what started the session, e.g. task, review, MergeRequest
    #[arg(long)]
    kind: Option<String>,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    description: Option<String>,
    /// the session's workspace directory
    #[arg(long)]
    workspace: Option<String>,
    /// https link to the work item
    #[arg(long)]
    url: Option<String>,
    /// https url of a merge request to add (repeatable)
    #[arg(long)]
    mr: Vec<String>,
}

#[derive(Args)]
struct SessionStatusArgs {
    #[arg(value_parser = ["busy", "idle", "waiting"])]
    state: String,
    /// what the session waits for (with waiting)
    #[arg(long)]
    waiting_for: Option<String>,
    /// when the prompt cache goes cold, epoch seconds; omitted: removed
    #[arg(long)]
    cache_expires_at: Option<i64>,
}

#[derive(Args)]
struct SessionWorktreeArgs {
    /// absolute path of the workspace directory
    dir: String,
}

fn main() {
    let cli = Cli::parse();
    let result = workspace::init_root().and_then(|()| match cli.command {
        Command::New(a) => workspace::cmd_new(a),
        Command::Cd(a) => workspace::cmd_cd(a),
        Command::Rm(a) => workspace::cmd_rm(a),
        Command::Clone(a) => workspace::cmd_clone(a),
        Command::Prune(a) => reap::cmd_prune(a),
        Command::Reap => reap::cmd_reap(),
        Command::Poll => poll::cmd_poll(),
        Command::ForgeNotice => poll::cmd_forge_notice(),
        Command::ReportNotice => report::cmd_report_notice(),
        Command::Info => workspace::cmd_info(),
        Command::Pick => workspace::cmd_pick(),
        Command::Agent(a) => agent::cmd_agent(a),
        Command::Resume(a) => agent::cmd_resume(a),
        Command::Mate => agent::cmd_mate(),
        Command::Attach(a) => agent::cmd_attach(a),
        Command::Msg(a) => msg::cmd_msg(a),
        Command::Session { action } => match action {
            SessionAction::Get(a) => session::cmd_session_get(a),
            SessionAction::Set(a) => session::cmd_session_set(a),
            SessionAction::Status(a) => session::cmd_session_status(a),
            SessionAction::Worktree(a) => session::cmd_session_worktree(a),
            SessionAction::List(a) => session::cmd_session_list(a),
            SessionAction::Results(a) => session::cmd_session_results(a),
            SessionAction::Ack(a) => session::cmd_session_ack(a),
        },
        Command::Done => session::cmd_done(),
        Command::Hook { action } => hook::cmd_hook(action),
        Command::Worker { action } => match action {
            WorkerAction::Turn { name, compact } => worker::cmd_worker_turn(&name, compact),
            WorkerAction::Compact { name, seq } => worker::cmd_worker_compact(&name, seq),
        },
    });
    if let Err(e) = result {
        if !e.is_empty() {
            eprintln!("jw: {e}");
        }
        process::exit(1);
    }
}
