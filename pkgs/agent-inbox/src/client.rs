//! Client of the event router's inbox: a few requests of raw HTTP/1.0 over a TcpStream
//! (the router closes the connection after every response).

use std::error::Error;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

const TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_URL: &str = "http://127.0.0.1:7531";

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

    pub fn prompt(&self) -> &str {
        self.field("prompt")
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
        let secs = (now - self.received).max(0.0) as u64;
        match secs {
            0..=59 => format!("{secs}s"),
            60..=3599 => format!("{}m", secs / 60),
            _ => format!("{}h", secs / 3600),
        }
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

pub fn request(base: &str, method: &str, path: &str) -> Result<(u16, Vec<u8>)> {
    let mut stream = connect(base)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    write!(
        stream,
        "{method} {path} HTTP/1.0\r\nHost: {base}\r\nContent-Length: 0\r\n\r\n"
    )?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    parse_response(&raw)
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
    let (status, body) = request(base, "GET", "/inbox")?;
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
    match request(base, "DELETE", &format!("/inbox/{id}"))?.0 {
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
}
