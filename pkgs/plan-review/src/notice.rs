use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::server::{Cell, Note, Submission};
use crate::store::{oneline, review_ref, Meta, ReviewRef, Store};

const MAX_FRAMED: usize = 7900;
/// What jw wraps around a message: `Message from events:\n\n` and `> ` per line.
const FRAME: usize = 22;
const PER_LINE: usize = 2;
const TRUNCATED: &str = "… truncated, read the full feedback";

#[derive(Serialize)]
struct SessionMessage {
    to: String,
    text: String,
}

/// Handle a `review.submitted` event: the line for the session-message handler, `None` when there is nothing to send.
pub fn notice(store: &Store, event: &str) -> Result<Option<String>, String> {
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
    let Some(meta) = store.read_meta(&id) else {
        return Ok(None);
    };
    let file = store.feedback_path(&id, revision);
    let Some(feedback) = fs::read(&file)
        .ok()
        .and_then(|data| serde_json::from_slice::<Submission>(&data).ok())
    else {
        return Ok(None);
    };
    let text = fit(&render(&file, revision, &meta, &feedback));
    let line = serde_json::to_string(&SessionMessage {
        to: meta.session,
        text,
    })
    .map_err(|e| e.to_string())?;
    Ok(Some(line))
}

fn render(file: &Path, revision: u64, meta: &Meta, feedback: &Submission) -> String {
    let path = oneline(&meta.path);
    let mut lines = vec![
        format!(
            "Feedback from the user on plan review \"{}\" (r{revision} of {path}), sent from the plan-review page.",
            oneline(&meta.title)
        ),
        format!("Full feedback: {}", file.display()),
        format!("Revise, then edit {path} and run plan-review publish again for the next revision."),
    ];
    if !feedback.notes.is_empty() {
        lines.push("Notes:".to_string());
        lines.extend(
            feedback
                .notes
                .iter()
                .map(|note| format!("- [{}] {}", target(note), oneline(&note.text))),
        );
    }
    if !feedback.answers.is_empty() {
        let answers: Vec<String> = feedback
            .answers
            .iter()
            .map(|(key, value)| format!("{}={}", oneline(key), oneline(value)))
            .collect();
        lines.push(format!(
            "Page answers (set by the page's controls, possibly without the user): {}",
            answers.join(", ")
        ));
    }
    let message = message_lines(&feedback.message);
    if !message.is_empty() {
        lines.push("Message typed by the user:".to_string());
        lines.extend(message);
    }
    lines.join("\n")
}

fn target(note: &Note) -> String {
    let selector = oneline(&note.selector);
    let excerpt = oneline(&note.excerpt);
    match &note.cell {
        Some(Cell { table, row, col }) => format!(
            "table \"{}\" row \"{}\" col \"{}\"",
            oneline(table),
            oneline(row),
            oneline(col)
        ),
        None if !excerpt.is_empty() => format!("{selector} \"{excerpt}\""),
        None => selector,
    }
}

/// The message as `  > <line>` lines; none when it is blank.
fn message_lines(message: &str) -> Vec<String> {
    let message: String = message
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}', '\u{85}'], "\n")
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect();
    let message = message.trim_end_matches('\n');
    if message.trim().is_empty() {
        return vec![];
    }
    message
        .split('\n')
        .map(|line| match line {
            "" => "  >".to_string(),
            _ => format!("  > {line}"),
        })
        .collect()
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
