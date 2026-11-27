use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::{Duration, Instant};

use crate::agent::runtime_dir;
use crate::cmd::which;
use crate::jj::Jj;
use crate::procs::{claude_sessions, in_use, live_claudes, session_procs, Claude};
use crate::record::{now, write_error, Record, Records};
use crate::shpool::Shpool;
use crate::workspace::{
    agent_workspace, ask_lost, basename, confirm, is_dir, realpath, recent, remove, remove_broken,
    repos, root,
};
use crate::worktree;
use crate::PruneArgs;

/// Seconds a detached Claude session must have been idle to be reaped.
const IDLE_LIMIT: f64 = 4.0 * 3600.0;
/// Seconds a finished session must have been idle and detached to be stopped.
const FINISHED_IDLE: u64 = 600;
/// The time the reaper works through the session records, so the run ends within its timeout.
const REAP_DEADLINE: Duration = Duration::from_secs(600);

/// (shpool session, Claude session id) of a stopped session.
type Stopped = (String, String);

/// A workspace that was kept, and why; the sessions stopped on the way are gone anyway. A loud reason is one worth
/// logging when reaping.
struct Kept {
    error: String,
    loud: bool,
    killed: Vec<Stopped>,
}

impl From<String> for Kept {
    fn from(error: String) -> Kept {
        Kept {
            error,
            loud: false,
            killed: vec![],
        }
    }
}

fn kept(error: &str) -> Kept {
    Kept::from(error.to_string())
}

/// The shpool sessions of the Claude sessions in d if all are agent sessions idle for idle seconds in detached shpool
/// sessions and nothing outside their sessions' processes uses d. The error says why not.
fn check_claudes(d: &str, sessions: &[Claude], idle: u64) -> Result<Vec<String>, String> {
    let mine = session_procs(sessions, Path::new("/proc"));
    if mine.contains(&process::id()) {
        return Err("this session runs jw".to_string());
    }
    if in_use(d, &mine) {
        return Err("in use by other processes".to_string());
    }
    for c in sessions {
        let status = c.status.as_deref();
        if status != Some("idle") {
            return Err(format!("Claude {} is {}", c.pid, status.unwrap_or("None")));
        }
        if c.agent.is_none() {
            return Err(format!("Claude {} is not an agent session", c.pid));
        }
        if !c
            .status_updated_at
            .is_some_and(|at| now() - at / 1000.0 >= idle as f64)
        {
            return Err(format!("Claude {} idle for less than {idle}s", c.pid));
        }
    }
    let names: BTreeSet<&str> = sessions.iter().filter_map(|c| c.agent.as_deref()).collect();
    let state = Shpool.list()?;
    for name in &names {
        let current = state.get(*name).map(String::as_str);
        if current != Some("disconnected") {
            return Err(format!(
                "shpool session {name} is {}",
                current.unwrap_or("unknown")
            ));
        }
    }
    Ok(names.into_iter().map(str::to_string).collect())
}

/// Kills the shpool sessions and waits until nothing is using d.
fn kill_shpool(d: &str, names: &[String]) -> Result<(), String> {
    for name in names {
        if !Shpool.kill(name)? {
            return Err(format!("shpool kill {name} failed"));
        }
    }
    for _ in 0..50 {
        if !in_use(d, &HashSet::new()) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "still in use after killing shpool session {}",
        names.join(", ")
    ))
}

/// Stops d's Claude shpool sessions if all are idle for at least idle seconds and detached.
fn stop_sessions(d: &str, idle: u64) -> Result<Vec<Stopped>, String> {
    let sessions = claude_sessions(d);
    if sessions.is_empty() {
        if in_use(d, &HashSet::new()) {
            return Err("in use by other processes".to_string());
        }
        return Ok(vec![]);
    }
    kill_shpool(d, &check_claudes(d, &sessions, idle)?)?;
    Ok(sessions
        .into_iter()
        .map(|c| {
            (
                c.agent.unwrap_or_default(),
                c.session_id.unwrap_or_else(|| "?".to_string()),
            )
        })
        .collect())
}

/// Removes the workspace name of the repo at home if its stack is merged and nothing would be lost, stopping its idle
/// sessions; returns the stopped sessions.
fn retire(home: &str, name: &str, yes: bool, idle: u64) -> Result<Vec<Stopped>, Kept> {
    let d = format!("{home}.agents/{name}");
    // cheap: before the stack check, which can fetch
    if in_use(&d, &HashSet::new()) {
        let sessions = claude_sessions(&d);
        if sessions.is_empty() {
            return Err(kept("in use by other processes"));
        }
        check_claudes(&d, &sessions, idle)?;
    }
    let jj = Jj::in_dir(&d);
    // before the slow stack check, so a rewrite during it is caught later
    let before = jj.commit_id()?;
    match jj.stack_merged() {
        Some(true) => {}
        Some(false) => return Err(kept("unmerged work")),
        None => {
            return Err(Kept {
                loud: true,
                ..kept("couldn't check the stack")
            })
        }
    }
    // before the kill: listing needs no snapshot
    let asked = if yes {
        vec![]
    } else {
        ask_lost(&d, false, false, &[])?
    };
    let killed = stop_sessions(&d, idle)?;
    match remove_merged(home, name, &d, &jj, &before, yes, &asked) {
        Ok(()) => Ok(killed),
        Err(mut e) => {
            e.killed = killed;
            Err(e)
        }
    }
}

fn remove_merged(
    home: &str,
    name: &str,
    d: &str,
    jj: &Jj,
    before: &str,
    yes: bool,
    asked: &[String],
) -> Result<(), Kept> {
    // snapshots, and may wait for the user
    ask_lost(d, yes, true, asked)?;
    if jj.commit_id()? != before {
        return Err(kept("changed while removing"));
    }
    // visible work that no workspace's or pushed history includes; only logged, the stack is merged
    let heads = jj.heads_outside().unwrap_or_default();
    if !heads.is_empty() {
        let more = if heads.len() > 10 {
            format!(", ... (+{} more)", heads.len() - 10)
        } else {
            String::new()
        };
        let shown = &heads[..heads.len().min(10)];
        eprintln!(
            "jw: {name}: heads outside the workspaces: {}{more}",
            shown.join("; ")
        );
    }
    remove(home, name).map_err(|error| Kept {
        loud: !error.is_empty(),
        error,
        killed: vec![],
    })
}

fn killed_note(killed: &[Stopped]) -> String {
    if killed.is_empty() {
        return String::new();
    }
    let names: BTreeSet<&str> = killed.iter().map(|(name, _)| name.as_str()).collect();
    format!(
        " (killed shpool session {})",
        names.into_iter().collect::<Vec<_>>().join(", ")
    )
}

/// The sessions reap and prune leave alone: first mates and headless workers that are not done or reaped. A mate's repo
/// has no work to merge, so its stack always reads as merged.
fn kept_sessions(records: &[(String, Record)]) -> HashSet<String> {
    records
        .iter()
        .filter(|(_, r)| {
            r.long_lived() && r.reaped.is_none() && r.status.as_deref() != Some("done")
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Removes the workspaces whose stack is merged into trunk and that hold nothing else (`jj stack status`), not
/// modified within the last hour. Those in use go too if the only users are idle Claude sessions in detached shpool
/// sessions, which are killed along with their background jobs. Directories jj cannot read (failing to load) are deleted
/// only after asking once for all of them; with --yes they are kept, as they may hold unsnapshotted edits.
pub fn cmd_prune(a: PruneArgs) -> Result<(), String> {
    let kept = kept_sessions(&Records::open()?.load_all()?);
    let mut broken: Vec<(String, String, String, Vec<String>)> = vec![];
    for repo in repos() {
        let home = format!("{}/{repo}", root());
        let Ok(entries) = fs::read_dir(format!("{home}.agents")) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        // jj already said why
        let Ok(known) = Jj::in_dir(&home).workspace_names() else {
            continue;
        };
        for name in names {
            let d = format!("{home}.agents/{name}");
            if kept.contains(&name) || !is_dir(&format!("{d}/.jj")) || recent(&d) {
                continue;
            }
            if !in_use(&d, &HashSet::new()) && !Jj::in_dir(&d).loads() {
                broken.push((repo.clone(), home.clone(), name, known.clone()));
                continue;
            }
            match retire(&home, &name, a.yes, 3600) {
                Ok(killed) => {
                    eprintln!("jw: removed {repo}.agents/{name}{}", killed_note(&killed));
                }
                Err(e) => {
                    let why = if e.error.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", e.error)
                    };
                    eprintln!(
                        "jw: kept {repo}.agents/{name}{why}{}",
                        killed_note(&e.killed)
                    );
                }
            }
        }
    }
    if broken.is_empty() {
        return Ok(());
    }
    if a.yes {
        // they may hold unsnapshotted edits: only a person deletes them
        for (repo, _, name, _) in &broken {
            eprintln!("jw: kept {repo}.agents/{name}: jj can't load it; run jw prune interactively to delete");
        }
        return Ok(());
    }
    eprintln!("jw: broken workspaces (jj cannot read them, so it cannot tell what is in them):");
    for (repo, _, name, _) in &broken {
        eprintln!("  {repo}.agents/{name}");
    }
    if let Err(e) = confirm(&format!("Delete these {}? [y/N] ", broken.len()), false) {
        eprintln!("jw: kept them: {e}");
        return Ok(());
    }
    for (repo, home, name, known) in &broken {
        remove_broken(home, name, known)?;
        eprintln!("jw: removed {repo}.agents/{name}");
    }
    Ok(())
}

fn report_stopped(killed: &[Stopped], path: &str, seen: &mut HashSet<String>) {
    for (name, _) in killed {
        seen.insert(name.clone());
        println!("stopped {name} in {path}: jw resume --session {name}");
    }
}

fn mark_record(name: &str, mut mark: impl FnMut(&mut Record)) {
    let updated = Records::open().and_then(|records| {
        records.update(name, |r| {
            mark(r);
            true
        })
    });
    if let Err(e) = updated {
        eprintln!("jw: {name}: {}", write_error(&e));
    }
}

/// Drops the worktrees a done session created, except workspaces of session records (workspaces: realpath to session
/// name) and those in use; gone ones are skipped. Stops dropping at the deadline.
fn drop_worktrees(worktrees: &[String], workspaces: &HashMap<PathBuf, String>, deadline: Instant) {
    for d in worktrees {
        if !Path::new(d).is_absolute() || !Path::new(d).exists() {
            continue;
        }
        if let Some(owner) = workspaces.get(&realpath(d)) {
            println!("kept {d}: workspace of session {owner}");
        } else if in_use(d, &HashSet::new()) {
            println!("kept {d}: in use by other processes");
        } else {
            if Instant::now() >= deadline {
                println!("kept {d}: out of time");
                continue;
            }
            let why = worktree::drop(d);
            if Path::new(d).exists() {
                println!("kept {d}: {}", why.as_deref().unwrap_or("holds work"));
            } else {
                println!("removed {d}");
            }
        }
    }
}

/// Stops and removes the workspace of the session record r if its work is finished; see cmd_reap.
fn reap_record(
    name: &str,
    r: &Record,
    now: f64,
    workspaces: &HashMap<PathBuf, String>,
    deadline: Instant,
    seen: &mut HashSet<String>,
) {
    let done = r.status.as_deref() == Some("done");
    let Some((home, ws_name, path)) = agent_workspace(r.workspace.as_deref()) else {
        return;
    };
    if r.reaped.is_some() || !(done || !r.mrs.is_empty()) {
        return;
    }
    let removed = format!("removed {}.agents/{ws_name}", basename(&home));
    if done {
        match stop_sessions(&path, FINISHED_IDLE) {
            Ok(killed) => report_stopped(&killed, &path, seen),
            // not idle and detached yet
            Err(_) => return,
        }
        drop_worktrees(&r.worktrees, workspaces, deadline);
        match retire(&home, &ws_name, true, FINISHED_IDLE) {
            Ok(killed) => {
                report_stopped(&killed, &path, seen);
                println!("{removed}");
            }
            Err(e) => {
                report_stopped(&e.killed, &path, seen);
                if e.error.is_empty() {
                    println!("kept {path}");
                } else {
                    println!("kept {path}: {}", e.error);
                }
            }
        }
        mark_record(name, |r| r.reaped = Some(now));
        return;
    }
    if r.notified.is_none() && !claude_sessions(&path).is_empty() {
        return;
    }
    match retire(&home, &ws_name, true, FINISHED_IDLE) {
        Ok(killed) => {
            report_stopped(&killed, &path, seen);
            println!("{removed}");
            mark_record(name, |r| r.reaped = Some(now));
        }
        Err(e) => {
            report_stopped(&e.killed, &path, seen);
            // other reasons: still waiting or not ready
            if e.loud {
                eprintln!("jw: {name}: {}", e.error);
            }
        }
    }
}

fn stuck(c: &Claude) -> bool {
    // a session waiting on a permission prompt holds its MR's lock, so later events only queue
    c.status.as_deref() == Some("idle")
        || (c.status.as_deref() == Some("waiting")
            && c.waiting_for.as_deref() == Some("permission prompt"))
}

/// (shpool session, working directory, Claude session id) of the detached shpool sessions whose Claudes are all idle
/// (or stuck on a permission prompt) for IDLE_LIMIT, sorted by name; the sessions in kept are never listed.
fn idle_sessions(
    claudes: &[Claude],
    state: &HashMap<String, String>,
    kept: &HashSet<String>,
    now: f64,
) -> Vec<(String, String, String)> {
    let mut groups: BTreeMap<&str, Vec<&Claude>> = BTreeMap::new();
    for c in claudes {
        if let Some(name) = c.agent.as_deref() {
            groups.entry(name).or_default().push(c);
        }
    }
    let mut found = vec![];
    for (name, members) in groups {
        if kept.contains(name) || state.get(name).map(String::as_str) != Some("disconnected") {
            continue;
        }
        let all_idle = members.iter().all(|c| {
            stuck(c)
                && c.status_updated_at
                    .is_some_and(|at| now - at / 1000.0 >= IDLE_LIMIT)
        });
        if all_idle {
            let first = members[0];
            found.push((
                name.to_string(),
                first.cwd.clone(),
                first.session_id.clone().unwrap_or_else(|| "?".to_string()),
            ));
        }
    }
    found
}

fn prune_records(store: &Records) {
    if let Err(e) = store.prune(now(), |name| println!("pruned {name}")) {
        eprintln!("jw: prune: {e}");
    }
}

/// Stops the sessions whose work is finished (`jw done`, or their MRs are merged) once they are idle for
/// FINISHED_IDLE and detached, and removes their merged workspaces; a session that ran `jw done` also loses the worktrees
/// it created (`jw hook create`) that hold no work. A session whose MRs got merged is
/// first asked to wrap up by `jw poll`. Then stops the detached Claude shpool sessions idle for IDLE_LIMIT, whose
/// workspaces stay. Last deletes the session records untouched for 7 days (`pruned NAME`).
pub fn cmd_reap() -> Result<(), String> {
    let lock_path = format!("{}/jw-reap.lock", runtime_dir());
    let lock = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&lock_path)
        .map_err(|e| format!("{lock_path}: {e}"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            eprintln!("jw: reap already running");
            return Ok(());
        }
        Err(TryLockError::Error(e)) => return Err(format!("{lock_path}: {e}")),
    }
    let missing: Vec<&str> = ["jj", "git"].into_iter().filter(|t| !which(t)).collect();
    if !missing.is_empty() {
        eprintln!("jw: missing on PATH: {}", missing.join(" "));
    }
    let mut seen = HashSet::new();
    let started = now();
    let store = Records::open()?;
    let records = store.load_all()?;
    let workspaces: HashMap<PathBuf, String> = records
        .iter()
        .filter_map(|(name, r)| {
            let ws = r
                .workspace
                .as_deref()
                .filter(|w| Path::new(w).is_absolute())?;
            Some((realpath(ws), name.clone()))
        })
        .collect();
    let kept = kept_sessions(&records);
    let deadline = Instant::now() + REAP_DEADLINE;
    for (i, (name, record)) in records.iter().enumerate() {
        if Instant::now() > deadline {
            eprintln!(
                "jw: reap: out of time, {} record(s) left for the next run",
                records.len() - i
            );
            break;
        }
        if !kept.contains(name) {
            reap_record(name, record, started, &workspaces, deadline, &mut seen);
        }
    }
    let mut failed = false;
    let idle = idle_sessions(&live_claudes(), &Shpool.list()?, &kept, now());
    for (name, cwd, id) in idle {
        if seen.contains(&name) {
            continue;
        }
        if Shpool.kill(&name).unwrap_or(false) {
            report_stopped(&[(name, id)], &cwd, &mut seen);
        } else {
            eprintln!("jw: shpool kill {name} failed");
            failed = true;
        }
    }
    prune_records(&store);
    if failed {
        return Err(String::new());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/reap.rs"]
mod idle_tests;
