//! Client of the inbox: a directory of one JSON file per queued event, written by the inbox
//! handler (`<id>.json`), and `event-pub` to publish events.

use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

const ORIGIN_MAX_BYTES: u64 = 64 * 1024;
const ITEM_MAX_BYTES: u64 = 1024 * 1024;

/// One queued event with the inbox's bookkeeping.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub received: f64,
    pub event: Value,
}

/// One question of an AskUserQuestion event.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub question: String,
    /// Short tag of the question (`header`); empty if there is none.
    pub header: String,
    pub multi: bool,
    /// Option labels; `descriptions[i]` belongs to `labels[i]` (empty if there is none).
    pub labels: Vec<String>,
    pub descriptions: Vec<String>,
}

/// What a session started by `jw agent` or the GitLab event handler works on (the JSON file
/// `CLAUDE_ORIGIN_FILE` points to); fields the file does not have are empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Origin {
    /// What the agent was started for (`review`, `note`, `build-failed`, `default`, ... for GitLab
    /// events, `task` for `jw agent`): picks the filter tab, see `tui::Tab`.
    pub kind: String,
    pub iid: Option<u64>,
    /// `MergeRequest` or `Issue`, "" for a task.
    pub target: String,
    pub title: String,
    pub url: String,
    pub description: String,
    pub prompt: String,
}

impl Origin {
    /// The origin file `path`, which must be a regular file of at most 64 KB directly in `dir`
    /// (the path comes from an event: untrusted) holding a JSON object.
    pub fn load(path: &Path, dir: &Path) -> Option<Origin> {
        if path.parent() != Some(dir) {
            return None;
        }
        // not a FIFO: opening it would block
        if !fs::metadata(path).ok()?.is_file() {
            return None;
        }
        let raw = read_limited(path, ORIGIN_MAX_BYTES)?;
        let v: Value = serde_json::from_str(&raw).ok()?;
        v.as_object()?;
        let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        Some(Origin {
            kind: text("kind"),
            iid: v.get("iid").and_then(Value::as_u64),
            target: text("target"),
            title: text("title"),
            url: text("url"),
            description: text("description"),
            prompt: text("prompt"),
        })
    }
}

/// A task for a new agent, published as a `task.new` event (which is routed to a repo unless
/// `repo` is given and starts the session).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Task {
    pub prompt: String,
    /// Absolute paths of images the agent should look at.
    pub images: Vec<String>,
    pub repo: Option<String>,
    /// Correlates the `task.started`/`task.failed` events with a task.
    pub id: Option<String>,
}

/// The `task.new` event for `task`: what is empty or unset is left out.
pub fn task_event(task: &Task) -> Value {
    let mut event = json!({"type": "task.new"});
    if !task.prompt.is_empty() {
        event["prompt"] = json!(task.prompt);
    }
    if !task.images.is_empty() {
        event["images"] = json!(task.images);
    }
    if let Some(repo) = &task.repo {
        event["repo"] = json!(repo);
    }
    if let Some(id) = &task.id {
        event["id"] = json!(id);
    }
    event
}

/// `!7 title` for a merge request, `#7 title` for an issue, else the title; None if there is
/// nothing to show.
pub fn origin_label(origin: &Origin) -> Option<String> {
    let label = match (origin.target.as_str(), origin.iid) {
        ("MergeRequest", Some(iid)) => format!("!{iid} {}", origin.title),
        ("Issue", Some(iid)) => format!("#{iid} {}", origin.title),
        _ => origin.title.clone(),
    };
    Some(clean(label.trim())).filter(|l| !l.is_empty())
}

impl Item {
    fn field(&self, key: &str) -> &str {
        self.event.get(key).and_then(Value::as_str).unwrap_or("")
    }

    pub fn kind(&self) -> &str {
        self.field("type")
    }

    pub fn project(&self) -> &str {
        self.field("project")
    }

    pub fn title(&self) -> &str {
        self.field("title")
    }

    pub fn cwd(&self) -> &str {
        self.field("cwd")
    }

    /// The Claude session the event belongs to, "" if there is none (sudo events).
    pub fn session_id(&self) -> &str {
        self.field("session_id")
    }

    /// The shpool session the event comes from (`jw attach <name>`), "" if there is none.
    pub fn shpool(&self) -> &str {
        self.field("shpool")
    }

    /// The origin file of the session the event comes from, "" for a hand-started one.
    pub fn origin_file(&self) -> &str {
        self.field("origin_file")
    }

    pub fn prompt(&self) -> &str {
        self.field("prompt")
    }

    /// Why a `task.failed` task failed.
    pub fn reason(&self) -> &str {
        self.field("reason")
    }

    /// The `id` the event carries (that of the task for `task.failed`), "" if there is none.
    pub fn event_id(&self) -> &str {
        self.field("id")
    }

    /// The paths in `images` (`task.failed`).
    pub fn images(&self) -> Vec<&str> {
        self.event
            .get("images")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    }

    /// The text of the event (the last message of Claude for `claude.waiting`).
    pub fn body(&self) -> &str {
        self.field("body")
    }

    pub fn command(&self) -> &str {
        self.field("command")
    }

    pub fn tool_name(&self) -> &str {
        self.field("tool_name")
    }

    pub fn agent_type(&self) -> &str {
        self.field("agent_type")
    }

    /// A string of `tool_input` (`command`, `file_path`, ...), "" if there is none.
    pub fn input(&self, key: &str) -> &str {
        self.event
            .get("tool_input")
            .and_then(|i| i.get(key))
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// A string of `jj` (the `jw info` of the cwd), "" if there is none.
    fn jj(&self, key: &str) -> &str {
        self.event
            .pointer(&format!("/jj/{key}"))
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// A friendly name for the source: the jj repo, else the repo of a session workspace
    /// (`<repo>.agents/<name>` in the cwd), else the last component of the cwd, else `project`,
    /// else `host`.
    pub fn project_name(&self) -> String {
        let cwd = self.cwd();
        let from_cwd = cwd
            .split('/')
            .find_map(|c| c.strip_suffix(".agents").filter(|n| !n.is_empty()))
            .or_else(|| cwd.rsplit('/').find(|c| !c.is_empty()));
        [
            self.jj("repo"),
            from_cwd.unwrap_or(""),
            self.project(),
            self.field("host"),
        ]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
    }

    /// The workspace: the jj one unless empty or `default`, else `<workspace>` of a cwd in
    /// `<repo>.agents/<workspace>`, else "".
    pub fn workspace(&self) -> String {
        let jj = self.jj("workspace");
        if !jj.is_empty() && jj != "default" {
            return jj.to_string();
        }
        let mut parts = self.cwd().split('/');
        parts
            .find(|c| c.strip_suffix(".agents").is_some_and(|r| !r.is_empty()))
            .and_then(|_| parts.next())
            .unwrap_or_default()
            .to_string()
    }

    /// `repo[/subdir] ws:workspace[ @change][ index/size]` like the statusline; None without jj.
    pub fn jj_line(&self) -> Option<String> {
        let repo = Some(self.jj("repo")).filter(|r| !r.is_empty())?;
        let mut line = repo.to_string();
        if !self.jj("subdir").is_empty() {
            line += &format!("/{}", self.jj("subdir"));
        }
        line += &format!(" ws:{}", self.jj("workspace"));
        if !self.jj("change").is_empty() {
            line += &format!(" @{}", self.jj("change"));
        }
        let stack = |key: &str| self.event.pointer(&format!("/jj/stack/{key}"));
        if let (Some(index), Some(size)) = (stack("index"), stack("size")) {
            line += &format!(" {index}/{size}");
        }
        Some(line)
    }

    /// Path of the FIFO an answer goes to, if the event has one.
    pub fn reply(&self) -> Option<&str> {
        Some(self.field("reply")).filter(|r| !r.is_empty())
    }

    /// The questions of an AskUserQuestion event (`tool_input.questions`); entries without a
    /// text are skipped.
    pub fn questions(&self) -> Vec<Question> {
        let Some(list) = self
            .event
            .pointer("/tool_input/questions")
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        list.iter()
            .filter_map(|q| {
                let text =
                    |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
                let mut labels = Vec::new();
                let mut descriptions = Vec::new();
                let options = q.get("options").and_then(Value::as_array);
                for option in options.into_iter().flatten() {
                    if let Some(label) = text(option, "label") {
                        labels.push(label);
                        descriptions.push(text(option, "description").unwrap_or_default());
                    }
                }
                Some(Question {
                    question: text(q, "question")?,
                    header: text(q, "header").unwrap_or_default(),
                    multi: q.get("multiSelect").and_then(Value::as_bool) == Some(true),
                    labels,
                    descriptions,
                })
            })
            .collect()
    }

    /// `5s`, `3m`, `2h`
    pub fn age(&self, now: f64) -> String {
        age(self.received, now)
    }
}

/// How long ago `since` was (seconds since the epoch): `5s`, `3m`, `2h`
pub fn age(since: f64, now: f64) -> String {
    let secs = (now - since).max(0.0) as u64;
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h", secs / 3600),
    }
}

/// Event text is untrusted (commands, file paths): control characters (escape sequences,
/// newlines, tabs) become spaces before it reaches a terminal or a TSV line.
pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// XDG_RUNTIME_DIR, else /run/user/<euid> (as askpass.sh does).
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(
            || {
                // SAFETY: geteuid has no preconditions and cannot fail.
                PathBuf::from(format!("/run/user/{}", unsafe { libc::geteuid() }))
            },
            PathBuf::from,
        )
}

/// `<runtime dir>/agent-inbox`, where the inbox handler writes the items.
pub fn inbox_dir() -> PathBuf {
    runtime_dir().join("agent-inbox")
}

/// An item id (the file stem) is made of these only, so it cannot leave the directory.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The item of the JSON `v` of the file `<id>.json`; None unless it is an object with an `event`.
fn parse_item(id: &str, v: &Value) -> Option<Item> {
    Some(Item {
        id: id.to_string(),
        received: v.get("received").and_then(Value::as_f64).unwrap_or(0.0),
        event: v.as_object()?.get("event")?.clone(),
    })
}

/// `timeout` seconds after it was received, or once the
/// `reply` path is gone.
fn expired(item: &Item, now: f64) -> bool {
    let timeout = item.event.get("timeout").and_then(Value::as_f64);
    if timeout.is_some_and(|t| now > item.received + t) {
        return true;
    }
    item.reply()
        .is_some_and(|reply| fs::symlink_metadata(reply).is_err())
}

/// The content of the file `path` unless it is longer than `max` bytes.
fn read_limited(path: &Path, max: u64) -> Option<String> {
    let mut raw = String::new();
    fs::File::open(path)
        .ok()?
        .take(max + 1)
        .read_to_string(&mut raw)
        .ok()?;
    Some(raw).filter(|raw| raw.len() as u64 <= max)
}

/// Queued events at `now`, oldest first. Expired ones are deleted, files that are not items are
/// left alone.
pub fn list_at(dir: &Path, now: f64) -> Result<Vec<Item>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut items = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()).filter(|s| valid_id(s)) else {
            continue;
        };
        // not a FIFO (opening it would block) nor a symlink, and not a huge file
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let Some(item) = read_limited(&path, ITEM_MAX_BYTES)
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|v| parse_item(id, &v))
        else {
            continue;
        };
        if expired(&item, now) {
            let _ = fs::remove_file(&path);
        } else {
            items.push(item);
        }
    }
    items.sort_by(|a, b| a.received.total_cmp(&b.received).then_with(|| a.id.cmp(&b.id)));
    Ok(items)
}

/// Queued events, oldest first.
pub fn list(dir: &Path) -> Result<Vec<Item>> {
    list_at(dir, now())
}

/// Removes an item from the queue; false if there is no such item.
pub fn dismiss(dir: &Path, id: &str) -> Result<bool> {
    if !valid_id(id) {
        return Err(format!("invalid inbox id {id:?}").into());
    }
    match fs::remove_file(dir.join(format!("{id}.json"))) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Publishes `event` with `event-pub`.
pub fn post_event(event: &Value) -> std::result::Result<(), String> {
    publish("event-pub", event)
}

/// Runs `program <type of the event>` with the event on stdin; Err = what it said on stderr.
fn publish(program: &str, event: &Value) -> std::result::Result<(), String> {
    let kind = event
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "event without a type".to_string())?;
    let mut child = Command::new(program)
        .arg(kind)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // a program that exits early is reported by its status
        let _ = stdin.write_all(event.to_string().as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(match err.trim() {
        "" => format!("{program}: {}", out.status),
        err => err.to_string(),
    })
}

#[cfg(test)]
#[path = "client/client_test.rs"]
mod tests;
