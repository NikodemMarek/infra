use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, OpenOptions, TryLockError};
use std::path::Path;
use std::process::{self, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::agent::runtime_dir;
use crate::cmd::{output_timeout, which};
use crate::jj::Jj;
use crate::msg::deliver;
use crate::procs::{claude_sessions, in_use, live_claudes, session_procs, Claude};
use crate::record::{now, write_error, Record, Records};
use crate::shpool::Shpool;
use crate::workspace::{
    agent_workspace, ask_lost, basename, confirm, is_dir, is_gitlab, recent, remove, remove_broken,
    repos, root,
};
use crate::PruneArgs;

/// Seconds a detached Claude session must have been idle to be reaped.
const IDLE_LIMIT: f64 = 4.0 * 3600.0;
/// Seconds a finished session must have been idle and detached to be stopped.
const FINISHED_IDLE: u64 = 600;
/// The time the reaper works through the session records, so the run ends within its timeout.
const REAP_DEADLINE: Duration = Duration::from_secs(600);
const WRAP_UP: &str = "The merge requests of this session are merged. Finish what is left of this work (e.g. record the outcome), then run `jw done`. The session is stopped once it is idle and nobody is attached.";

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

/// Removes the workspaces whose stack is merged into trunk and that hold nothing else (`jj stack status`), not
/// modified within the last hour. Those in use go too if the only users are idle Claude sessions in detached shpool
/// sessions, which are killed along with their background jobs. gitlab-context's (gl-*) are left to `gitlab-context gc`,
/// which knows when they are reused. Directories jj cannot read (failing to load) are deleted only after asking once
/// for all of them; with --yes they are kept, as they may hold unsnapshotted edits.
pub fn cmd_prune(a: PruneArgs) -> Result<(), String> {
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
            if is_gitlab(&name) || !is_dir(&format!("{d}/.jj")) || recent(&d) {
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
    if which("gitlab-context") {
        // it logs what it removes
        let _ = Command::new("gitlab-context")
            .args(["gc", "--all"])
            .stdout(Stdio::null())
            .status();
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

fn mark_record(name: &str, mark: impl FnOnce(&mut Record)) {
    let updated = Records::new().update(name, |r| {
        mark(r);
        true
    });
    if let Err(e) = updated {
        eprintln!("jw: {name}: {}", write_error(&e));
    }
}

/// Stops and removes the workspace of the session record r if its work is finished; see cmd_reap.
fn reap_record(name: &str, r: &Record, now: f64, seen: &mut HashSet<String>) {
    let done = r.status.as_deref() == Some("done");
    let Some((home, ws_name, path)) = agent_workspace(r.workspace.as_deref()) else {
        return;
    };
    if r.reaped.is_some() || !(done || !r.mrs.is_empty()) {
        return;
    }
    let gitlab = is_gitlab(&ws_name);
    let removed = format!("removed {}.agents/{ws_name}", basename(&home));
    if done {
        match stop_sessions(&path, FINISHED_IDLE) {
            Ok(killed) => report_stopped(&killed, &path, seen),
            // not idle and detached yet
            Err(_) => return,
        }
        if !gitlab {
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
        }
        mark_record(name, |r| r.reaped = Some(now));
        return;
    }
    if gitlab {
        return;
    }
    if r.notified.is_none() && !claude_sessions(&path).is_empty() {
        let Some(states) = r
            .mrs
            .iter()
            .map(|u| mr_state(u))
            .collect::<Option<Vec<_>>>()
        else {
            eprintln!("jw: {name}: couldn't check its MRs");
            return;
        };
        let newly = newly_merged(&r.mrs, &states, &r.merged_mrs);
        let finished = mrs_merged(&states) && {
            let stack = Jj::in_dir(&path).stack_merged();
            if stack.is_none() {
                eprintln!("jw: {name}: couldn't check the stack");
            }
            stack == Some(true)
        };
        let text = if finished {
            WRAP_UP.to_string()
        } else if newly.is_empty() {
            return;
        } else {
            merged_message(&newly)
        };
        if let Err(e) = deliver(name, &text, "jw-reap", Duration::ZERO) {
            eprintln!("jw: {name}: {e}");
        }
        mark_record(name, |r| {
            r.merged_mrs.extend(newly);
            if finished {
                r.notified = Some(now);
            }
        });
        println!("notified {name}");
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

/// The state of the MR or PR at url, lowercased (open, opened, merged, closed); None if it couldn't be read.
fn mr_state(url: &str) -> Option<String> {
    let mut cmd = if url.contains("://github.com/") {
        let mut c = Command::new("gh");
        c.args(["pr", "view", url, "--json", "state", "--jq", ".state"]);
        c
    } else {
        let (repo, iid) = url.split_once("/-/merge_requests/")?;
        let mut c = Command::new("glab");
        c.args([
            "mr", "view", iid, "-R", repo, "-F", "json", "--jq", ".state",
        ]);
        c
    };
    let out = output_timeout(&mut cmd, None, Duration::from_secs(60)).ok()??;
    let state = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
    (out.status.success() && !state.is_empty()).then_some(state)
}

/// Whether a session's MRs are finished: none is open and at least one is merged.
fn mrs_merged(states: &[String]) -> bool {
    states.iter().all(|s| s == "merged" || s == "closed") && states.iter().any(|s| s == "merged")
}

/// The merged MRs among mrs (with their states) that the session hasn't been told about.
fn newly_merged(mrs: &[String], states: &[String], told: &[String]) -> Vec<String> {
    mrs.iter()
        .zip(states)
        .filter(|(u, s)| *s == "merged" && !told.contains(u))
        .map(|(u, _)| u.clone())
        .collect()
}

fn merged_message(urls: &[String]) -> String {
    format!(
        "Merged: {}. Run `jj upload` so what is still open is rebased onto trunk.",
        urls.join(", ")
    )
}

fn stuck(c: &Claude) -> bool {
    // a session waiting on a permission prompt holds its MR's lock, so later events only queue
    c.status.as_deref() == Some("idle")
        || (c.status.as_deref() == Some("waiting")
            && c.waiting_for.as_deref() == Some("permission prompt"))
}

/// (shpool session, working directory, Claude session id) of the detached shpool sessions whose Claudes are all idle
/// (or stuck on a permission prompt) for IDLE_LIMIT, sorted by name.
fn idle_sessions(
    claudes: &[Claude],
    state: &HashMap<String, String>,
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
        if state.get(name).map(String::as_str) != Some("disconnected") {
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

/// Stops the sessions whose work is finished (`jw done`, or their MRs are merged) once they are idle for
/// FINISHED_IDLE and detached, and removes their merged workspaces; a session whose MRs got merged is first asked to
/// wrap up. Then stops the detached Claude shpool sessions idle for IDLE_LIMIT, whose workspaces stay.
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
    let missing: Vec<&str> = ["jj", "git", "glab", "gh"]
        .into_iter()
        .filter(|t| !which(t))
        .collect();
    if !missing.is_empty() {
        eprintln!("jw: missing on PATH: {}", missing.join(" "));
    }
    let mut seen = HashSet::new();
    let started = now();
    let records = Records::new().load_all();
    let deadline = Instant::now() + REAP_DEADLINE;
    for (i, (name, record)) in records.iter().enumerate() {
        if Instant::now() > deadline {
            eprintln!(
                "jw: reap: out of time, {} record(s) left for the next run",
                records.len() - i
            );
            break;
        }
        reap_record(name, record, started, &mut seen);
    }
    let mut failed = false;
    let idle = idle_sessions(&live_claudes(), &Shpool.list()?, now());
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
    if failed {
        return Err(String::new());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{mrs_merged, newly_merged};

    fn states(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mrs_merged_needs_none_open_and_one_merged() {
        let mut mixed = states(&["open"; 4]);
        mixed.extend(states(&["closed"; 5]));
        mixed.push("merged".into());
        assert!(!mrs_merged(&mixed));
        assert!(!mrs_merged(&states(&["opened", "merged"])));
        assert!(mrs_merged(&states(&["closed", "closed", "merged"])));
        assert!(!mrs_merged(&states(&["closed"])));
        assert!(!mrs_merged(&[]));
    }

    #[test]
    fn newly_merged_skips_told_and_unmerged() {
        let mrs = states(&["a", "b", "c", "d"]);
        let st = states(&["merged", "open", "merged", "closed"]);
        assert_eq!(newly_merged(&mrs, &st, &states(&["a"])), states(&["c"]));
        assert!(newly_merged(&mrs, &st, &states(&["a", "c"])).is_empty());
    }
}

#[cfg(test)]
#[path = "../tests/unit/reap.rs"]
mod idle_tests;
