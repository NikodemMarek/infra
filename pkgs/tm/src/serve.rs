//! `tm serve`: the task list over HTTP, as an MCP server (`POST /mcp`) and `POST /api/add`.

use std::io::{Read, Write};
use std::sync::{Mutex, PoisonError};
use std::thread;

use schemars::{JsonSchema, Schema};
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request as HttpRequest, Response, Server};
use tm::{AddArgs, DoneArgs, LintArgs, ListArgs, Output, Request, TakeArgs, WeekArgs};

const MAX_BODY: usize = 65536;
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
const INSTRUCTIONS: &str = "Markdown task list. A task is a line like \
`- [ ] description #tag/sub @est(2h) @due(2026-10-18) @proj(name) @week(2026-w43)`; `add` takes the \
text without the checkbox. `@week` plans a task for that ISO week (`take` sets it); the backlog is \
the tasks without `@week`. Use the line numbers from `list` as refs for `done` and `take`.";

/// `tm::execute` reads and writes the file without locking, so calls must not overlap.
static CORE: Mutex<()> = Mutex::new(());

pub fn serve(listen: &str) -> Result<(), String> {
    tm::check_env()?;
    let server = Server::http(listen).map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| format!("cannot listen on {listen}: not an IP address"))?;
    eprintln!("tm: listening on http://{addr}");
    for request in server.incoming_requests() {
        // If the spawn fails, the dropped request makes tiny_http answer 500.
        let _ = thread::Builder::new().spawn(move || handle(request));
    }
    Ok(())
}

fn execute(request: Request) -> Output {
    let _guard = CORE.lock().unwrap_or_else(PoisonError::into_inner);
    tm::execute(request, true)
}

struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
    allow_post: bool,
}

impl Reply {
    fn new(status: u16, content_type: &'static str, body: impl Into<String>) -> Reply {
        Reply {
            status,
            content_type,
            body: body.into(),
            allow_post: false,
        }
    }

    fn text(status: u16, body: impl Into<String>) -> Reply {
        Reply::new(status, "text/plain; charset=utf-8", body)
    }

    fn json(status: u16, body: &Value) -> Reply {
        Reply::new(status, "application/json", body.to_string())
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).expect("header is ASCII")
}

fn handle(mut request: HttpRequest) {
    let line = format!("{} {}", request.method(), request.url());
    let reply = route(&mut request);
    let status = reply.status;
    let mut response = Response::from_data(reply.body)
        .with_status_code(reply.status)
        .with_header(header("Content-Type", reply.content_type));
    if reply.allow_post {
        response = response.with_header(header("Allow", "POST"));
    }
    // The client may be gone already; there is nobody to tell.
    let _ = request.respond(response);
    // After the response, and not `eprintln!`: a failing stderr must not panic a handled request.
    let _ = writeln!(std::io::stderr(), "tm: {line} -> {status}");
}

fn route(request: &mut HttpRequest) -> Reply {
    // Browsers always send Origin on cross-origin requests; the other clients don't.
    if request.headers().iter().any(|h| h.field.equiv("Origin")) {
        return Reply::text(403, "tm: cross-origin requests are not allowed");
    }
    let path = request.url().split('?').next().unwrap_or_default();
    // One arm per endpoint. A REST endpoint maps the body to a `Request`.
    let endpoint: fn(&[u8]) -> Reply = match path {
        "/mcp" => mcp,
        "/api/add" => |body| rest(body, |text| Request::Add(AddArgs { text })),
        _ => return Reply::text(404, "tm: not found"),
    };
    if *request.method() != Method::Post {
        return Reply {
            allow_post: true,
            ..Reply::text(405, "tm: method not allowed")
        };
    }
    let mut body = Vec::new();
    // Counting what was read also limits chunked bodies, which have no Content-Length.
    let read = request
        .as_reader()
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut body);
    match read {
        Ok(_) if body.len() > MAX_BODY => Reply::text(413, "tm: request body too large"),
        Ok(_) => endpoint(&body),
        Err(e) => Reply::text(400, format!("tm: cannot read the body: {e}")),
    }
}

/// The output as text blocks: stdout, then the stderr lines in one block; never none.
fn blocks(output: &Output) -> Vec<String> {
    let mut blocks = Vec::new();
    if !output.stdout.is_empty() {
        blocks.push(output.stdout.clone());
    }
    if !output.stderr.is_empty() {
        let lines: Vec<String> = output.stderr.iter().map(|l| format!("tm: {l}")).collect();
        blocks.push(lines.join("\n"));
    }
    if blocks.is_empty() {
        blocks.push(String::new());
    }
    blocks
}

fn rest(body: &[u8], make: fn(String) -> Request) -> Reply {
    let Ok(text) = std::str::from_utf8(body) else {
        return Reply::text(400, "tm: body must be UTF-8");
    };
    let output = execute(make(text.trim().to_string()));
    let status = if output.failed { 400 } else { 200 };
    Reply::text(status, blocks(&output).concat())
}

type RpcError = (i32, String);

fn envelope(id: &Value, outcome: Result<Value, RpcError>) -> Value {
    match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    }
}

fn mcp(body: &[u8]) -> Reply {
    let Ok(message) = serde_json::from_slice::<Value>(body) else {
        return rpc_failure(Value::Null, -32700, "Parse error");
    };
    let Some(message) = message.as_object() else {
        return rpc_failure(Value::Null, -32600, "Invalid Request");
    };
    // Only notifications carry no id, and they get no answer. The server never sends requests,
    // so there are no responses to receive.
    let id = match message.get("id") {
        None | Some(Value::Null) => return Reply::new(202, "text/plain; charset=utf-8", ""),
        Some(id) => id.clone(),
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return rpc_failure(id, -32600, "Invalid Request");
    };
    let params = message.get("params");
    let outcome = match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => call_tool(params),
        _ => Err((-32601, format!("Method not found: {method}"))),
    };
    Reply::json(200, &envelope(&id, outcome))
}

fn rpc_failure(id: Value, code: i32, message: &str) -> Reply {
    Reply::json(400, &envelope(&id, Err((code, message.to_string()))))
}

fn initialize(params: Option<&Value>) -> Value {
    let version = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str)
        .filter(|v| PROTOCOL_VERSIONS.contains(v))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "tm", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

fn call_tool(params: Option<&Value>) -> Result<Value, RpcError> {
    let Some(name) = params.and_then(|p| p.get("name")).and_then(Value::as_str) else {
        return Err((-32602, "params.name must be a string".to_string()));
    };
    let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
        return Err((-32602, format!("Unknown tool: {name}")));
    };
    let mut arguments = match params.and_then(|p| p.get("arguments")) {
        None | Some(Value::Null) => json!({}),
        Some(arguments) => arguments.clone(),
    };
    // Null means absent.
    if let Some(object) = arguments.as_object_mut() {
        object.retain(|_, v| !v.is_null());
    }
    // Serde structs would also take an array positionally.
    let request = if arguments.is_object() {
        (tool.parse)(arguments).map_err(|e| e.to_string())
    } else {
        Err("arguments must be an object".to_string())
    };
    let output = match request {
        Ok(request) => execute(request),
        Err(e) => Output {
            stdout: String::new(),
            stderr: vec![e],
            failed: true,
        },
    };
    let content: Vec<Value> = blocks(&output)
        .into_iter()
        .map(|text| json!({"type": "text", "text": text}))
        .collect();
    Ok(json!({"content": content, "isError": output.failed}))
}

/// A tool is a `Request` subcommand; its description is the subcommand's help plus `suffix`.
struct Tool {
    name: &'static str,
    read_only: bool,
    /// What MCP clients need beyond the CLI help.
    suffix: &'static str,
    schema: fn() -> Schema,
    /// An `Err` for arguments that don't fit the tool.
    parse: fn(Value) -> Result<Request, serde_json::Error>,
}

static TOOLS: [Tool; 7] = [
    Tool {
        name: "add",
        read_only: false,
        suffix: "Returns the new line as N: <line>.",
        schema: schema::<AddArgs>,
        parse: |a| serde_json::from_value(a).map(Request::Add),
    },
    Tool {
        name: "done",
        read_only: false,
        suffix: "Returns each changed line as N: <line>.",
        schema: schema::<DoneArgs>,
        parse: |a| serde_json::from_value(a).map(Request::Done),
    },
    Tool {
        name: "list",
        read_only: true,
        suffix: "Returns JSON.",
        schema: schema::<ListArgs>,
        parse: |a| serde_json::from_value(a).map(Request::List),
    },
    Tool {
        name: "week",
        read_only: true,
        suffix: "Returns JSON.",
        schema: schema::<WeekArgs>,
        parse: |a| serde_json::from_value(a).map(Request::Week),
    },
    Tool {
        name: "take",
        read_only: false,
        suffix: "",
        schema: schema::<TakeArgs>,
        parse: |a| serde_json::from_value(a).map(Request::Take),
    },
    Tool {
        name: "close",
        read_only: false,
        suffix: "",
        schema: schema::<WeekArgs>,
        parse: |a| serde_json::from_value(a).map(Request::Close),
    },
    Tool {
        name: "lint",
        read_only: true,
        suffix: "",
        schema: schema::<LintArgs>,
        parse: |a| serde_json::from_value(a).map(Request::Lint),
    },
];

fn schema<T: JsonSchema>() -> Schema {
    schemars::schema_for!(T)
}

fn tools() -> Value {
    let command = <Request as clap::Subcommand>::augment_subcommands(clap::Command::new("tm"));
    let tools = TOOLS.iter().map(|tool| {
        let about = command
            .find_subcommand(tool.name)
            .and_then(|c| c.get_about())
            .expect("every tool is a documented Request subcommand");
        let mut schema = (tool.schema)();
        if let Some(object) = schema.as_object_mut() {
            object.remove("$schema");
            object.remove("title");
            // Some clients require the key even when there are no properties.
            let properties = object.entry("properties").or_insert_with(|| json!({}));
            // Null counts as absent, so `Option<String>` is just `"string"`, not `["string", "null"]`,
            // which some clients reject.
            for property in properties
                .as_object_mut()
                .into_iter()
                .flat_map(|p| p.values_mut())
            {
                let not_null = match property.get("type") {
                    Some(Value::Array(types))
                        if types.len() == 2 && types.contains(&json!("null")) =>
                    {
                        types.iter().find(|t| *t != "null").cloned()
                    }
                    _ => None,
                };
                if let Some(t) = not_null {
                    property["type"] = t;
                }
            }
        }
        json!({
            "name": tool.name,
            "description": format!("{about}. {}", tool.suffix).trim_end(),
            "inputSchema": schema,
            "annotations": {"readOnlyHint": tool.read_only},
        })
    });
    Value::Array(tools.collect())
}
