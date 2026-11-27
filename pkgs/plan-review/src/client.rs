use std::env;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::store::{addr, oneline, origin, valid_id, valid_token, Requested, MAX_HTML};

const TIMEOUT: Duration = Duration::from_secs(10);

/// One HTTP/1.0 request to the server: the status and the body of its answer.
pub fn request(method: &str, path: &str, body: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let addr = addr();
    let fail = |e: io::Error| format!("cannot reach the plan-review server at {addr}: {e}");
    let target = addr
        .to_socket_addrs()
        .map_err(fail)?
        .next()
        .ok_or_else(|| format!("cannot resolve the plan-review server address {addr}"))?;
    let mut stream = TcpStream::connect_timeout(&target, TIMEOUT).map_err(fail)?;
    stream.set_read_timeout(Some(TIMEOUT)).map_err(fail)?;
    stream.set_write_timeout(Some(TIMEOUT)).map_err(fail)?;
    let head = format!(
        "{method} {path} HTTP/1.0\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(fail)?;
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).map_err(fail)?;
    parse_answer(&answer)
}

fn parse_answer(answer: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let malformed = || "the plan-review server sent a malformed answer".to_string();
    let split = answer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(malformed)?;
    let head = std::str::from_utf8(&answer[..split]).map_err(|_| malformed())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(malformed)?;
    Ok((status, answer[split + 4..].to_vec()))
}

/// The `error` of a failed answer.
pub fn api_error(status: u16, body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("the plan-review server answered {status}"))
}

/// Publish `file` as the next revision of the review of ($AGENT_SESSION, file).
pub fn publish(file: &Path, title: &str) -> Result<(), String> {
    let session = env::var("AGENT_SESSION").unwrap_or_default();
    let file = std::path::absolute(file).map_err(|e| e.to_string())?;
    let not_utf8 = || format!("{} is not valid UTF-8", file.display());
    let path = file.to_str().ok_or_else(not_utf8)?;
    let html = String::from_utf8(read_html(&file)?).map_err(|_| not_utf8())?;
    let body = json!({"session": session, "path": path, "title": title, "html": html});
    let (status, answer) = request("POST", "/api/publish", body.to_string().as_bytes())?;
    if status == 200 {
        Ok(())
    } else {
        Err(api_error(status, &answer))
    }
}

/// Print the reviews as the server lists them.
pub fn list() -> Result<(), String> {
    let (status, body) = request("GET", "/api/reviews", b"")?;
    if status != 200 {
        return Err(api_error(status, &body));
    }
    io::stdout()
        .write_all(&body)
        .map_err(|e| format!("writing stdout: {e}"))
}

/// Handle a `review.requested` event (its JSON on stdin): notify and open the review on request.
pub fn intake(event: &str) -> Result<(), String> {
    let fields: Map<String, Value> =
        serde_json::from_str(event).map_err(|_| "the event is not a JSON object")?;
    let Some(requested) = serde_json::from_value::<Requested>(Value::Object(fields))
        .ok()
        .filter(|r| valid_id(&r.review) && r.revision >= 1 && valid_token(&r.token))
    else {
        return Ok(());
    };
    if let Err(e) = open_on_request(&requested) {
        eprintln!("plan-review: warning: {e}");
    }
    Ok(())
}

/// The content of the plan: a small regular file of ours, opened without following symlinks or waiting on a FIFO.
fn read_html(path: &Path) -> Result<Vec<u8>, String> {
    let fail = |e: io::Error| format!("cannot read {}: {e}", path.display());
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(fail)?;
    let meta = file.metadata().map_err(fail)?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    // SAFETY: geteuid has no preconditions
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(format!("{} is not owned by you", path.display()));
    }
    let too_big = || format!("{} is larger than {MAX_HTML} bytes", path.display());
    if meta.len() > MAX_HTML {
        return Err(too_big());
    }
    let mut data = Vec::new();
    file.take(MAX_HTML + 1)
        .read_to_end(&mut data)
        .map_err(fail)?;
    if data.len() as u64 > MAX_HTML {
        return Err(too_big());
    }
    Ok(data)
}

/// Notifies about the review and, when the user picks "Open", opens it in the browser.
fn open_on_request(requested: &Requested) -> Result<(), String> {
    let out = Command::new("notify-send")
        .args(["-a", "plan-review", "-A", "open=Open", "--"])
        .arg(format!("Plan review: {}", oneline(&requested.title)))
        .arg(escape(&format!(
            "{} · r{}",
            oneline(&requested.session),
            requested.revision
        )))
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("notify-send: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(
            match stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()) {
                Some(line) => format!("notify-send failed: {} ({line})", out.status),
                None => format!("notify-send failed: {}", out.status),
            },
        );
    }
    if String::from_utf8_lossy(&out.stdout).trim() != "open" {
        return Ok(());
    }
    // no output of xdg-open is passed on: the URL carries the token
    let status = Command::new("xdg-open")
        .arg(format!(
            "{}/r/{}?t={}",
            origin(),
            requested.review,
            requested.token
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("xdg-open: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("xdg-open failed: {status}"))
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
