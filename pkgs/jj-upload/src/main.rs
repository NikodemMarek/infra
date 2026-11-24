use std::collections::{BTreeSet, HashMap, HashSet};
use std::process;

use clap::{Parser, Subcommand};

mod cmd;
mod forge;
mod jj;
mod record;
mod sync;

use cmd::{Output, Result};
use forge::{stack_section, strip_stack, upload_media, Forge, Mr};
use jj::{choose_bookmark, default_prefix, repr, Commit, Jj, DEFAULT_REVSET, REMOTE_TITLE};
use record::{record_mr, record_session};
use sync::{pull_descriptions, stack_status, sync};

const CONFIG_HELP: &str = "\
Config (all optional, set with `jj config set --user <key> <value>`):
  upload.remote          git remote to push to             (default: origin)
  upload.bookmark-prefix prefix of managed bookmarks       (default: <email user>/)
  upload.target-branch   branch the bottom MR targets      (default: trunk() bookmark)
  upload.draft           open new MRs as drafts            (default: false)
  forge.host / forge.project / forge.kind  forge target overrides, read by forge";

const LONG_ABOUT: &str = "\
Push a stack of jj changes as stacked merge/pull requests.

Every change in the revset gets its own bookmark (= git branch) and its own MR. Each MR targets \
the branch of its parent change, or the trunk branch for the bottom of the stack. Running it \
again force-pushes rewritten changes and syncs the MR target branch. Once an MR exists, its \
title and description on the forge win: they are pulled into the commit description (local \
edits are overwritten) and jj upload only maintains the stack list in the description. Changes \
you abandoned or squashed away get their MR closed and their branch deleted. Changes at the \
bottom of the stack whose MR was merged into trunk are abandoned by the sync.

When an MR is created, local files linked in its description (`![alt](path)`, absolute or \
relative to the repo root) that are not in the repo (git-ignored) are uploaded to the GitLab project and the links rewritten to the \
uploaded URLs; GitHub has no API for that, so its links are left alone with a warning, as are \
files that are missing or fail to upload.

Before pushing, it syncs: fetch, rebase onto trunk, and pull commits that exist only on a \
change's remote branch into the change (or, if both sides changed, into a \"remote changes on \
<branch>\" change after it, and stop). `jj sync` alone also pulls the MR titles and \
descriptions.

`jj stack status` fetches and prints one line per change of the stack, bottom first: <change8> \
TAB state TAB MR url (or -) TAB title, where state is no-mr, open, closed, changed, elsewhere \
or merged. The last line is `merged` (exit 0; every change is merged into trunk, or there are \
none), `not merged` (exit 1) or `unknown` (exit 2; the error goes to stderr). It must run from \
inside the workspace: jj's global flags such as `-R` don't reach it, because it goes through \
`util exec`.

In a Claude Code session (CLAUDE_CODE_SESSION_ID), the branch of a created MR is recorded at \
${XDG_STATE_HOME:-~/.local/state}/jj-upload/<repo>/<branch>.json with the workspace and \
session id.";

/// Push a stack of jj changes as stacked merge/pull requests.
#[derive(Parser)]
#[command(
    bin_name = "jj upload",
    long_about = LONG_ABOUT,
    after_help = CONFIG_HELP,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Changes to upload (default: trunk()..(@::) ~ (empty() & description(exact:"")))
    #[arg(short, long = "revisions", value_name = "REVSET")]
    revisions: Vec<String>,
    /// Open new MRs as drafts
    #[arg(long)]
    draft: bool,
    /// Only print what would happen
    #[arg(short = 'n', long)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Fetch, rebase onto trunk and pull MR titles and descriptions into the changes
    Sync,
    /// `status` prints the state of each change; anything else is passed to `jj log -r stack()`
    #[command(disable_help_flag = true)]
    Stack {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

struct Entry {
    commit: Commit,
    branch: String,
    target: String,
    title: String,
    body: String,
}

fn main() {
    match run_cli(Cli::parse()) {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    }
}

fn run_cli(cli: Cli) -> Result<i32> {
    if let Some(Cmd::Stack { args }) = &cli.cmd {
        if args.first().map(String::as_str) != Some("status") {
            return Err(Jj::exec_log_stack(args));
        }
    }
    let remote = Jj::config("upload.remote")?.unwrap_or_else(|| "origin".to_string());
    let prefix = match Jj::config("upload.bookmark-prefix")? {
        Some(prefix) => prefix,
        None => default_prefix()?,
    };
    match cli.cmd {
        Some(Cmd::Stack { .. }) => Ok(stack_status(&remote, &prefix)),
        Some(Cmd::Sync) => {
            let failures = sync(&remote, &prefix, DEFAULT_REVSET)?;
            for f in &failures {
                eprintln!("{f}");
            }
            if !failures.is_empty() {
                return Ok(1);
            }
            pull_descriptions(
                &Jj::commits(DEFAULT_REVSET)?,
                &Forge::new(&remote),
                &prefix,
                &Jj::remote_branches(&remote)?,
                false,
            )?;
            Ok(0)
        }
        None => {
            let draft = cli.draft || Jj::config("upload.draft")?.as_deref() == Some("true");
            let revset = if cli.revisions.is_empty() {
                DEFAULT_REVSET.to_string()
            } else {
                cli.revisions
                    .iter()
                    .map(|r| format!("({r})"))
                    .collect::<Vec<_>>()
                    .join(" | ")
            };
            upload(&remote, &prefix, &revset, draft, cli.dry_run)?;
            Ok(0)
        }
    }
}

fn push(remote: &str, branches: &[&str]) -> Result<Output> {
    let p = Jj::push(remote, branches)?;
    eprint!("{}", p.stderr);
    Ok(p)
}

fn upload(remote: &str, prefix: &str, revset: &str, draft: bool, dry_run: bool) -> Result<()> {
    if dry_run {
        eprintln!("dry run: not syncing (run `jj sync` first)");
    } else {
        let failures = sync(remote, prefix, revset)?;
        if !failures.is_empty() {
            for f in &failures {
                eprintln!("{f}");
            }
            return Err("sync could not reconcile remote changes; nothing pushed".to_string());
        }
    }

    let mut commits = Jj::commits(revset)?;
    if commits.is_empty() {
        return Err(format!("revset {} is empty", repr(revset)));
    }
    for c in &commits {
        let short = c.short();
        if c.immutable {
            return Err(format!("{short} is immutable (already in trunk?)"));
        }
        if c.description.starts_with(REMOTE_TITLE) {
            return Err(format!(
                "{short} holds remote changes; squash or abandon it first"
            ));
        }
        if c.conflict {
            return Err(format!("{short} has conflicts; resolve them first"));
        }
        if c.description.is_empty() {
            return Err(format!(
                "{short} has no description; `jj describe {short}` first"
            ));
        }
        if c.parents.len() != 1 {
            return Err(format!(
                "{short} is a merge commit; stacked MRs need a linear history"
            ));
        }
    }

    let fg = Forge::new(remote);
    let term = fg.term()?;
    let pushed = Jj::remote_branches(remote)?;
    if pull_descriptions(&commits, &fg, prefix, &pushed, dry_run)? {
        commits = Jj::commits(revset)?;
    }

    let trunk = Jj::trunk_branch(remote)?;
    let selected: HashSet<&str> = commits.iter().map(|c| c.commit.as_str()).collect();
    let outside: Vec<&str> = commits
        .iter()
        .flat_map(|c| &c.parents)
        .map(String::as_str)
        .filter(|p| !selected.contains(p))
        .collect();
    let mut in_trunk: HashSet<String> = HashSet::new();
    let mut outside_bookmarks: HashMap<String, Vec<String>> = HashMap::new();
    if !outside.is_empty() {
        let ids = outside.join(" | ");
        in_trunk = Jj::commit_ids(&format!("({ids}) & ::trunk()"))?
            .into_iter()
            .collect();
        for c in Jj::commits(&format!("({ids}) ~ ::trunk()"))? {
            outside_bookmarks.insert(c.commit, c.bookmarks);
        }
    }

    let mut plan: Vec<Entry> = Vec::new();
    let mut branches: HashMap<String, String> = HashMap::new();
    let mut to_delete: Vec<String> = Vec::new();
    for c in &commits {
        let (branch, extra) = choose_bookmark(c, prefix);
        to_delete.extend(extra);
        let parent = c.parents[0].as_str();
        let target = if selected.contains(parent) {
            branches
                .get(parent)
                .cloned()
                .ok_or_else(|| format!("{} comes before its parent in the revset", c.short()))?
        } else if in_trunk.contains(parent) {
            trunk.clone()
        } else {
            let candidate = outside_bookmarks
                .get(parent)
                .and_then(|bs| bs.iter().find(|b| pushed.contains_key(*b)));
            match candidate {
                Some(b) => b.clone(),
                None => {
                    return Err(format!(
                        "parent of {0} is neither uploaded nor in trunk; include it, e.g. -r 'trunk()..{0}'",
                        c.short()
                    ))
                }
            }
        };
        branches.insert(c.commit.clone(), branch.clone());
        let lines: Vec<&str> = c.description.lines().collect();
        plan.push(Entry {
            commit: c.clone(),
            branch,
            target,
            title: lines.first().map_or("", |l| l.trim()).to_string(),
            body: lines
                .get(1..)
                .unwrap_or_default()
                .join("\n")
                .trim()
                .to_string(),
        });
    }

    eprintln!(
        "Uploading {} change(s) to {remote}, bottom MR targets {trunk}:",
        plan.len()
    );
    for e in &plan {
        eprintln!(
            "  {}  {}  ->  {}   {}",
            e.commit.short(),
            e.branch,
            e.target,
            e.title
        );
    }

    let mut stale: BTreeSet<String> = to_delete.iter().cloned().collect();
    stale.extend(Jj::deleted_managed_bookmarks(prefix, remote)?);
    if !stale.is_empty() {
        let names: Vec<&str> = stale.iter().map(String::as_str).collect();
        eprintln!(
            "Closing {term}s / deleting branches of dropped changes: {}",
            names.join(", ")
        );
    }
    if dry_run {
        return Ok(());
    }

    for e in &plan {
        if !e.commit.bookmarks.contains(&e.branch) {
            Jj::bookmark_set(&e.branch, &e.commit.commit)?;
        }
    }
    if !to_delete.is_empty() {
        Jj::bookmark_delete(&to_delete)?;
    }
    let branch_names: Vec<&str> = plan.iter().map(|e| e.branch.as_str()).collect();
    if !push(remote, &branch_names)?.ok {
        return Err("push failed".to_string());
    }

    let mut mrs: Vec<Mr> = Vec::new();
    for e in &plan {
        let mut found = fg.find(&e.branch)?;
        if let Some(m) = &found {
            if m.state == "merged" {
                eprintln!(
                    "  {} {} is already merged; abandon the change if trunk has it",
                    m.reference, e.branch
                );
                mrs.push(m.clone());
                continue;
            }
            if m.state == "closed" {
                found = fg.reopen(m)?;
            }
        }
        let mr = match found {
            None => {
                let body = upload_media(&e.body, &fg)?;
                let mr = fg.create(&e.branch, &e.target, &e.title, &body, draft)?;
                eprintln!("  created {}  {}", mr.reference, mr.url);
                record_session(&e.branch);
                mr
            }
            Some(m) => {
                let mr = fg.update(
                    &m,
                    (m.title != e.title).then_some(e.title.as_str()),
                    (m.base != e.target).then_some(e.target.as_str()),
                    None,
                )?;
                eprintln!("  updated {}  {}", mr.reference, mr.url);
                mr
            }
        };
        record_mr(&mr.url);
        mrs.push(mr);
    }

    let entries: Vec<(&str, &str)> = plan
        .iter()
        .zip(&mrs)
        .map(|(e, m)| (e.branch.as_str(), m.reference.as_str()))
        .collect();
    for (e, mr) in plan.iter().zip(&mrs) {
        if mr.state == "merged" {
            continue;
        }
        let section = if plan.len() > 1 {
            stack_section(&entries, &e.branch, term)
        } else {
            String::new()
        };
        let desc = format!("{}\n\n{section}", strip_stack(&mr.body));
        let desc = desc.trim();
        if mr.body.trim() != desc {
            fg.update(mr, None, None, Some(desc))?;
        }
    }

    if !stale.is_empty() {
        for b in &stale {
            if let Some(mr) = fg.find(b)? {
                if mr.state == "open" {
                    fg.close(&mr)?;
                    eprintln!("  closed {} ({b})", mr.reference);
                }
            }
        }
        let names: Vec<&str> = stale
            .iter()
            .map(String::as_str)
            .filter(|b| pushed.contains_key(*b))
            .collect();
        if !names.is_empty() && !push(remote, &names)?.ok {
            return Err("deleting stale branches failed".to_string());
        }
    }
    Ok(())
}
