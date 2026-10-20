//! Client of the event router's inbox: a few requests of raw HTTP/1.0 over a TcpStream
//! (the router closes the connection after every response).

use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

const TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_URL: &str = "http://127.0.0.1:7531";
const ORIGIN_MAX_BYTES: u64 = 64 * 1024;

/// One queued event with the router's bookkeeping.
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
        let mut raw = String::new();
        fs::File::open(path)
            .ok()?
            .take(ORIGIN_MAX_BYTES + 1)
            .read_to_string(&mut raw)
            .ok()?;
        if raw.len() as u64 > ORIGIN_MAX_BYTES {
            return None;
        }
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

/// A task for a new agent, sent to the router as a `task.new` event (which routes it to a repo
/// unless `repo` is given and starts the session).
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

    /// A string of `jj` (the router-side `jw info` of the cwd), "" if there is none.
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

    /// Where the event comes from: `repo:workspace` (no workspace for an empty one or
    /// `default`); without jj the `project_name`.
    pub fn place(&self) -> String {
        let (repo, workspace) = (self.jj("repo"), self.jj("workspace"));
        match (repo, workspace) {
            ("", _) => self.project_name(),
            (repo, "" | "default") => repo.to_string(),
            (repo, workspace) => format!("{repo}:{workspace}"),
        }
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

/// `host:port` of the router, from EVENT_ROUTER_URL.
pub fn base() -> Result<String> {
    let url = std::env::var("EVENT_ROUTER_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string());
    parse_base(&url)
}

fn parse_base(url: &str) -> Result<String> {
    let rest = url.strip_prefix("http://").unwrap_or("");
    let hostport = rest.strip_suffix('/').unwrap_or(rest);
    if hostport.is_empty() || hostport.contains('/') || !hostport.contains(':') {
        return Err(
            format!("unsupported EVENT_ROUTER_URL {url:?} (expected http://host:port)").into(),
        );
    }
    Ok(hostport.to_string())
}

/// First address of `base` that accepts a connection (`localhost` may resolve to ::1 and 127.0.0.1).
fn connect(base: &str) -> Result<TcpStream> {
    let mut last: Option<std::io::Error> = None;
    for addr in base.to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    match last {
        Some(e) => Err(e.into()),
        None => Err("router address does not resolve".into()),
    }
}

/// One HTTP/1.0 request, with an optional JSON body.
pub fn request(base: &str, method: &str, path: &str, body: Option<&str>) -> Result<(u16, Vec<u8>)> {
    let mut stream = connect(base)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    // one write: a server that answers after the head must not see the body arrive later
    let req = match body {
        Some(body) => format!(
            "{method} {path} HTTP/1.0\r\nHost: {base}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
        None => format!("{method} {path} HTTP/1.0\r\nHost: {base}\r\nContent-Length: 0\r\n\r\n"),
    };
    stream.write_all(req.as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    parse_response(&raw)
}

/// Sends `event` to the router (`POST /events`); only 202 is success.
pub fn post_event(base: &str, event: &Value) -> std::result::Result<(), String> {
    match request(base, "POST", "/events", Some(&event.to_string())) {
        Ok((202, _)) => Ok(()),
        Ok((status, body)) => Err(format!(
            "router: {status} {}",
            String::from_utf8_lossy(&body).trim()
        )),
        Err(e) => Err(format!("router: {e}")),
    }
}

fn parse_response(raw: &[u8]) -> Result<(u16, Vec<u8>)> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("malformed response from the router")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("malformed status line from the router")?;
    Ok((status, raw[split + 4..].to_vec()))
}

/// Queued events, oldest first.
pub fn list(base: &str) -> Result<Vec<Item>> {
    let (status, body) = request(base, "GET", "/inbox", None)?;
    if status != 200 {
        return Err(format!("GET /inbox: HTTP {status}").into());
    }
    let value: Value = serde_json::from_slice(&body)?;
    value
        .as_array()
        .ok_or("GET /inbox: not a JSON array")?
        .iter()
        .map(parse_item)
        .collect()
}

fn parse_item(v: &Value) -> Result<Item> {
    Ok(Item {
        id: v
            .get("id")
            .and_then(Value::as_str)
            .ok_or("GET /inbox: item without id")?
            .to_string(),
        received: v.get("received").and_then(Value::as_f64).unwrap_or(0.0),
        event: v.get("event").cloned().unwrap_or(Value::Null),
    })
}

/// Removes an item from the queue; false if the router does not know it.
pub fn dismiss(base: &str, id: &str) -> Result<bool> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("invalid inbox id {id:?}").into());
    }
    match request(base, "DELETE", &format!("/inbox/{id}"), None)?.0 {
        204 => Ok(true),
        404 => Ok(false),
        status => Err(format!("DELETE /inbox/{id}: HTTP {status}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    /// Serves one canned response; the thread returns the request it received.
    fn stub(response: &str) -> (String, JoinHandle<String>) {
        let response = response.to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let handle = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 512];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = conn.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
            }
            conn.write_all(response.as_bytes()).unwrap();
            String::from_utf8(req).unwrap()
        });
        (base, handle)
    }

    #[test]
    fn list_parses_items() {
        let body = r#"[{"id":"a1","received":100.5,"event":{"type":"sudo.password_request","title":"sudo ls","reply":"/r/reply"}},{"id":"b2","received":200.0,"event":{"type":"claude.question"}}]"#;
        let (base, server) = stub(&format!(
            "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{body}"
        ));
        let items = list(&base).unwrap();
        assert!(server
            .join()
            .unwrap()
            .starts_with("GET /inbox HTTP/1.0\r\n"));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "a1");
        assert_eq!(items[0].received, 100.5);
        assert_eq!(items[0].kind(), "sudo.password_request");
        assert_eq!(items[0].title(), "sudo ls");
        assert_eq!(items[0].reply(), Some("/r/reply"));
        assert_eq!(items[1].reply(), None);
        assert_eq!(items[1].project(), "");
    }

    #[test]
    fn list_rejects_error_status_and_bad_body() {
        let (base, server) = stub("HTTP/1.0 500 Internal Server Error\r\n\r\nboom");
        assert!(list(&base).is_err());
        server.join().unwrap();
        let (base, server) = stub("HTTP/1.0 200 OK\r\n\r\n{\"not\":\"an array\"}");
        assert!(list(&base).is_err());
        server.join().unwrap();
    }

    #[test]
    fn list_fails_when_nothing_listens() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        drop(listener);
        assert!(list(&base).is_err());
    }

    #[test]
    fn dismiss_maps_status_codes() {
        let (base, server) = stub("HTTP/1.0 204 No Content\r\n\r\n");
        assert!(dismiss(&base, "a1").unwrap());
        assert!(server
            .join()
            .unwrap()
            .starts_with("DELETE /inbox/a1 HTTP/1.0\r\n"));
        let (base, server) = stub("HTTP/1.0 404 Not Found\r\n\r\n");
        assert!(!dismiss(&base, "a1").unwrap());
        server.join().unwrap();
        let (base, server) = stub("HTTP/1.0 500 Internal Server Error\r\n\r\n");
        assert!(dismiss(&base, "a1").is_err());
        server.join().unwrap();
    }

    #[test]
    fn post_event_sends_json_and_accepts_only_202() {
        let (base, server) = stub("HTTP/1.0 202 Accepted\r\n\r\n");
        let event = serde_json::json!({"type": "task.new", "prompt": "x"});
        assert_eq!(post_event(&base, &event), Ok(()));
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /events HTTP/1.0\r\n"), "{req}");
        assert!(req.contains(&format!("Host: {base}\r\n")), "{req}");
        assert!(req.contains("Content-Type: application/json\r\n"), "{req}");
        let len = event.to_string().len();
        assert!(req.contains(&format!("Content-Length: {len}\r\n")), "{req}");
        for status in ["200 OK", "204 No Content"] {
            let (base, server) = stub(&format!("HTTP/1.0 {status}\r\n\r\nbody"));
            assert!(post_event(&base, &event).is_err(), "{status}");
            server.join().unwrap();
        }
        let (base, server) = stub("HTTP/1.0 500 Internal Server Error\r\n\r\nboom");
        let err = post_event(&base, &event).unwrap_err();
        assert!(err.starts_with("router: 500"), "{err}");
        assert!(err.contains("boom"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn task_event_leaves_out_what_is_empty() {
        assert_eq!(
            task_event(&Task::default()),
            serde_json::json!({"type": "task.new"})
        );
        let full = Task {
            prompt: "fix".into(),
            images: vec!["/a.png".into()],
            repo: Some("/r".into()),
            id: Some("c1".into()),
        };
        assert_eq!(
            task_event(&full),
            serde_json::json!({"type": "task.new", "prompt": "fix", "images": ["/a.png"], "repo": "/r", "id": "c1"})
        );
        let partial = Task {
            repo: Some("/r".into()),
            ..Task::default()
        };
        assert_eq!(
            task_event(&partial),
            serde_json::json!({"type": "task.new", "repo": "/r"})
        );
    }

    #[test]
    fn dismiss_rejects_odd_ids() {
        assert!(dismiss("127.0.0.1:1", "../x").is_err());
        assert!(dismiss("127.0.0.1:1", "").is_err());
    }

    #[test]
    fn base_accepts_only_http_host_port() {
        assert_eq!(
            parse_base("http://127.0.0.1:7531").unwrap(),
            "127.0.0.1:7531"
        );
        assert_eq!(parse_base("http://localhost:80/").unwrap(), "localhost:80");
        assert!(parse_base("https://127.0.0.1:7531").is_err());
        assert!(parse_base("http://127.0.0.1:7531/x").is_err());
        assert!(parse_base("http://localhost").is_err());
        assert!(parse_base("").is_err());
    }

    #[test]
    fn questions_are_read_from_tool_input() {
        let event = serde_json::json!({"type": "claude.question", "tool_input": {"questions": [
            {"question": "Which?", "multiSelect": true, "options": [
                {"label": "A", "description": "first"}, {"label": "B"}, {"description": "no label"}]},
            {"header": "no question text"},
            {"question": "Sure?", "header": "Confirm"}
        ]}});
        let item = Item {
            id: "q".into(),
            received: 0.0,
            event,
        };
        assert_eq!(
            item.questions(),
            vec![
                Question {
                    question: "Which?".into(),
                    header: "".into(),
                    multi: true,
                    labels: vec!["A".into(), "B".into()],
                    descriptions: vec!["first".into(), "".into()],
                },
                Question {
                    question: "Sure?".into(),
                    header: "Confirm".into(),
                    multi: false,
                    labels: vec![],
                    descriptions: vec![],
                },
            ]
        );
        let none = Item {
            id: "n".into(),
            received: 0.0,
            event: Value::Null,
        };
        assert!(none.questions().is_empty());
    }

    #[test]
    fn project_name_prefers_workspace_repo_then_cwd_project_host() {
        let name = |event: Value| {
            Item {
                id: "a".into(),
                received: 0.0,
                event,
            }
            .project_name()
        };
        use serde_json::json;
        assert_eq!(
            name(json!({"cwd": "/home/u/projects/infra.agents/s-1012-002032-2abc"})),
            "infra"
        );
        assert_eq!(name(json!({"cwd": "/home/u/projects/infra/"})), "infra");
        // a bare `.agents` component names nothing
        assert_eq!(name(json!({"cwd": "/x/.agents/w1"})), "w1");
        assert_eq!(name(json!({"cwd": "/", "project": "p", "host": "h"})), "p");
        assert_eq!(name(json!({"project": "", "host": "h"})), "h");
        assert_eq!(name(Value::Null), "");
        // the jj repo wins over everything else
        assert_eq!(
            name(json!({"cwd": "/home/u/projects/infra.agents/s-1", "jj": {"repo": "real"}})),
            "real"
        );
        assert_eq!(name(json!({"cwd": "/x/y", "jj": {"repo": ""}})), "y");
    }

    #[test]
    fn session_id_is_empty_without_one() {
        use serde_json::json;
        let id = |event: Value| {
            Item {
                id: "a".into(),
                received: 0.0,
                event,
            }
            .session_id()
            .to_string()
        };
        assert_eq!(id(json!({"session_id": "abc-123"})), "abc-123");
        assert_eq!(id(json!({"cwd": "/x"})), "");
        assert_eq!(id(Value::Null), "");
    }

    #[test]
    fn place_is_repo_and_non_default_workspace() {
        use serde_json::json;
        let item = |event: Value| Item {
            id: "a".into(),
            received: 0.0,
            event,
        };
        let jj = |workspace: Value| {
            item(json!({"cwd": "/x/y", "jj": {"repo": "r", "workspace": workspace}}))
        };
        assert_eq!(jj(json!("s-1")).place(), "r:s-1");
        assert_eq!(jj(json!("default")).place(), "r");
        assert_eq!(jj(json!("")).place(), "r");
        assert_eq!(jj(Value::Null).place(), "r");
        assert_eq!(item(json!({"cwd": "/x/y"})).place(), "y");
        assert_eq!(item(Value::Null).place(), "");
    }

    #[test]
    fn jj_line_follows_the_statusline_format() {
        use serde_json::json;
        let line = |jj: Value| {
            Item {
                id: "a".into(),
                received: 0.0,
                event: json!({"jj": jj}),
            }
            .jj_line()
        };
        assert_eq!(
            line(
                json!({"repo": "r", "workspace": "w", "subdir": "pkgs/x", "change": "abc",
                "stack": {"index": 2, "size": 3}})
            ),
            Some("r/pkgs/x ws:w @abc 2/3".to_string())
        );
        assert_eq!(
            line(
                json!({"repo": "r", "workspace": "default", "subdir": "", "change": null,
                "stack": null})
            ),
            Some("r ws:default".to_string())
        );
        assert_eq!(line(json!({"workspace": "w"})), None);
        assert_eq!(line(Value::Null), None);
    }

    #[test]
    fn age_and_clean() {
        let item = Item {
            id: "a".into(),
            received: 1000.0,
            event: Value::Null,
        };
        assert_eq!(item.age(1005.0), "5s");
        assert_eq!(item.age(1000.0 + 180.0), "3m");
        assert_eq!(item.age(1000.0 + 7200.0), "2h");
        assert_eq!(item.age(900.0), "0s");
        assert_eq!(clean("a\x1b[31m\tb\nc"), "a [31m b c");
    }

    /// Fresh `<temp>/<test>/claude-origin`.
    fn origin_dir(test: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("agent-inbox-test-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("claude-origin");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `<dir>/<name>` holding `text`.
    fn write_origin(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
        fs::write(dir.join(name), text).unwrap();
        dir.join(name)
    }

    #[test]
    fn origin_loads_a_merge_request_and_a_task() {
        let mr = r#"{"kind":"review","iid":7,"target":"MergeRequest","title":"T","url":"https://gitlab/x/-/merge_requests/7","description":"d"}"#;
        let dir = origin_dir("origin-mr");
        let path = write_origin(&dir, "a.json", mr);
        assert_eq!(
            Origin::load(&path, &dir),
            Some(Origin {
                kind: "review".into(),
                iid: Some(7),
                target: "MergeRequest".into(),
                title: "T".into(),
                url: "https://gitlab/x/-/merge_requests/7".into(),
                description: "d".into(),
                prompt: "".into(),
            })
        );
        let text = r#"{"kind":"task","title":"t","prompt":"p"}"#;
        let path = write_origin(&dir, "b.json", text);
        let task = Origin::load(&path, &dir).unwrap();
        assert_eq!(
            (task.iid, task.url.as_str(), task.prompt.as_str()),
            (None, "", "p")
        );
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn origin_rejects_foreign_oversized_and_malformed_files() {
        let dir = origin_dir("origin-bad");
        let load = |path: &Path| Origin::load(path, &dir);
        let ok = write_origin(&dir, "a.json", r#"{"title":"T"}"#);
        assert!(load(&ok).is_some());
        // outside the directory, also by way of `..` and a subdirectory
        let outside = write_origin(dir.parent().unwrap(), "b.json", r#"{"title":"T"}"#);
        assert!(load(&outside).is_none());
        assert!(load(&dir.join("../b.json")).is_none());
        fs::create_dir(dir.join("sub")).unwrap();
        let nested = write_origin(&dir.join("sub"), "c.json", r#"{"title":"T"}"#);
        assert!(load(&nested).is_none());
        // a directory, a missing file, over 64 KB, bad JSON, not an object
        assert!(load(&dir.join("sub")).is_none());
        assert!(load(&dir.join("gone.json")).is_none());
        let big = format!(
            r#"{{"title":"T","description":"{}"}}"#,
            "x".repeat(64 * 1024)
        );
        assert!(load(&write_origin(&dir, "big.json", &big)).is_none());
        assert!(load(&write_origin(&dir, "junk.json", "{not json")).is_none());
        assert!(load(&write_origin(&dir, "list.json", "[]")).is_none());
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn origin_label_names_the_merge_request_issue_or_task() {
        let label = |target: &str, iid: Option<u64>, title: &str| {
            origin_label(&Origin {
                target: target.into(),
                iid,
                title: title.into(),
                ..Origin::default()
            })
        };
        assert_eq!(label("MergeRequest", Some(7), "T"), Some("!7 T".into()));
        assert_eq!(label("Issue", Some(12), "T"), Some("#12 T".into()));
        assert_eq!(label("", None, "do it"), Some("do it".into()));
        // without a number only the title is left
        assert_eq!(label("MergeRequest", None, "T"), Some("T".into()));
        assert_eq!(label("MergeRequest", Some(7), ""), Some("!7".into()));
        assert_eq!(label("", None, ""), None);
        assert_eq!(
            label("Issue", Some(1), "a\x1b[31m\nb"),
            Some("#1 a [31m b".into())
        );
    }

    #[test]
    fn origin_file_is_empty_without_one() {
        let item = |event: Value| Item {
            id: "a".into(),
            received: 0.0,
            event,
        };
        assert_eq!(
            item(serde_json::json!({"origin_file": "/o/1"})).origin_file(),
            "/o/1"
        );
        assert_eq!(item(serde_json::json!({})).origin_file(), "");
    }
}
