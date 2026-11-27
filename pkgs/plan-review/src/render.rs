use crate::server::{Cell, Note, Submission};
use crate::store::{oneline, Meta};

/// The message for the session about feedback on revision `revision` of the review `id`.
pub fn render(origin: &str, id: &str, revision: u64, meta: &Meta, feedback: &Submission) -> String {
    let path = oneline(&meta.path);
    let mut lines = vec![
        format!(
            "Feedback from the user on plan review \"{}\" (r{revision} of {path}), sent from the plan-review page.",
            oneline(&meta.title)
        ),
        format!("Full feedback: curl -s {origin}/api/feedback/{id}/{revision} | jq -r .text"),
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
