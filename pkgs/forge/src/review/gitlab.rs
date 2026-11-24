use serde::Deserialize;
use serde_json::{json, Value};

use super::{partial, Comment, Review, Side};
use crate::api::Api;
use crate::cmd::Result;
use crate::pr::gitlab::view_path;
use crate::target::Target;

#[derive(Deserialize)]
struct Discussion {
    id: String,
    #[serde(default)]
    individual_note: bool,
}

#[derive(Deserialize)]
struct Version {
    id: u64,
    head_commit_sha: String,
    base_commit_sha: String,
    start_commit_sha: String,
}

#[derive(Deserialize)]
struct VersionDiffs {
    diffs: Vec<FileDiff>,
}

#[derive(Deserialize)]
struct FileDiff {
    old_path: String,
    new_path: String,
    #[serde(default)]
    diff: String,
}

pub fn submit(
    api: &Api,
    target: &Target,
    number: u64,
    commit: Option<&str>,
    review: &Review,
) -> Result<()> {
    let mr = view_path(&target.project, number);
    let notes_path = format!("{mr}/draft_notes");
    let existing: Vec<Value> = api.get_ndjson(&notes_path)?;
    if !existing.is_empty() {
        return Err(format!(
            "you already have {} draft notes on !{number}: publish or delete them first",
            existing.len()
        ));
    }

    let mut problems = Vec::new();
    let mut replies = Vec::new();
    if !review.replies.is_empty() {
        let discussions: Vec<Discussion> =
            api.get_ndjson(&format!("{mr}/discussions?per_page=100"))?;
        for (i, r) in review.replies.iter().enumerate() {
            if discussions
                .iter()
                .any(|d| d.id == r.thread && !d.individual_note)
            {
                replies.push(json!({
                    "note": escape(&r.body),
                    "in_reply_to_discussion_id": r.thread,
                    "resolve_discussion": r.resolve,
                }));
            } else {
                problems.push(format!(
                    "replies[{i}]: thread {} not found on !{number}",
                    r.thread
                ));
            }
        }
    }
    let comments = if review.comments.is_empty() {
        Vec::new()
    } else {
        inline_notes(api, &mr, number, commit, &review.comments, &mut problems)?
    };
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }

    let notes: Vec<Value> = review
        .summary
        .iter()
        .map(|s| json!({ "note": escape(s) }))
        .chain(comments)
        .chain(replies)
        .collect();
    for (created, note) in notes.iter().enumerate() {
        api.send::<Value>("POST", &notes_path, note)
            .map_err(|e| partial(e, created, notes.len()))?;
    }
    Ok(())
}

fn inline_notes(
    api: &Api,
    mr: &str,
    number: u64,
    commit: Option<&str>,
    comments: &[Comment],
    problems: &mut Vec<String>,
) -> Result<Vec<Value>> {
    let versions: Vec<Version> = api.get(&format!("{mr}/versions?per_page=100"))?;
    let version = match commit {
        Some(sha) => versions
            .iter()
            .find(|v| v.head_commit_sha == sha)
            .ok_or_else(|| format!("commit {sha} is not a version of !{number}"))?,
        None => versions
            .first()
            .ok_or_else(|| format!("!{number} has no versions"))?,
    };
    let detail: VersionDiffs = api.get(&format!("{mr}/versions/{}", version.id))?;

    let mut notes = Vec::new();
    for (i, c) in comments.iter().enumerate() {
        let Some(file) = detail
            .diffs
            .iter()
            .find(|d| d.new_path == c.path || d.old_path == c.path)
        else {
            problems.push(format!("comments[{i}]: {} is not in the diff", c.path));
            continue;
        };
        let Some((new_line, old_line)) = locate(&file.diff, c.side, c.line) else {
            problems.push(format!(
                "comments[{i}]: {}:{} ({}) is not in the diff",
                c.path,
                c.line,
                c.side.name()
            ));
            continue;
        };
        let mut position = json!({
            "position_type": "text",
            "base_sha": version.base_commit_sha,
            "start_sha": version.start_commit_sha,
            "head_sha": version.head_commit_sha,
            "old_path": file.old_path,
            "new_path": file.new_path,
        });
        if let Some(line) = new_line {
            position["new_line"] = line.into();
        }
        if let Some(line) = old_line {
            position["old_line"] = line.into();
        }
        notes.push(json!({ "note": escape(&c.body), "position": position }));
    }
    Ok(notes)
}

/// The (new_line, old_line) position of `line` on `side` in a unified diff, if the diff shows it.
fn locate(diff: &str, side: Side, line: i64) -> Option<(Option<i64>, Option<i64>)> {
    let (mut old, mut new) = (0, 0);
    for l in diff.lines() {
        if let Some(header) = l.strip_prefix("@@ -") {
            if let Some(start) = hunk_start(header) {
                (old, new) = start;
            }
            continue;
        }
        match l.as_bytes().first() {
            Some(b'+') => {
                if side == Side::New && new == line {
                    return Some((Some(new), None));
                }
                new += 1;
            }
            Some(b'-') => {
                if side == Side::Old && old == line {
                    return Some((None, Some(old)));
                }
                old += 1;
            }
            Some(b' ') => {
                if (side == Side::New && new == line) || (side == Side::Old && old == line) {
                    return Some((Some(new), Some(old)));
                }
                old += 1;
                new += 1;
            }
            _ => {}
        }
    }
    None
}

/// Parses `o[,c] +n[,c] @@` (a hunk header without its `@@ -` prefix).
fn hunk_start(header: &str) -> Option<(i64, i64)> {
    let (old, new) = header.split_once(" +")?;
    let first = |s: &str| -> Option<i64> { s.split([',', ' ']).next()?.parse().ok() };
    Some((first(old)?, first(new)?))
}

/// Keeps a leading `/` from running a quick action outside code blocks; a note with
/// lines starting with `<` or `>>>` is escaped everywhere.
fn escape(body: &str) -> String {
    let lines: Vec<&str> = body.split('\n').collect();
    let everywhere = lines
        .iter()
        .any(|l| l.starts_with('<') || l.starts_with(">>>"));
    let mut fenced = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("```") {
            // GitLab's block regex ^```.+?\n```$ needs at least one character between the fences
            let start = i + if lines[i] == "```" { 2 } else { 1 };
            if let Some(offset) = lines.iter().skip(start).position(|l| *l == "```") {
                let close = start + offset;
                fenced[i..=close].fill(true);
                i = close;
            }
        }
        i += 1;
    }
    lines
        .iter()
        .zip(&fenced)
        .map(|(l, in_block)| {
            if l.starts_with('/') && (everywhere || !in_block) {
                format!("\\{l}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
