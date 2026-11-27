use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::client::{self, api_error};
use crate::store::{review_ref, ReviewRef};

const MAX_FRAMED: usize = 7900;
/// What jw wraps around a message: `Message from events:\n\n` and `> ` per line.
const FRAME: usize = 22;
const PER_LINE: usize = 2;
const TRUNCATED: &str = "… truncated, read the full feedback";

#[derive(Serialize, Deserialize)]
struct SessionMessage {
    to: String,
    text: String,
}

/// Handle a `review.submitted` event: the line for the session-message handler, `None` when there is nothing to send.
pub fn notice(event: &str) -> Result<Option<String>, String> {
    let fields: Map<String, Value> =
        serde_json::from_str(event).map_err(|_| "the event is not a JSON object")?;
    if fields.get("type").and_then(Value::as_str) != Some("review.submitted") {
        return Ok(None);
    }
    let Some(ReviewRef {
        review: id,
        revision,
    }) = review_ref(event)?
    else {
        return Ok(None);
    };
    let (status, body) = client::request("GET", &format!("/api/feedback/{id}/{revision}"), b"")?;
    match status {
        200 => {}
        404 => return Ok(None),
        _ => return Err(api_error(status, &body)),
    }
    let feedback: SessionMessage = serde_json::from_slice(&body)
        .map_err(|e| format!("the plan-review server sent a malformed answer: {e}"))?;
    let line = serde_json::to_string(&SessionMessage {
        to: feedback.to,
        text: fit(&feedback.text),
    })
    .map_err(|e| e.to_string())?;
    Ok(Some(line))
}

/// How much of the session-message budget `text` takes once jw has framed it.
fn framed(text: &str) -> usize {
    FRAME + text.chars().count() + PER_LINE * text.split('\n').count()
}

/// `text` if it fits the budget, else its longest prefix of whole lines (the last one may be cut) plus a truncation line.
fn fit(text: &str) -> String {
    if framed(text) <= MAX_FRAMED {
        return text.to_string();
    }
    let mut room = MAX_FRAMED.saturating_sub(FRAME + 1 + TRUNCATED.chars().count() + PER_LINE);
    let mut kept = String::new();
    for (i, line) in text.split('\n').enumerate() {
        let overhead = usize::from(i > 0) + PER_LINE;
        let cost = line.chars().count() + overhead;
        if cost <= room {
            room -= cost;
            if i > 0 {
                kept.push('\n');
            }
            kept.push_str(line);
            continue;
        }
        let cut = room.saturating_sub(overhead);
        if cut > 0 {
            if i > 0 {
                kept.push('\n');
            }
            kept.extend(line.chars().take(cut));
        }
        break;
    }
    format!("{kept}\n{TRUNCATED}")
}
