use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const TASKS: &str = "# Tasks
- [ ] wireguard container forwards to mullvad @est(2h) #self-hosting
- [ ] kubernetes cluster runs wireguard @est(4h) @due(2026-10-25) #self-hosting
- [ ] sunshine on headless vm @est(3d) #self-hosting
- [x] old done thing @est(1h)
- [x] migrate secrets @est(4h) @week(2026-w41)
- [x] clean up longhorn @est(1h) @week(2026-w41)
- [ ] write a blog post @est(2h) #fun @drop(2026-w41) @week(2026-w42)
- [x] read ML slides @est(2h) @due(2026-10-14) #university @week(2026-w42)
- [ ] stachiewicza wardrobe  @est(4h) @drop(2026-w41) #organization @week(2026-w42)
notes here
";

const TIMEOUT: Duration = Duration::from_secs(10);

struct Env {
    dir: PathBuf,
}

impl Env {
    fn fixture() -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("tm-test-serve-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let e = Env { dir };
        e.write("tasks.md", TASKS);
        e
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.dir.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
    }

    fn append(&self, rel: &str, line: &str) {
        let cur = self.read(rel);
        self.write(rel, &format!("{cur}{line}\n"));
    }

    /// Runs the CLI against this dir and returns stdout, asserting exit 0.
    fn cli(&self, args: &[&str]) -> String {
        let o = Command::new(env!("CARGO_BIN_EXE_tm"))
            .args(args)
            .env("TM_DIR", &self.dir)
            .env("TM_TODAY", "2026-10-18")
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(0),
            "cli {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    /// Both dirs hold the same tasks.md.
    fn same_as(&self, other: &Env) {
        assert_eq!(self.read("tasks.md"), other.read("tasks.md"));
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A running `tm serve`, killed on drop.
struct Server {
    env: Env,
    child: Child,
    addr: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn start() -> Server {
        let env = Env::fixture();
        let mut child = Command::new(env!("CARGO_BIN_EXE_tm"))
            .args(["serve", "--listen", "127.0.0.1:0"])
            .env("TM_DIR", &env.dir)
            .env("TM_TODAY", "2026-10-18")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stderr);
            let mut first = String::new();
            let _ = r.read_line(&mut first);
            let _ = tx.send(first);
            // keep draining so the server never blocks on a full pipe
            let mut sink = String::new();
            while r.read_line(&mut sink).map(|n| n > 0).unwrap_or(false) {
                sink.clear();
            }
        });
        // Build the guard first so the child is killed if we panic below.
        let mut s = Server {
            env,
            child,
            addr: String::new(),
        };
        let line = rx
            .recv_timeout(TIMEOUT)
            .unwrap_or_else(|_| panic!("server printed no 'listening on' line within {TIMEOUT:?}"));
        let line = line.trim_end();
        let port = line
            .strip_prefix("tm: listening on http://127.0.0.1:")
            .unwrap_or_else(|| panic!("unexpected first stderr line: {line:?}"));
        assert!(
            !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
            "bad port in {line:?}"
        );
        s.addr = format!("127.0.0.1:{port}");
        s
    }

    fn request(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Resp {
        http(&self.addr, method, path, headers, body)
    }

    fn post_mcp(&self, body: &str) -> Resp {
        self.request(
            "POST",
            "/mcp",
            &[("Content-Type", "application/json")],
            body.as_bytes(),
        )
    }

    /// Sends a JSON-RPC request with id 1 and returns the parsed body (status 200 expected).
    fn rpc(&self, method: &str, params: Value) -> Value {
        let r = self
            .post_mcp(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string());
        assert_eq!(r.status, 200, "body: {}", r.text());
        r.json()
    }

    /// Calls a tool; returns (isError, content texts).
    fn call(&self, name: &str, args: Value) -> (bool, Vec<String>) {
        self.call_params(json!({"name":name,"arguments":args}))
    }

    fn call_params(&self, params: Value) -> (bool, Vec<String>) {
        let v = self.rpc("tools/call", params);
        assert!(v.get("error").is_none(), "unexpected rpc error: {v}");
        let res = &v["result"];
        let is_error = res["isError"]
            .as_bool()
            .unwrap_or_else(|| panic!("no isError in {v}"));
        let texts = res["content"]
            .as_array()
            .unwrap_or_else(|| panic!("no content in {v}"))
            .iter()
            .map(|b| {
                assert_eq!(b["type"], "text", "block: {b}");
                b["text"].as_str().unwrap().to_string()
            })
            .collect();
        (is_error, texts)
    }
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("bad json ({e}): {}", self.text()))
    }
}

fn decode_chunked(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let eol = data
            .windows(2)
            .position(|w| w == b"\r\n")
            .expect("bad chunked body: no size line");
        let size_line = std::str::from_utf8(&data[..eol]).unwrap();
        let size_str = size_line.split(';').next().unwrap().trim();
        let size = usize::from_str_radix(size_str, 16).expect("bad chunk size");
        data = &data[eol + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

fn connect(addr: &str) -> TcpStream {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(TIMEOUT)).unwrap();
    s.set_write_timeout(Some(TIMEOUT)).unwrap();
    s
}

fn http(addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Resp {
    let mut s = connect(addr);
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    // The server may answer (e.g. 413) and close before reading everything.
    let _ = s.write_all(req.as_bytes());
    let _ = s.write_all(body);
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) if !raw.is_empty() && e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("read error: {e} (got {} bytes)", raw.len()),
        }
    }
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no header end in {:?}", String::from_utf8_lossy(&raw)));
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .expect("bad status line");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let mut body = raw[split + 4..].to_vec();
    let chunked = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked")
    });
    if chunked {
        body = decode_chunked(&body);
    }
    Resp {
        status,
        headers,
        body,
    }
}

fn j(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("bad json ({e}): {s}"))
}

fn assert_rpc_error(v: &Value, code: i64) {
    assert_eq!(v["jsonrpc"], "2.0", "{v}");
    assert_eq!(v["error"]["code"], code, "{v}");
    assert!(v.get("result").is_none(), "{v}");
}

const ADD_LINE12: &str = "- [ ] i sleep better #health/sleep @created(2026-10-18)";

// 1
#[test]
fn startup_prints_listening_line() {
    let s = Server::start();
    assert!(s.addr.starts_with("127.0.0.1:"));
}

#[test]
fn startup_fails_without_tasks_dir() {
    let o = Command::new(env!("CARGO_BIN_EXE_tm"))
        .args(["serve", "--listen", "127.0.0.1:0"])
        .env("TM_DIR", "/nonexistent")
        .env("TM_TODAY", "2026-10-18")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr).trim_end(),
        "tm: tasks dir not found: /nonexistent"
    );
}

// 2
#[test]
fn initialize_negotiates_version() {
    let s = Server::start();
    let v = s.rpc("initialize", json!({"protocolVersion":"2025-06-18"}));
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 1);
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(v["result"]["serverInfo"]["name"], "tm");
    assert!(v["result"]["capabilities"]["tools"].is_object(), "{v}");

    let v = s.rpc("initialize", json!({"protocolVersion":"1999-01-01"}));
    assert_eq!(v["result"]["protocolVersion"], "2025-11-25");
}

#[test]
fn initialize_has_no_session_header() {
    let s = Server::start();
    let r = s.post_mcp(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}})
            .to_string(),
    );
    assert_eq!(r.status, 200);
    assert!(r.header("Mcp-Session-Id").is_none());
    assert!(r
        .header("Content-Type")
        .unwrap_or("")
        .starts_with("application/json"));
}

// 3
#[test]
fn requests_without_id_get_202() {
    let s = Server::start();
    let r = s.post_mcp(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    assert_eq!(r.status, 202);
    assert!(r.body.is_empty(), "{}", r.text());
    let r = s.post_mcp(r#"{"method":"x","id":null}"#);
    assert_eq!(r.status, 202);
    assert!(r.body.is_empty());
}

// 4
#[test]
fn tools_list() {
    let s = Server::start();
    let r = s.post_mcp(r#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#);
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["id"], "a");
    let tools = v["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["add", "done", "list", "week", "take", "close", "lint"]
    );
    for t in tools {
        let name = t["name"].as_str().unwrap();
        assert!(
            !t["description"].as_str().unwrap_or("").is_empty(),
            "{name}: empty description"
        );
        let schema = t["inputSchema"].as_object().unwrap();
        assert!(!schema.contains_key("description"), "{name}");
        for (k, p) in schema["properties"].as_object().unwrap() {
            assert!(
                p["type"].is_string(),
                "{name}.{k}: type must be a plain string: {p}"
            );
        }
        assert_eq!(t["inputSchema"]["type"], "object", "{name}");
        assert_eq!(
            t["inputSchema"]["additionalProperties"],
            json!(false),
            "{name}"
        );
        let ro = t["annotations"]["readOnlyHint"].as_bool().unwrap();
        assert_eq!(ro, ["list", "week", "lint"].contains(&name), "{name}");
        let required = &t["inputSchema"]["required"];
        let expect = match name {
            "add" => Some(json!(["text"])),
            "done" | "take" => Some(json!(["refs"])),
            _ => None,
        };
        match expect {
            Some(e) => assert_eq!(required, &e, "{name}"),
            None => assert!(
                required.is_null() || required == &json!([]),
                "{name}: {required}"
            ),
        }
    }
}

#[test]
fn tool_descriptions_match_subcommand_about() {
    let s = Server::start();
    let v = s.rpc("tools/list", json!({}));
    let tools = v["result"]["tools"].as_array().unwrap();
    let expect = [
        (
            "add",
            "Add a task to tasks.md (text without the checkbox). Returns the new line as N: <line>.",
        ),
        (
            "done",
            "Mark tasks done. Returns each changed line as N: <line>.",
        ),
        ("list", "List tasks. Returns JSON."),
        ("week", "Summarize a week (default current). Returns JSON."),
        ("take", "Plan tasks for a week (sets @week)."),
        (
            "close",
            "Return a finished week's open tasks to the backlog with @drop (default latest finished week).",
        ),
        ("lint", "Check tasks.md for format problems."),
    ];
    for (name, d) in expect {
        let t = tools.iter().find(|t| t["name"] == name).unwrap();
        assert_eq!(t["description"], d, "{name}");
    }
}

#[test]
fn lint_schema_has_empty_properties() {
    let s = Server::start();
    let v = s.rpc("tools/list", json!({}));
    let tools = v["result"]["tools"].as_array().unwrap();
    let lint = tools.iter().find(|t| t["name"] == "lint").unwrap();
    assert_eq!(lint["inputSchema"]["properties"], json!({}));
}

#[test]
fn optional_strings_are_plain_string_type() {
    let s = Server::start();
    let v = s.rpc("tools/list", json!({}));
    let tools = v["result"]["tools"].as_array().unwrap();
    let list = tools.iter().find(|t| t["name"] == "list").unwrap();
    assert_eq!(list["inputSchema"]["properties"]["in"]["type"], "string");
    let week = tools.iter().find(|t| t["name"] == "week").unwrap();
    assert_eq!(week["inputSchema"]["properties"]["week"]["type"], "string");
}

#[test]
fn list_schema_exposes_in_not_scope() {
    let s = Server::start();
    let v = s.rpc("tools/list", json!({}));
    let tools = v["result"]["tools"].as_array().unwrap();
    let list = tools.iter().find(|t| t["name"] == "list").unwrap();
    let props = list["inputSchema"]["properties"].as_object().unwrap();
    assert!(props.contains_key("in"), "{props:?}");
    assert!(!props.contains_key("scope"), "{props:?}");
    for t in tools {
        let schema = t["inputSchema"].as_object().unwrap();
        assert!(!schema.contains_key("$schema"), "{}", t["name"]);
        assert!(!schema.contains_key("title"), "{}", t["name"]);
    }
}

#[test]
fn lint_rejects_unknown_key_with_message() {
    let s = Server::start();
    let (err, content) = s.call("lint", json!({"x":1}));
    assert!(err);
    assert!(content[0].starts_with("tm: "), "{content:?}");
    assert!(content[0].contains("unknown field"), "{content:?}");
}

// 5
#[test]
fn add_appends_line() {
    let s = Server::start();
    let (err, content) = s.call("add", json!({"text":"i sleep better #health/sleep"}));
    assert!(!err);
    assert_eq!(content, [format!("12: {ADD_LINE12}\n")]);
    let lines: Vec<String> = s.env.read("tasks.md").lines().map(String::from).collect();
    assert_eq!(lines.len(), 12);
    assert_eq!(lines[11], ADD_LINE12);
}

// 6
#[test]
fn add_invalid_est_is_tool_error() {
    let s = Server::start();
    let (err, content) = s.call("add", json!({"text":"x @est(2x)"}));
    assert!(err);
    assert_eq!(content, ["tm: invalid @est value: 2x"]);
    assert_eq!(s.env.read("tasks.md"), TASKS);
}

// 7
#[test]
fn done_ambiguous_ref() {
    let s = Server::start();
    let (err, content) = s.call("done", json!({"refs":["wireguard"]}));
    assert!(err);
    assert!(
        content[0].starts_with("tm: ambiguous \"wireguard\", 2 matches:"),
        "{}",
        content[0]
    );
    assert!(content[0].contains("\n2: - [ ] wireguard container"));
    assert!(content[0].contains("\n3: - [ ] kubernetes cluster"));
    assert_eq!(s.env.read("tasks.md"), TASKS);
}

// 8
#[test]
fn list_and_week_match_cli() {
    let s = Server::start();
    let twin = Env::fixture();

    let (err, content) = s.call("list", json!({"in":"backlog","open":true}));
    assert!(!err);
    let cli = twin.cli(&["list", "--in", "backlog", "--open", "--json"]);
    assert_eq!(j(&content[0]), j(&cli));

    let (err, content) = s.call("week", json!({}));
    assert!(!err);
    assert_eq!(j(&content[0]), j(&twin.cli(&["week", "--json"])));

    let (err, content) = s.call("week", json!({"week":"2026-w41"}));
    assert!(!err);
    assert_eq!(j(&content[0])["status"], "past", "{}", content[0]);
}

// 9
#[test]
fn take_matches_cli() {
    let s = Server::start();
    let twin = Env::fixture();
    let (err, content) = s.call("take", json!({"refs":["kubernetes"]}));
    assert!(!err, "{content:?}");
    let cli = twin.cli(&["take", "kubernetes"]);
    assert_eq!(content[0], cli);
    s.env.same_as(&twin);
}

// 10
#[test]
fn close_then_nothing_to_close() {
    let s = Server::start();
    let (err, content) = s.call("close", json!({}));
    assert!(!err);
    assert_eq!(
        content,
        ["8: - [ ] write a blog post @est(2h) #fun @drop(2026-w42)\n\
          10: - [ ] stachiewicza wardrobe  @est(4h) @drop(2026-w42) #organization\n"]
    );
    let after = s.env.read("tasks.md");
    let (err, content) = s.call("close", json!({}));
    assert!(!err);
    assert_eq!(content, ["nothing to close in 2026-w42\n"]);
    assert_eq!(s.env.read("tasks.md"), after);
}

// 11
#[test]
fn lint_clean_and_dirty() {
    let s = Server::start();
    let (err, content) = s.call("lint", json!({}));
    assert!(!err);
    assert_eq!(content, [""]);

    s.env.append("tasks.md", "- [ ] x @project(foo)");
    let (err, content) = s.call("lint", json!({}));
    assert!(err);
    assert_eq!(content, ["12: unknown token @project\n"]);
}

// 12
#[test]
fn warnings_in_separate_block() {
    let s = Server::start();
    s.env.append("tasks.md", "- [ ] x @project(foo)");
    let (err, content) = s.call("list", json!({}));
    assert!(!err);
    assert_eq!(content.len(), 2, "{content:?}");
    assert_eq!(j(&content[0]), j(&s.env.cli(&["list", "--json"])));
    assert_eq!(content[1], "tm: skipping line 12: unknown token @project");
}

// 13
#[test]
fn argument_errors() {
    let s = Server::start();
    let cases = [
        json!({"name":"add","arguments":{}}),
        json!({"name":"add","arguments":{"text":5}}),
        json!({"name":"add","arguments":{"text":null}}),
        json!({"name":"done","arguments":{"refs":"x"}}),
        json!({"name":"list","arguments":{"bogus":1}}),
        json!({"name":"lint","arguments":{"x":1}}),
        json!({"name":"add","arguments":"x"}),
        json!({"name":"add","arguments":["x"]}),
        json!({"name":"lint","arguments":[]}),
    ];
    for p in cases {
        let (err, content) = s.call_params(p.clone());
        assert!(err, "{p}");
        assert!(content[0].starts_with("tm: "), "{p}: {content:?}");
    }
    assert_eq!(s.env.read("tasks.md"), TASKS);
}

// 14
#[test]
fn domain_errors() {
    let s = Server::start();
    let cases = [
        ("done", json!({"refs":[]}), "tm: refs must not be empty"),
        ("take", json!({"refs":[]}), "tm: refs must not be empty"),
        (
            "list",
            json!({"open":true,"done":true}),
            "tm: open and done cannot both be true",
        ),
        (
            "add",
            json!({"text":"  "}),
            "tm: the text must not be empty",
        ),
        (
            "add",
            json!({"text":"a\nb"}),
            "tm: text must be a single line",
        ),
    ];
    for (name, args, msg) in cases {
        let (err, content) = s.call(name, args.clone());
        assert!(err, "{name} {args}");
        assert_eq!(content, [msg], "{name} {args}");
    }
    assert_eq!(s.env.read("tasks.md"), TASKS);
}

#[test]
fn null_valued_keys_and_error_messages() {
    let s = Server::start();
    let (err, c) = s.call("list", json!({"tags": null, "open": null}));
    assert!(!err, "{c:?}");
    let (err, c) = s.call("add", json!({"text": null}));
    assert!(err);
    assert!(c[0].contains("missing field"), "{c:?}");
    let (err, c) = s.call("lint", json!({"x":1}));
    assert!(err);
    assert!(c[0].contains("unknown field"), "{c:?}");
    let (err, c) = s.call("list", json!([]));
    assert!(err);
    assert!(c[0].contains("arguments must be an object"), "{c:?}");
}

// 15
#[test]
fn null_counts_as_absent() {
    let s = Server::start();
    let a = s.call("week", json!({"week":null}));
    let b = s.call("week", json!({}));
    assert!(!a.0);
    assert_eq!(a, b);
}

// 16
#[test]
fn jsonrpc_errors() {
    let s = Server::start();
    let r = s.post_mcp(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"raw","arguments":{}}}"#,
    );
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_rpc_error(&v, -32602);
    assert_eq!(v["id"], 3);

    let v = s.rpc("tools/call", json!({"name":"raw","arguments":[]}));
    assert_rpc_error(&v, -32602);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Unknown tool: raw"),
        "{v}"
    );

    let v = s.rpc("foo/bar", json!({}));
    assert_rpc_error(&v, -32601);

    let v = s.rpc("ping", json!({}));
    assert_eq!(v["result"], json!({}));
}

// 17
#[test]
fn malformed_bodies() {
    let s = Server::start();
    let r = s.post_mcp("{");
    assert_eq!(r.status, 400);
    let v = r.json();
    assert_rpc_error(&v, -32700);
    assert!(v["id"].is_null());

    let r = s.post_mcp("[]");
    assert_eq!(r.status, 400);
    let v = r.json();
    assert_rpc_error(&v, -32600);
    assert!(v["id"].is_null());

    let r = s.post_mcp(r#"{"id":1}"#);
    assert_eq!(r.status, 400);
    let v = r.json();
    assert_rpc_error(&v, -32600);
    assert_eq!(v["id"], 1);
}

// 18
#[test]
fn routing() {
    let s = Server::start();
    for m in ["GET", "DELETE"] {
        let r = s.request(m, "/mcp", &[], b"");
        assert_eq!(r.status, 405, "{m} /mcp");
        assert_eq!(r.header("Allow"), Some("POST"));
    }
    for p in ["/nope", "/api/done"] {
        let r = s.request("POST", p, &[], b"x");
        assert_eq!(r.status, 404, "{p}");
    }
    let r = s.request(
        "POST",
        "/mcp?x=1",
        &[("Content-Type", "application/json")],
        br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["result"], json!({}));
}

// 19
#[test]
fn origin_is_rejected() {
    let s = Server::start();
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"add","arguments":{"text":"evil"}}})
    .to_string();
    let r = s.request(
        "POST",
        "/mcp",
        &[
            ("Content-Type", "application/json"),
            ("Origin", "http://evil.example"),
        ],
        body.as_bytes(),
    );
    assert_eq!(r.status, 403);
    assert_eq!(s.env.read("tasks.md"), TASKS);

    let r = s.request(
        "POST",
        "/api/add",
        &[
            ("Origin", "null"),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        b"text=x",
    );
    assert_eq!(r.status, 403);
    assert!(r
        .header("Content-Type")
        .unwrap_or("")
        .starts_with("text/plain"));
    assert_eq!(r.text(), "tm: cross-origin requests are not allowed");
    assert_eq!(s.env.read("tasks.md"), TASKS);
}

// 20
#[test]
fn oversized_body_is_413() {
    let s = Server::start();
    let body = vec![b'a'; 65537];
    let r = s.request("POST", "/mcp", &[], &body);
    assert_eq!(r.status, 413);
    let r = s.request("POST", "/api/add", &[], &body);
    assert_eq!(r.status, 413);
    assert_eq!(s.env.read("tasks.md"), TASKS);
}

#[test]
fn oversized_chunked_body_is_413() {
    let s = Server::start();
    let mut c = connect(&s.addr);
    let head = format!(
        "POST /mcp HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        s.addr
    );
    // The server may answer 413 and close before reading everything.
    let _ = c.write_all(head.as_bytes());
    let chunk = vec![b'a'; 8192];
    for _ in 0..9 {
        let _ = c.write_all(format!("{:x}\r\n", chunk.len()).as_bytes());
        let _ = c.write_all(&chunk);
        let _ = c.write_all(b"\r\n");
    }
    let _ = c.write_all(b"0\r\n\r\n");
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match c.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) if !raw.is_empty() && e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("read error: {e} (got {} bytes)", raw.len()),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    assert!(text.starts_with("HTTP/1.1 413"), "{text:?}");
    assert_eq!(s.env.read("tasks.md"), TASKS);
}

// 21
#[test]
fn rest_add() {
    let s = Server::start();
    let r = s.request("POST", "/api/add", &[], b"buy milk #home\n");
    assert_eq!(r.status, 200);
    assert!(r
        .header("Content-Type")
        .unwrap_or("")
        .starts_with("text/plain"));
    assert_eq!(r.text(), "12: - [ ] buy milk #home @created(2026-10-18)\n");
    let lines: Vec<String> = s.env.read("tasks.md").lines().map(String::from).collect();
    assert_eq!(lines[11], "- [ ] buy milk #home @created(2026-10-18)");

    let before = s.env.read("tasks.md");
    let r = s.request("POST", "/api/add", &[], b"x @est(2x)");
    assert_eq!(r.status, 400);
    assert_eq!(r.text(), "tm: invalid @est value: 2x");

    let r = s.request("POST", "/api/add", &[], b"");
    assert_eq!(r.status, 400);
    assert_eq!(r.text(), "tm: the text must not be empty");
    assert_eq!(s.env.read("tasks.md"), before);

    let r = s.request("GET", "/api/add", &[], b"");
    assert_eq!(r.status, 405);
}

// 22
#[test]
fn external_edits_are_seen() {
    let s = Server::start();
    let (_, before) = s.call("list", json!({}));
    assert!(!before[0].contains("edited externally"));
    s.env.append("tasks.md", "- [ ] edited externally");
    let (err, after) = s.call("list", json!({}));
    assert!(!err);
    assert!(after[0].contains("edited externally"), "{}", after[0]);
}

// 23
#[test]
fn concurrent_adds_are_serialized() {
    let s = Server::start();
    let addr = s.addr.clone();
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let addr = addr.clone();
            std::thread::spawn(move || {
                let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                    "params":{"name":"add","arguments":{"text":format!("task {i}")}}})
                .to_string();
                let r = http(
                    &addr,
                    "POST",
                    "/mcp",
                    &[("Content-Type", "application/json")],
                    body.as_bytes(),
                );
                assert_eq!(r.status, 200);
                let v = r.json();
                assert_eq!(v["result"]["isError"], json!(false), "{v}");
                v["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
        })
        .collect();
    let got: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let mut nums: Vec<u32> = got
        .iter()
        .map(|t| {
            t.split_once(": ")
                .and_then(|(n, _)| n.parse().ok())
                .unwrap_or_else(|| panic!("unexpected output {t:?}"))
        })
        .collect();
    nums.sort();
    assert_eq!(nums, (12..=21).collect::<Vec<u32>>());
    let file = s.env.read("tasks.md");
    assert_eq!(file.lines().count(), 21);
    for t in &got {
        let (n, line) = t.trim_end().split_once(": ").unwrap();
        let n: usize = n.parse().unwrap();
        assert_eq!(file.lines().nth(n - 1), Some(line), "{t:?}");
    }
}

// 24
#[test]
fn stalled_client_does_not_block_others() {
    let s = Server::start();
    let mut stalled = connect(&s.addr);
    stalled
        .write_all(
            format!(
                "POST /mcp HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: 2000\r\n\r\n0123456789",
                s.addr
            )
            .as_bytes(),
        )
        .unwrap();
    stalled.flush().unwrap();
    std::thread::sleep(Duration::from_millis(200));

    let start = Instant::now();
    let r = s.post_mcp(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
    assert_eq!(r.status, 200);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "ping took {:?}",
        start.elapsed()
    );
    drop(stalled);
}
