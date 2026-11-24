use std::io::{self, Read};

use serde_json::{Map, Value};

use crate::msg::MSG_MAX;
use crate::poll::{quoted_body, quoted_head};
use crate::record::{valid_name, Record, Records, ResultKind};
use crate::shpool::Shpool;

/// A turn end whose whole message is this only waits on the session's own background work: not relayed.
const WAITING: &str = "waiting";

/// The message for a turn end of child, which is not done or attached: its last message as the event's body.
fn waiting_text(child: &str, record: &Record, event: &Map<String, Value>) -> Option<String> {
    let attached = Shpool
        .list()
        .is_ok_and(|state| state.get(child).map(String::as_str) == Some("attached"));
    if attached {
        return None;
    }
    let body = event
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let done = record.status.as_deref() == Some("done");
    if !done && body.trim() == WAITING {
        return None;
    }
    let quoted = quoted_body(body);
    let quoted = quoted.strip_prefix('\n').unwrap_or("  > (no message)");
    Some(if done {
        format!(
            "Session {child}, which reports to you, is done (jw done); its last message:\n\n{quoted}\n\nIt is stopped once it is idle and nobody is attached."
        )
    } else {
        format!(
            "Session {child}, which reports to you, ended its turn with this message:\n\n{quoted}"
        )
    })
}

/// The message for a result of child: None once the supervisor acked it (its file is gone).
fn result_text(records: &Records, child: &str, event: &Map<String, Value>) -> Option<String> {
    let seq = event.get("seq").and_then(Value::as_u64)?;
    let result = records.results(child).into_iter().find(|r| r.seq == seq)?;
    let quoted = quoted_head(&result.text, MSG_MAX);
    let quoted = quoted.strip_prefix('\n').unwrap_or("  > (no message)");
    let what = match result.kind {
        ResultKind::Result => "finished a turn",
        ResultKind::Died => "died during a turn",
    };
    Some(format!(
        "Session {child}, which reports to you, {what} (result {seq}; full text: `jw session results -s {child}`; once handled: `jw session ack {child} {seq}`):\n\n{quoted}"
    ))
}

/// The JSON line `{"to", "text"}` that `jw msg` delivers for the claude event in input; None for an event that
/// reaches no supervisor.
fn notice(input: &str) -> Result<Option<String>, String> {
    let event = match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(map)) if map.get("type").is_some_and(Value::is_string) => map,
        _ => return Err("invalid claude event".to_string()),
    };
    if event["type"] != "claude.waiting" && event["type"] != "session.result" {
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
    let text = if event["type"] == "session.result" {
        result_text(&records, child, &event)
    } else {
        waiting_text(child, &record, &event)
    };
    Ok(text.map(|text| serde_json::json!({"to": to, "text": text}).to_string()))
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
