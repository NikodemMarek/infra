use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::store::{event_pub, now, valid_id, write_private, Meta, Store, ADDR, ORIGIN};

const PANEL: &str = include_str!("../assets/panel.html");
const OVERLAY: &str = include_str!("../assets/overlay.js");

const MAX_BODY: usize = 1_048_576;

const PANEL_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-src 'self'; frame-ancestors 'none'";
const ARTIFACT_CSP: &str = "sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:";

pub struct Request {
    pub method: String,
    pub url: String,
    pub host: Option<String>,
    pub origin: Option<String>,
    pub body: Vec<u8>,
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Response {
        let header = |name: &str, value: &str| (name.to_string(), value.to_string());
        Response {
            status,
            headers: vec![
                header("Content-Type", content_type),
                header("Cache-Control", "no-store"),
                header("Referrer-Policy", "same-origin"),
                header("X-Content-Type-Options", "nosniff"),
            ],
            body: body.into(),
        }
    }

    fn csp(mut self, policy: &str) -> Response {
        self.headers
            .push(("Content-Security-Policy".to_string(), policy.to_string()));
        self
    }
}

fn error(status: u16, message: &str) -> Response {
    Response::new(
        status,
        "application/json",
        json!({"error": message}).to_string(),
    )
}

fn not_found() -> Response {
    error(404, "not found")
}

enum Route<'a> {
    Panel(&'a str),
    Artifact(&'a str, u64),
    // N stays raw: a malformed one must not be rejected before the Origin check
    Submit(&'a str, &'a str),
}

impl<'a> Route<'a> {
    fn parse(method: &str, path: &'a str) -> Option<Route<'a>> {
        let parts: Vec<&str> = path.strip_prefix("/r/")?.split('/').collect();
        match (method, parts.as_slice()) {
            ("GET", &[id]) => Some(Route::Panel(id)),
            ("GET", &[id, n]) => Some(Route::Artifact(id, revision(n)?)),
            ("POST", &[id, n, "submit"]) => Some(Route::Submit(id, n)),
            _ => None,
        }
    }

    fn id(&self) -> &'a str {
        match *self {
            Route::Panel(id) | Route::Artifact(id, _) | Route::Submit(id, _) => id,
        }
    }
}

fn revision(s: &str) -> Option<u64> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|&n| n >= 1)
}

fn query_token(query: &str) -> Option<&str> {
    query.split('&').find_map(|pair| pair.strip_prefix("t="))
}

fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Cell {
    pub(crate) table: String,
    pub(crate) row: String,
    pub(crate) col: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Note {
    pub(crate) selector: String,
    pub(crate) excerpt: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub(crate) cell: Option<Cell>,
    pub(crate) text: String,
}

/// The body of a submit request, and the part of `feedback-r<N>.json` that the user wrote.
#[derive(Serialize, Deserialize)]
pub(crate) struct Submission {
    pub(crate) notes: Vec<Note>,
    pub(crate) answers: BTreeMap<String, String>,
    pub(crate) message: String,
}

impl Submission {
    fn valid(&self) -> bool {
        self.notes.iter().all(|n| !n.text.trim().is_empty())
            && self.answers.keys().all(|k| valid_answer_key(k))
    }

    fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.answers.is_empty() && self.message.trim().is_empty()
    }
}

fn valid_answer_key(key: &str) -> bool {
    (1..=100).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

/// `<root>/<id>/feedback-r<N>.json`
#[derive(Serialize)]
struct Feedback<'a> {
    review: &'a str,
    revision: u64,
    session: &'a str,
    title: &'a str,
    path: &'a str,
    submitted: u64,
    #[serde(flatten)]
    submission: &'a Submission,
}

pub fn handle(
    store: &Store,
    req: &Request,
    publish: &mut dyn FnMut(&str, &Value) -> Result<(), String>,
) -> Response {
    if req.host.as_deref() != Some(ADDR) {
        return error(403, "forbidden");
    }
    let (path, query) = req.url.split_once('?').unwrap_or((&req.url, ""));
    let Some(route) = Route::parse(&req.method, path) else {
        return not_found();
    };
    if matches!(route, Route::Submit(..)) && req.origin.as_deref() != Some(ORIGIN) {
        return error(403, "forbidden");
    }
    let id = route.id();
    let Some(meta) = valid_id(id)
        .then(|| store.read_meta(id))
        .flatten()
        .filter(|m| query_token(query).is_some_and(|t| same_token(t, &m.token)))
    else {
        return not_found();
    };
    let dir = store.root.join(id);
    match route {
        Route::Panel(_) => panel(store, id, &meta),
        Route::Artifact(_, n) => match fs::read(dir.join(format!("r{n}.html"))) {
            Ok(html) => {
                Response::new(200, "text/html; charset=utf-8", inject(&html)).csp(ARTIFACT_CSP)
            }
            Err(_) => not_found(),
        },
        Route::Submit(_, n) => {
            let Some(n) = revision(n) else {
                return not_found();
            };
            if !dir.join(format!("r{n}.html")).exists() {
                return not_found();
            }
            submit(store, id, n, &req.body, publish)
        }
    }
}

fn panel(store: &Store, id: &str, meta: &Meta) -> Response {
    let sent = store.feedback_path(id, meta.revision).exists();
    let previous = (meta.revision > 1)
        .then(|| fs::read(store.feedback_path(id, meta.revision - 1)).ok())
        .flatten()
        .and_then(|data| serde_json::from_slice::<Submission>(&data).ok());
    let data = json!({
        "id": id,
        "token": meta.token,
        "revision": meta.revision,
        "title": meta.title,
        "session": meta.session,
        "path": meta.path,
        "sent": sent,
        "previous": previous,
    });
    let iframe = format!(
        r#"<iframe sandbox="allow-scripts" src="/r/{id}/{}?t={}"></iframe>"#,
        meta.revision, meta.token
    );
    // the review data goes in last: it carries user text that must not be scanned for placeholders
    let page = PANEL
        .replace("{{iframe}}", &iframe)
        .replace("{{review}}", &data.to_string().replace('<', "\\u003c"));
    Response::new(200, "text/html; charset=utf-8", page).csp(PANEL_CSP)
}

/// The artifact with the overlay before its last `</body>`, else at its end.
fn inject(html: &[u8]) -> Vec<u8> {
    let at = html
        .windows(7)
        .rposition(|w| w.eq_ignore_ascii_case(b"</body>"))
        .unwrap_or(html.len());
    let mut out = html[..at].to_vec();
    out.extend_from_slice(format!("<script data-plan-review>{OVERLAY}</script>").as_bytes());
    out.extend_from_slice(&html[at..]);
    out
}

fn submit(
    store: &Store,
    id: &str,
    n: u64,
    body: &[u8],
    publish: &mut dyn FnMut(&str, &Value) -> Result<(), String>,
) -> Response {
    if body.len() > MAX_BODY {
        return error(413, "the feedback is too large");
    }
    let file = match save(store, id, n, body) {
        Ok(file) => file,
        Err(response) => return response,
    };
    if let Err(e) = publish("review.submitted", &json!({"review": id, "revision": n})) {
        let _ = fs::remove_file(&file);
        return error(502, &format!("the session could not be notified: {e}"));
    }
    Response::new(200, "application/json", json!({"ok": true}).to_string())
}

/// Writes `feedback-r<N>.json` under the lock, so that it cannot race a publish of the next revision.
fn save(store: &Store, id: &str, n: u64, body: &[u8]) -> Result<PathBuf, Response> {
    let _lock = store.lock().map_err(|e| error(500, &e))?;
    let meta = store.read_meta(id).ok_or_else(not_found)?;
    if n != meta.revision {
        return Err(error(409, "a newer revision exists"));
    }
    let file = store.feedback_path(id, n);
    if file.exists() {
        return Err(error(409, "already sent"));
    }
    let submission: Submission = serde_json::from_slice(body)
        .ok()
        .filter(Submission::valid)
        .ok_or_else(|| error(400, "the feedback is malformed"))?;
    if submission.is_empty() {
        return Err(error(400, "nothing to send"));
    }
    let feedback = Feedback {
        review: id,
        revision: n,
        session: &meta.session,
        title: &meta.title,
        path: &meta.path,
        submitted: now(),
        submission: &submission,
    };
    let json = serde_json::to_vec(&feedback).map_err(|e| error(500, &e.to_string()))?;
    write_private(&file, &json).map_err(|e| error(500, &e))?;
    Ok(file)
}

pub fn serve(store: &Store) -> Result<(), String> {
    let server = tiny_http::Server::http(ADDR).map_err(|e| format!("cannot bind {ADDR}: {e}"))?;
    for mut request in server.incoming_requests() {
        let header = |name: &'static str| {
            request
                .headers()
                .iter()
                .find(|h| h.field.equiv(name))
                .map(|h| h.value.as_str().to_string())
        };
        let mut req = Request {
            method: request.method().as_str().to_string(),
            url: request.url().to_string(),
            host: header("Host"),
            origin: header("Origin"),
            body: Vec::new(),
        };
        let read = request
            .as_reader()
            .take(MAX_BODY as u64 + 1)
            .read_to_end(&mut req.body);
        let response = match read {
            Ok(_) => handle(store, &req, &mut event_pub),
            Err(_) => error(400, "the request body could not be read"),
        };
        let mut reply =
            tiny_http::Response::from_data(response.body).with_status_code(response.status);
        for (name, value) in response.headers {
            if let Ok(header) = tiny_http::Header::from_bytes(name, value) {
                reply.add_header(header);
            }
        }
        if let Err(e) = request.respond(reply) {
            eprintln!("plan-review: warning: cannot respond: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/server.rs"]
mod tests;
