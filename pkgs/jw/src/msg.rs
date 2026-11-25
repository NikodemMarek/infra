use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::{Duration, Instant};

use crate::procs::{live_claudes, Claude};
use crate::record::Records;
use crate::worker::{self, Delivery};
use crate::workspace::{basename, dirname, has_agents_suffix};
use crate::MsgArgs;

const MSG_WAIT: Duration = Duration::from_secs(10);
pub(crate) const MSG_MAX: usize = 8000;

/// Who a message is from: the agent session, else the workspace, else the user at a terminal.
fn sender() -> String {
    if let Some(name) = env::var("AGENT_SESSION")
        .ok()
        .filter(|name| !name.is_empty())
    {
        return name;
    }
    let project = env::var("CLAUDE_PROJECT_DIR").unwrap_or_default();
    if has_agents_suffix(dirname(&project)) {
        return basename(&project).to_string();
    }
    if env::var_os("CLAUDECODE").is_none() {
        "user".to_string()
    } else {
        "unknown".to_string()
    }
}

/// (pid, session label, socket path) of the Claude sessions called name, working in a directory of that name or being
/// that agent session, that listen for messages.
fn message_sessions(claudes: Vec<Claude>, name: &str) -> Vec<(u32, String, String)> {
    claudes
        .into_iter()
        .filter_map(|c| {
            let socket = c.messaging_socket?;
            let matches = c.name.as_deref() == Some(name)
                || basename(&c.cwd) == name
                || c.agent.as_deref() == Some(name);
            if !matches {
                return None;
            }
            let label = c
                .name
                .filter(|n| !n.is_empty())
                .or(c.agent)
                .unwrap_or_else(|| "?".to_string());
            Some((c.pid, label, socket))
        })
        .collect()
}

/// Text as a quote under a header; every line break (also \r, U+2028/2029, U+0085) starts a quoted line and other
/// control characters go, so the text cannot pass for a header.
pub(crate) fn message_content(text: &str, origin: &str) -> String {
    let text = text
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}', '\u{85}'], "\n");
    let text: String = text
        .trim_end_matches('\n')
        .chars()
        .filter(|&c| c == '\t' || c == '\n' || !c.is_control())
        .collect();
    let origin: String = origin
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "._-".contains(*c))
        .take(64)
        .collect();
    let origin = if origin.is_empty() {
        "unknown"
    } else {
        origin.as_str()
    };
    let quoted: Vec<String> = text.split('\n').map(|l| format!("> {l}")).collect();
    let content = format!("Message from {origin}:\n\n{}", quoted.join("\n"));
    if content.chars().count() <= MSG_MAX {
        content
    } else {
        let head: String = content.chars().take(MSG_MAX).collect();
        format!("{head}\n[truncated]")
    }
}

fn send(socket: &str, line: &str) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(line.as_bytes())
}

/// Sends text from origin to the running session name, retrying for wait; returns the session's label.
fn deliver(name: &str, text: &str, origin: &str, wait: Duration) -> Result<String, String> {
    let message = serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": message_content(text, origin)},
    });
    let line = format!("{message}\n");
    let deadline = Instant::now() + wait;
    // a session just started may not have registered or opened its socket yet
    loop {
        let matches = message_sessions(live_claudes(), name);
        if matches.len() > 1 {
            let several: Vec<String> = matches
                .iter()
                .map(|(pid, label, _)| format!("{label} (pid {pid})"))
                .collect();
            return Err(format!(
                "several sessions match {name}: {}",
                several.join(", ")
            ));
        }
        let mut error = format!("no running session {name}");
        if let Some((_, label, socket)) = matches.first() {
            match send(socket, &line) {
                Ok(()) => return Ok(label.clone()),
                Err(e) => error = format!("cannot reach session {name}: {e}"),
            }
        }
        if Instant::now() >= deadline {
            return Err(error);
        }
        thread::sleep(Duration::from_millis(500));
    }
}

pub fn cmd_msg(a: MsgArgs) -> Result<(), String> {
    let words = match a.text.first().map(String::as_str) {
        Some("--") => &a.text[1..],
        _ => &a.text[..],
    };
    let text = if words.is_empty() {
        let mut stdin = String::new();
        std::io::stdin()
            .read_to_string(&mut stdin)
            .map_err(|e| e.to_string())?;
        stdin
    } else {
        words.join(" ")
    };
    if text.trim().is_empty() {
        return Err("empty message".to_string());
    }
    let origin = a.sender.unwrap_or_else(sender);
    if let Some(record) = Records::new().read(&a.name).filter(|r| r.headless) {
        if record.reaped.is_some() {
            return Err(format!("session {} is reaped", a.name));
        }
        match worker::deliver(&a.name, &message_content(&text, &origin))? {
            Delivery::Queued => eprintln!("jw: queued for {}", a.name),
            Delivery::Started => eprintln!("jw: started a turn of {}", a.name),
        }
        return Ok(());
    }
    let session = deliver(&a.name, &text, &origin, MSG_WAIT)?;
    eprintln!("jw: sent to {session}");
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/msg.rs"]
mod unit_tests;
