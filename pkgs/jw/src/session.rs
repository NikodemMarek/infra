use std::env;
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::record::{now, valid_name, Record, Records};
use crate::shpool::Shpool;
use crate::systemd::Systemd;
use crate::worker;
use crate::{
    SessionAckArgs, SessionGetArgs, SessionListArgs, SessionResultsArgs, SessionSetArgs,
    SessionStatusArgs,
};

const URL_MAX: usize = 2048;

/// Event sessions run on untrusted text (GitLab); what they could hand off or write would become instructions elsewhere.
pub fn in_event_session() -> bool {
    env::var("AGENT_ORIGIN").is_ok_and(|origin| origin.starts_with("event:"))
}

/// name, else the session this process runs in.
pub fn session_name(name: Option<&str>) -> Result<String, String> {
    let name = match name {
        Some(name) => name.to_string(),
        None => env::var("AGENT_SESSION")
            .ok()
            .filter(|name| !name.is_empty())
            .ok_or("no session: pass -s NAME or run in an agent session")?,
    };
    if !valid_name(&name) {
        return Err(format!("invalid session name: '{name}'"));
    }
    Ok(name)
}

fn agent_session_name() -> Result<String, String> {
    let name = env::var("AGENT_SESSION")
        .ok()
        .filter(|name| !name.is_empty())
        .ok_or("not in an agent session")?;
    if !valid_name(&name) {
        return Err(format!("invalid session name: {name}"));
    }
    Ok(name)
}

fn is_https(url: &str) -> bool {
    url.strip_prefix("https://")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| (b'!'..=b'~').contains(&b)))
}

pub fn cmd_session_get(a: SessionGetArgs) -> Result<(), String> {
    let name = session_name(a.session.as_deref())?;
    let record = Records::new()
        .read(&name)
        .ok_or_else(|| format!("no session record {name}"))?;
    println!(
        "{}",
        serde_json::to_string(&record).map_err(|e| e.to_string())?
    );
    Ok(())
}

#[derive(Serialize)]
struct Listed<'a> {
    name: &'a str,
    running: bool,
    /// The numbers of the results the supervisor has not acked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unacked: Vec<u64>,
    #[serde(flatten)]
    record: &'a Record,
}

pub fn cmd_session_list(a: SessionListArgs) -> Result<(), String> {
    if let Some(name) = a.reports_to.as_deref().filter(|name| !valid_name(name)) {
        return Err(format!("invalid session name: '{name}'"));
    }
    let running = Shpool.list()?;
    let records = Records::new();
    for (name, record) in records.load_all() {
        if a.reports_to.is_some() && record.reports_to != a.reports_to {
            continue;
        }
        let listed = Listed {
            name: &name,
            running: if record.headless {
                Systemd.is_active(&worker::unit(&name)) == Some(true)
            } else {
                running.contains_key(&name)
            },
            unacked: records.result_seqs(&name),
            record: &record,
        };
        println!(
            "{}",
            serde_json::to_string(&listed).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

pub fn cmd_session_results(a: SessionResultsArgs) -> Result<(), String> {
    let name = session_name(a.session.as_deref())?;
    for result in Records::new().results(&name) {
        println!(
            "{}",
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

pub fn cmd_session_ack(a: SessionAckArgs) -> Result<(), String> {
    if in_event_session() {
        return Err("event sessions cannot change session records".to_string());
    }
    Records::new().ack(&a.name, a.seq)
}

pub fn cmd_session_set(a: SessionSetArgs) -> Result<(), String> {
    if in_event_session() {
        return Err("event sessions cannot change session records".to_string());
    }
    let name = session_name(a.session.as_deref())?;
    for url in a.url.iter().chain(&a.mr) {
        if url.chars().count() > URL_MAX || !is_https(url) {
            return Err(format!("not an https url: {url}"));
        }
    }
    let restarted = a.workspace.as_deref().is_some_and(|w| !w.is_empty());
    let fields = Record {
        kind: a.kind,
        title: a.title,
        description: a.description,
        url: a.url,
        workspace: a.workspace,
        ..Record::default()
    };
    Records::new().update(&name, |r| {
        r.merge(fields);
        if restarted {
            r.clear_marks();
        }
        for url in &a.mr {
            if !r.mrs.contains(url) {
                r.mrs.push(url.clone());
            }
        }
        true
    })?;
    Ok(())
}

pub fn cmd_session_status(a: SessionStatusArgs) -> Result<(), String> {
    let name = agent_session_name()?;
    let state = a.state;
    if a.waiting_for.is_some() && state != "waiting" {
        return Err("--waiting-for needs waiting".to_string());
    }
    let waiting_for: String = a
        .waiting_for
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let waiting_for: String = waiting_for.trim().chars().take(200).collect();
    Records::new().set_status(
        &name,
        &state,
        (!waiting_for.is_empty()).then_some(waiting_for),
        a.cache_expires_at,
    )
}

/// Records dir in the worktrees of this agent session.
pub fn track_worktree(dir: &str) -> Result<(), String> {
    let name = agent_session_name()?;
    let real = Some(Path::new(dir))
        .filter(|d| d.is_absolute())
        .and_then(|d| fs::canonicalize(d).ok())
        .filter(|d| d.is_dir())
        .and_then(|d| d.into_os_string().into_string().ok())
        .ok_or_else(|| format!("not an absolute directory: {dir}"))?;
    Records::new().update(&name, |r| {
        let known = r.worktrees.len();
        r.worktrees.retain(|w| Path::new(w).exists());
        let pruned = r.worktrees.len() != known;
        if r.worktrees.contains(&real) {
            return pruned;
        }
        r.worktrees.push(real);
        true
    })?;
    Ok(())
}

pub fn cmd_done() -> Result<(), String> {
    let name = agent_session_name()?;
    Records::new().update(&name, |r| {
        r.status = Some("done".to_string());
        r.status_at = Some(now());
        r.waiting_for = None;
        true
    })?;
    eprintln!("jw: {name} is done; it is stopped once it is idle and detached, and the worktrees it created are removed");
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/session.rs"]
mod tests;
