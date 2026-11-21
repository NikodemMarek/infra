use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::record::valid_name;
use crate::workspace::realpath;

/// A live Claude session, from its registry file in $CLAUDE_CONFIG_DIR/sessions and /proc.
pub struct Claude {
    pub pid: u32,
    pub cwd: String,
    pub agent: Option<String>,
    pub status: Option<String>,
    pub waiting_for: Option<String>,
    pub status_updated_at: Option<f64>,
    pub session_id: Option<String>,
    pub name: Option<String>,
    pub messaging_socket: Option<String>,
}

pub fn home() -> String {
    env::var("HOME").unwrap_or_default()
}

pub fn claude_dir() -> String {
    match env::var("CLAUDE_CONFIG_DIR") {
        Ok(dir) if !dir.is_empty() => dir,
        _ => format!("{}/.local/share/claude", home()),
    }
}

/// Fields 3.. of /proc/<pid>/stat (the command name, field 2, may contain spaces and parentheses): the parent is
/// [1], the start time (field 22) [19].
fn stat_fields(proc: &Path, pid: u32) -> Option<Vec<String>> {
    let stat = fs::read(proc.join(pid.to_string()).join("stat")).ok()?;
    let stat = String::from_utf8_lossy(&stat);
    Some(
        stat.rsplit_once(')')?
            .1
            .split_whitespace()
            .map(str::to_string)
            .collect(),
    )
}

fn parent_of(pid: u32) -> Option<u32> {
    stat_fields(Path::new("/proc"), pid)?.get(1)?.parse().ok()
}

fn parse_claude(path: &Path, proc: &Path) -> Option<Claude> {
    let entry: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let pid = match entry.get("pid")? {
        Value::Number(n) => n.as_u64()?,
        Value::String(s) => s.trim().parse::<u64>().ok()?,
        _ => return None,
    };
    let pid = u32::try_from(pid).ok()?;
    let start = entry.get("procStart")?.as_str()?;
    if stat_fields(proc, pid)?.get(19).map(String::as_str) != Some(start) {
        return None;
    }
    let dir = proc.join(pid.to_string());
    let cwd = fs::read_link(dir.join("cwd")).ok()?;
    let environ = fs::read(dir.join("environ")).ok()?;
    let agent = environ
        .split(|b| *b == 0)
        .find_map(|var| var.strip_prefix(b"AGENT_SESSION="))
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .filter(|name| valid_name(name));
    let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
    Some(Claude {
        pid,
        cwd: cwd.to_string_lossy().into_owned(),
        agent,
        status: text("status"),
        waiting_for: text("waitingFor"),
        status_updated_at: entry.get("statusUpdatedAt").and_then(Value::as_f64),
        session_id: text("sessionId"),
        name: text("name"),
        messaging_socket: text("messagingSocketPath"),
    })
}

pub fn live_claudes() -> Vec<Claude> {
    load_claudes(
        &Path::new(&claude_dir()).join("sessions"),
        Path::new("/proc"),
    )
}

/// The Claudes of the registry files in sessions whose process in proc is still the one that registered.
pub fn load_claudes(sessions: &Path, proc: &Path) -> Vec<Claude> {
    let Ok(entries) = fs::read_dir(sessions) else {
        return vec![];
    };
    let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    files.sort();
    files.iter().filter_map(|f| parse_claude(f, proc)).collect()
}

/// The live Claude sessions with their working directory in d.
pub fn claude_sessions(d: &str) -> Vec<Claude> {
    let d = realpath(d);
    live_claudes()
        .into_iter()
        .filter(|c| Path::new(&c.cwd).starts_with(&d))
        .collect()
}

/// pids and all their descendant processes.
pub fn descendants(pids: &[u32]) -> HashSet<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(ppid) = parent_of(pid) {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut found = HashSet::new();
    let mut todo = pids.to_vec();
    while let Some(pid) = todo.pop() {
        if found.insert(pid) {
            todo.extend(children.get(&pid).into_iter().flatten());
        }
    }
    found
}

/// Whether some process, other than those in ignore, has its working directory in d.
pub fn in_use(d: &str, ignore: &HashSet<u32>) -> bool {
    let d = realpath(d);
    for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if ignore.contains(&pid) {
            continue;
        }
        if fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|cwd| cwd.starts_with(&d)) {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[path = "../tests/unit/procs.rs"]
mod unit_tests;
