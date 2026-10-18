use std::fmt::Write;

use serde_json::json;

use crate::args::{ListArgs, StatusFilter};
use crate::parse::parse_date;
use crate::store::{Result, Store};
use crate::task::{Status, Task};

pub(crate) fn list(store: &mut Store, args: ListArgs, json: bool, out: &mut String) -> Result<()> {
    let ListArgs {
        query,
        status,
        tags,
        due_before,
    } = args;
    let before = due_before
        .map(|s| parse_date(&s).ok_or_else(|| format!("invalid date: {s}")))
        .transpose()?;
    let query = query.map(|q| q.to_lowercase());
    let keep = |t: &Task| {
        let has_tag = |tag: &String| {
            let sub = format!("{tag}/");
            t.tags.iter().any(|g| g == tag || g.starts_with(&sub))
        };
        query
            .as_ref()
            .is_none_or(|q| t.description.to_lowercase().contains(q))
            && status.is_none_or(|f| match f {
                StatusFilter::Open => t.status != Status::Done,
                StatusFilter::Todo => t.status == Status::Todo,
                StatusFilter::Doing => t.status == Status::Doing,
                StatusFilter::Done => t.status == Status::Done,
            })
            && tags.iter().all(has_tag)
            && before.is_none_or(|b| t.due.is_some_and(|d| d <= b))
    };

    let rows: Vec<(usize, &str, Task)> = store
        .tasks()
        .into_iter()
        .filter(|(_, t)| keep(t))
        .map(|(l, task)| (l + 1, store.file.text(l), task))
        .collect();

    if json {
        let items: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|(line, text, task)| {
                json!({
                    "line": line,
                    "status": task.status.to_string(),
                    "description": task.description,
                    "tags": task.tags,
                    "est": task.est.map(|e| e.to_string()),
                    "due": task.due.map(|d| d.to_string()),
                    "created": task.created.map(|d| d.to_string()),
                    "done_on": task.done_on.map(|d| d.to_string()),
                    "text": text,
                })
            })
            .collect();
        out!(out, "{}", json!(items));
    } else {
        for (line, text, _) in rows {
            out!(out, "{line}: {text}");
        }
    }
    Ok(())
}
