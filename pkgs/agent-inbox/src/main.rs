//! agent-inbox: terminal UI over the event router's queue (see ADR-023): inspect events, answer
//! sudo password requests and, via the permission hook, Claude permission prompts and
//! questions (ADR-025); the last tab (`f`) shows the live Claude sessions (attach, kill, start a
//! new one; `jw` must be on PATH, the Nix package adds it). Only agents started by `jw agent` or the
//! GitLab event handler (those with CLAUDE_ORIGIN_FILE) are listed, `o` opens their merge
//! request or issue (`xdg-open`, also added by the Nix package).
//!
//!   agent-inbox              interactive UI; $AGENT_INBOX_TABS (JSON, set by the Nix package)
//!                            lists its filter tabs: [{"name":"Review","kinds":["review"]},
//!                            {"name":"Coding"}], a tab without kinds shows the rest
//!   agent-inbox list         id, age, type, project, title (TSV, newest first)
//!   agent-inbox reply <id>   read one line from stdin, write it to the item's reply FIFO, dismiss
//!                            (sudo: the password; claude.*: a JSON decision such as
//!                            {"behavior":"allow"}, see ADR-025)
//!   agent-inbox dismiss <id>
//!   agent-inbox sessions     live Claude sessions with an origin: pid, status, age of the status,
//!                            shpool session, place, session id (TSV, newest first; read from the
//!                            session registry)

mod client;
mod reply;
mod sessions;
mod tui;

use std::env;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use client::{clean, Result};
use reply::ReplyError;
use serde_json::Value;
use tui::Tab;

const USAGE: &str = "usage: agent-inbox [list | reply <id> | dismiss <id> | sessions]";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        [] => run_tui(),
        ["list"] => list(),
        ["reply", id] => answer(id),
        ["dismiss", id] => dismiss(id),
        ["sessions"] => sessions(),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agent-inbox: {e}");
            ExitCode::FAILURE
        }
    }
}

/// XDG_RUNTIME_DIR, else /run/user/<euid> (as askpass.sh does).
fn runtime_dir() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(
            || {
                // SAFETY: geteuid has no preconditions and cannot fail.
                PathBuf::from(format!("/run/user/{}", unsafe { libc::geteuid() }))
            },
            PathBuf::from,
        )
}

/// The filter tabs in the JSON `raw` (unset or empty: none): a list of objects with a `name` and
/// optional `kinds`, a list of strings or null.
fn parse_tabs(raw: &str) -> Result<Vec<Tab>> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let bad = |why: &str| format!("AGENT_INBOX_TABS: {why}");
    let value: Value = serde_json::from_str(raw).map_err(|e| bad(&e.to_string()))?;
    let list = value.as_array().ok_or_else(|| bad("not a list"))?;
    list.iter()
        .map(|tab| -> Result<Tab> {
            let name = tab
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .ok_or_else(|| bad("a tab needs a name"))?;
            let kinds = match tab.get("kinds") {
                None | Some(Value::Null) => None,
                Some(Value::Array(kinds)) => Some(
                    kinds
                        .iter()
                        .map(|k| k.as_str().map(String::from))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| bad("kinds must be strings"))?,
                ),
                Some(_) => return Err(bad("kinds must be a list or null").into()),
            };
            Ok(Tab {
                name: name.into(),
                kinds,
            })
        })
        .collect()
}

fn run_tui() -> Result<()> {
    let tabs = parse_tabs(&env::var("AGENT_INBOX_TABS").unwrap_or_default())?;
    let base = client::base()?;
    let sessions_dir = sessions::config_dir().join("sessions");
    tui::run(&base, Some(runtime_dir().as_path()), &sessions_dir, tabs)?;
    Ok(())
}

fn list() -> Result<()> {
    let items = client::list(&client::base()?)?;
    let now = client::now();
    let mut out = io::stdout().lock();
    for item in items.iter().rev() {
        writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}",
            item.id,
            item.age(now),
            clean(item.kind()),
            clean(item.project()),
            clean(item.title())
        )?;
    }
    Ok(())
}

fn sessions() -> Result<()> {
    let dir = sessions::config_dir().join("sessions");
    let now = client::now();
    let mut out = io::stdout().lock();
    for s in sessions::load(&dir, Path::new("/proc")) {
        writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}",
            s.pid,
            clean(&s.status),
            client::age(s.status_at, now),
            clean(&s.shpool),
            clean(&s.place()),
            clean(&s.session_id)
        )?;
    }
    Ok(())
}

fn answer(id: &str) -> Result<()> {
    let runtime = runtime_dir();
    let base = client::base()?;
    let items = client::list(&base)?;
    let item = items
        .iter()
        .find(|i| i.id == id)
        .ok_or("no such inbox item")?;
    let target = item.reply().ok_or("inbox item has no reply target")?;
    let prefix =
        reply::reply_prefix(item.kind()).ok_or("this kind of inbox item cannot be answered")?;
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Err("pipe the answer on stdin; use the TUI for interactive entry".into());
    }
    let mut input = String::new();
    stdin.lock().read_line(&mut input)?;
    let line = input.trim_end_matches(['\n', '\r']);
    if line.is_empty() {
        return Err("empty answer".into());
    }
    // a decision for a hook is one JSON object (the hook validates the rest)
    if item.kind().starts_with("claude.")
        && !matches!(serde_json::from_str::<Value>(line), Ok(Value::Object(_)))
    {
        return Err("the answer must be a JSON object on one line".into());
    }
    let outcome = reply::write_reply(&runtime, prefix, Path::new(target), line);
    input.clear();
    // an expired request is as resolved as an answered one
    if matches!(outcome, Ok(()) | Err(ReplyError::Expired)) {
        client::dismiss(&base, id)?;
    }
    outcome.map_err(Into::into)
}

fn dismiss(id: &str) -> Result<()> {
    if client::dismiss(&client::base()?, id)? {
        Ok(())
    } else {
        Err("no such inbox item".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(name: &str, kinds: Option<&[&str]>) -> Tab {
        Tab {
            name: name.into(),
            kinds: kinds.map(|k| k.iter().map(|s| s.to_string()).collect()),
        }
    }

    #[test]
    fn tabs_are_read_from_json() {
        let raw = r#"[{"name":"Review","kinds":["review","note"]},{"name":"Coding"},
            {"name":"Rest","kinds":null}]"#;
        assert_eq!(
            parse_tabs(raw).unwrap(),
            vec![
                tab("Review", Some(&["review", "note"])),
                tab("Coding", None),
                tab("Rest", None),
            ]
        );
    }

    #[test]
    fn unset_or_empty_means_no_tabs() {
        assert_eq!(parse_tabs("").unwrap(), vec![]);
        assert_eq!(parse_tabs(" \n").unwrap(), vec![]);
        assert_eq!(parse_tabs("[]").unwrap(), vec![]);
    }

    #[test]
    fn invalid_tabs_are_rejected() {
        for raw in [
            "{",
            r#"{"name":"A"}"#,
            r#"[{"kinds":["a"]}]"#,
            r#"[{"name":""}]"#,
            r#"[{"name":"A","kinds":"a"}]"#,
            r#"[{"name":"A","kinds":[1]}]"#,
            r#"["A"]"#,
        ] {
            let e = parse_tabs(raw).unwrap_err().to_string();
            assert!(e.starts_with("AGENT_INBOX_TABS: "), "{raw}: {e}");
        }
    }
}
