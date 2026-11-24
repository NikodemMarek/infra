use std::io::{self, Read};

use serde_json::Value;

use crate::poll::quoted_body;
use crate::record::{valid_name, Records};
use crate::shpool::Shpool;

/// A turn end whose whole message is this only waits on the session's own background work: not relayed.
const WAITING: &str = "waiting";

/// The JSON line `{"to", "text"}` that `jw msg` delivers for the claude event in input; None for an event that
/// reaches no supervisor.
fn notice(input: &str) -> Result<Option<String>, String> {
    let event = match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(map)) if map.get("type").is_some_and(Value::is_string) => map,
        _ => return Err("invalid claude event".to_string()),
    };
    if event["type"] != "claude.waiting" {
        return Ok(None);
    }
    let Some(child) = event
        .get("session")
        .and_then(Value::as_str)
        .filter(|s| valid_name(s))
    else {
        return Ok(None);
    };
    let records = Records::new();
    let Some(record) = records.read(child) else {
        return Ok(None);
    };
    let Some(to) = record
        .reports_to
        .as_deref()
        .filter(|s| valid_name(s) && *s != child)
    else {
        return Ok(None);
    };
    let Some(supervisor) = records.read(to) else {
        return Ok(None);
    };
    if supervisor.status.as_deref() == Some("done") || supervisor.reaped.is_some() {
        return Ok(None);
    }
    let attached = Shpool
        .list()
        .is_ok_and(|state| state.get(child).map(String::as_str) == Some("attached"));
    if attached {
        return Ok(None);
    }
    let body = event
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let done = record.status.as_deref() == Some("done");
    if !done && body.trim() == WAITING {
        return Ok(None);
    }
    let quoted = quoted_body(body);
    let quoted = quoted.strip_prefix('\n').unwrap_or("  > (no message)");
    let text = if done {
        format!(
            "Session {child}, which reports to you, is done (jw done); its last message:\n\n{quoted}\n\nIt is stopped once it is idle and nobody is attached."
        )
    } else {
        format!(
            "Session {child}, which reports to you, ended its turn with this message:\n\n{quoted}"
        )
    };
    Ok(Some(
        serde_json::json!({"to": to, "text": text}).to_string(),
    ))
}

/// Reads a claude event on stdin and prints the JSON line `{"to", "text"}` that `jw msg` delivers to the session
/// the event's session reports to; nothing for an event without a message.
pub fn cmd_report_notice() -> Result<(), String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    if let Some(line) = notice(&input)? {
        println!("{line}");
    }
    Ok(())
}
