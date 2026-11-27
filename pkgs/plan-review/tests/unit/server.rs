use super::*;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::store::{Meta, Store};
use serde_json::{json, Value};

const HOST: &str = "127.0.0.1:4388";
const ORIGIN: &str = "http://127.0.0.1:4388";
const ID: &str = "0123456789abcdef";
const TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
const PANEL_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-src 'self'; frame-ancestors 'none'";
const ARTIFACT_CSP: &str = "sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:";
const PLAN: &str = "<html><body><h1>Plan</h1></body></html>";

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fx {
    store: Store,
    root: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    /// A store with one review at `revision`, every r<N>.html holding PLAN.
    fn new(revision: u64) -> Fx {
        let root = std::env::temp_dir().join(format!(
            "plan-review-server-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let dir = root.join(ID);
        fs::create_dir_all(&dir).unwrap();
        let fx = Fx {
            store: Store::new(root.clone()),
            root,
        };
        let meta = Meta {
            session: "s-1".to_string(),
            path: "/work/plan.html".to_string(),
            title: "My plan".to_string(),
            token: TOKEN.to_string(),
            revision,
            updated: 1_700_000_000,
        };
        fs::write(dir.join("meta.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        for n in 1..=revision {
            fs::write(dir.join(format!("r{n}.html")), PLAN).unwrap();
        }
        fx
    }

    /// A store whose root does not exist yet.
    fn empty() -> Fx {
        let root = std::env::temp_dir().join(format!(
            "plan-review-server-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        Fx {
            store: Store::new(root.clone()),
            root,
        }
    }

    /// Adds a review dir with meta and r1..r<revision>.html.
    fn add(&self, id: &str, token: &str, session: &str, title: &str, revision: u64, updated: u64) {
        let dir = self.root.join(id);
        fs::create_dir_all(&dir).unwrap();
        let meta = Meta {
            session: session.to_string(),
            path: format!("/work/{id}.html"),
            title: title.to_string(),
            token: token.to_string(),
            revision,
            updated,
        };
        fs::write(dir.join("meta.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        for n in 1..=revision {
            fs::write(dir.join(format!("r{n}.html")), PLAN).unwrap();
        }
    }

    /// Review ids (dir names) under the root.
    fn ids(&self) -> Vec<String> {
        let mut v: Vec<String> = match fs::read_dir(&self.root) {
            Ok(rd) => rd
                .map(|e| e.unwrap())
                .filter(|e| e.file_type().unwrap().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => vec![],
        };
        v.sort();
        v
    }

    fn dir(&self) -> PathBuf {
        self.root.join(ID)
    }

    fn set_meta_title(&self, title: &str) {
        let p = self.dir().join("meta.json");
        let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        v["title"] = json!(title);
        fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
    }

    fn write_revision(&self, n: u64, html: &[u8]) {
        fs::write(self.dir().join(format!("r{n}.html")), html).unwrap();
    }

    fn feedback_path(&self, n: u64) -> PathBuf {
        self.dir().join(format!("feedback-r{n}.json"))
    }

    fn write_feedback(&self, n: u64) {
        fs::write(self.feedback_path(n), br#"{"review":"x"}"#).unwrap();
    }
}

fn req(method: &str, url: &str, host: Option<&str>, origin: Option<&str>, body: &[u8]) -> Request {
    Request {
        method: method.to_string(),
        url: url.to_string(),
        host: host.map(String::from),
        origin: origin.map(String::from),
        body: body.to_vec(),
    }
}

fn get(url: &str) -> Request {
    req("GET", url, Some(HOST), None, b"")
}

fn post(url: &str, body: &[u8]) -> Request {
    req("POST", url, Some(HOST), Some(ORIGIN), body)
}

type Calls = Vec<(String, Value)>;

fn run(fx: &Fx, r: &Request) -> (Response, Calls) {
    run_with(fx, r, false)
}

fn run_with(fx: &Fx, r: &Request, fail: bool) -> (Response, Calls) {
    let mut calls: Calls = Vec::new();
    let resp = {
        let mut publish = |kind: &str, payload: &Value| -> Result<(), String> {
            calls.push((kind.to_string(), payload.clone()));
            if fail {
                Err("event-pub failed".to_string())
            } else {
                Ok(())
            }
        };
        handle(&fx.store, r, &mut publish)
    };
    (resp, calls)
}

fn header<'a>(resp: &'a Response, name: &str) -> Option<&'a str> {
    resp.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn assert_common_headers(resp: &Response) {
    assert_eq!(
        header(resp, "cache-control"),
        Some("no-store"),
        "status {}",
        resp.status
    );
    assert_eq!(
        header(resp, "referrer-policy"),
        Some("same-origin"),
        "status {}",
        resp.status
    );
    assert_eq!(
        header(resp, "x-content-type-options"),
        Some("nosniff"),
        "status {}",
        resp.status
    );
}

fn panel_url() -> String {
    format!("/r/{ID}?t={TOKEN}")
}

fn artifact_url(n: u64) -> String {
    format!("/r/{ID}/{n}?t={TOKEN}")
}

fn submit_url(n: u64) -> String {
    format!("/r/{ID}/{n}/submit?t={TOKEN}")
}

fn valid_body() -> Value {
    json!({"notes": [{"selector": "#a", "excerpt": "x", "cell": null, "text": "fix this"}], "answers": {}, "message": ""})
}

fn submit(fx: &Fx, body: &Value) -> (Response, Calls) {
    run(fx, &post(&submit_url(1), body.to_string().as_bytes()))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// The parsed `#review` JSON and its raw text.
fn review_json(resp: &Response) -> (Value, String) {
    let body = String::from_utf8(resp.body.clone()).unwrap();
    let marker = "<script type=\"application/json\" id=\"review\">";
    let start = body.find(marker).expect("review script") + marker.len();
    let end = start + body[start..].find("</script>").expect("review script end");
    let raw = body[start..end].to_string();
    (serde_json::from_str(&raw).expect("review JSON"), raw)
}

// ---- Host ----

#[test]
fn host_other_than_exact_is_403_on_every_route() {
    let fx = Fx::new(1);
    let urls = [
        ("GET", panel_url()),
        ("GET", artifact_url(1)),
        ("POST", submit_url(1)),
        ("GET", "/nothing".to_string()),
        ("GET", format!("/r/{ID}?t=bad")),
    ];
    let hosts = [
        None,
        Some("127.0.0.1"),
        Some("127.0.0.1:4389"),
        Some("localhost:4388"),
        Some("evil.example.com:4388"),
        Some("127.0.0.1:4388.evil.com"),
        Some(""),
    ];
    for (m, u) in &urls {
        for h in hosts {
            let r = req(m, u, h, Some(ORIGIN), b"{}");
            let (resp, calls) = run(&fx, &r);
            assert_eq!(resp.status, 403, "{m} {u} host {h:?}");
            assert_common_headers(&resp);
            assert!(calls.is_empty());
        }
    }
}

// ---- Common headers ----

#[test]
fn common_headers_on_every_kind_of_response() {
    let fx = Fx::new(1);
    let reqs = vec![
        get(&panel_url()),
        get(&artifact_url(1)),
        get("/nothing"),
        get(&format!("/r/{ID}?t=bad")),
        post(&submit_url(1), valid_body().to_string().as_bytes()),
        post(&submit_url(1), b"not json"),
        req("POST", &submit_url(1), Some(HOST), None, b"{}"),
        req("GET", &panel_url(), Some("x"), None, b""),
    ];
    for r in &reqs {
        let (resp, _) = run(&fx, r);
        assert_common_headers(&resp);
    }
}

// ---- Token and routing ----

#[test]
fn not_found_cases_share_one_body() {
    let fx = Fx::new(1);
    let urls = vec![
        format!("/r/{ID}?t=wrong"),
        format!("/r/{ID}?t={}", &TOKEN[..TOKEN.len() - 1]),
        format!("/r/{ID}?t={TOKEN}0"),
        format!("/r/{ID}?t="),
        format!("/r/{ID}"),
        format!("/r/{ID}/1"),
        format!("/r/{ID}/1?t=wrong"),
        format!("/r/ffffffffffffffff?t={TOKEN}"),
        format!("/r/short?t={TOKEN}"),
        format!("/r/{}?t={TOKEN}", ID.to_uppercase()),
        format!("/r/../x?t={TOKEN}"),
        format!("/r/{ID}/2?t={TOKEN}"),
        format!("/r/{ID}/0?t={TOKEN}"),
        format!("/r/{ID}/abc?t={TOKEN}"),
        format!("/r/{ID}/-1?t={TOKEN}"),
        format!("/r/{ID}/1/extra?t={TOKEN}"),
        format!("/r/{ID}/1/submit?t={TOKEN}"),
        format!("/?t={TOKEN}"),
        format!("/other?t={TOKEN}"),
        format!("/r/?t={TOKEN}"),
    ];
    let mut bodies = Vec::new();
    for u in &urls {
        let (resp, calls) = run(&fx, &get(u));
        assert_eq!(resp.status, 404, "GET {u}");
        assert_common_headers(&resp);
        assert!(calls.is_empty());
        bodies.push(resp.body);
    }
    let methods: Vec<Request> = vec![
        req("POST", &panel_url(), Some(HOST), Some(ORIGIN), b"{}"),
        req("POST", &artifact_url(1), Some(HOST), Some(ORIGIN), b"{}"),
        req("PUT", &panel_url(), Some(HOST), Some(ORIGIN), b""),
        req("DELETE", &panel_url(), Some(HOST), Some(ORIGIN), b""),
        req("HEAD", &panel_url(), Some(HOST), None, b""),
        req(
            "POST",
            &format!("/r/{ID}/1/submit?t=wrong"),
            Some(HOST),
            Some(ORIGIN),
            b"{}",
        ),
        req(
            "POST",
            &format!("/r/ffffffffffffffff/1/submit?t={TOKEN}"),
            Some(HOST),
            Some(ORIGIN),
            b"{}",
        ),
        req(
            "POST",
            &format!("/r/{ID}/2/submit?t={TOKEN}"),
            Some(HOST),
            Some(ORIGIN),
            b"{}",
        ),
        req(
            "POST",
            &format!("/r/{ID}/x/submit?t={TOKEN}"),
            Some(HOST),
            Some(ORIGIN),
            b"{}",
        ),
        req(
            "POST",
            &format!("/r/{ID}/1/submit"),
            Some(HOST),
            Some(ORIGIN),
            b"{}",
        ),
    ];
    for r in &methods {
        let (resp, calls) = run(&fx, r);
        assert_eq!(resp.status, 404, "{} {}", r.method, r.url);
        assert_common_headers(&resp);
        assert!(calls.is_empty());
        bodies.push(resp.body);
    }
    assert!(bodies.windows(2).all(|w| w[0] == w[1]), "404 bodies differ");
}

// ---- Panel ----

#[test]
fn panel_200_html_with_exact_csp() {
    let fx = Fx::new(1);
    let (resp, _) = run(&fx, &get(&panel_url()));
    assert_eq!(resp.status, 200);
    assert!(header(&resp, "content-type").unwrap().contains("text/html"));
    assert_eq!(header(&resp, "content-security-policy"), Some(PANEL_CSP));
    assert_common_headers(&resp);
}

#[test]
fn panel_iframe_points_at_latest_revision() {
    let fx = Fx::new(3);
    let (resp, _) = run(&fx, &get(&panel_url()));
    let body = String::from_utf8(resp.body).unwrap();
    let want = format!("<iframe sandbox=\"allow-scripts\" src=\"/r/{ID}/3?t={TOKEN}\">");
    assert!(body.contains(&want), "missing {want}");
    assert!(!body.contains(&format!("src=\"/r/{ID}/2?t=")));
}

#[test]
fn panel_review_json_fields() {
    let fx = Fx::new(2);
    let (resp, _) = run(&fx, &get(&panel_url()));
    let (v, _) = review_json(&resp);
    assert_eq!(v["id"], ID);
    assert_eq!(v["token"], TOKEN);
    assert_eq!(v["revision"], 2);
    assert_eq!(v["title"], "My plan");
    assert_eq!(v["session"], "s-1");
    assert_eq!(v["path"], "/work/plan.html");
    assert!(v.get("sent").is_some());
    assert!(v.get("previous").is_some());
}

#[test]
fn panel_sent_follows_feedback_of_latest_revision() {
    let fx = Fx::new(2);
    let (resp, _) = run(&fx, &get(&panel_url()));
    assert_eq!(review_json(&resp).0["sent"], json!(false));
    fx.write_feedback(2);
    let (resp, _) = run(&fx, &get(&panel_url()));
    assert_eq!(review_json(&resp).0["sent"], json!(true));
}

#[test]
fn panel_previous_follows_feedback_of_previous_revision() {
    let fx = Fx::new(1);
    let (resp, _) = run(&fx, &get(&panel_url()));
    assert!(review_json(&resp).0["previous"].is_null());
    // feedback of the current revision is never `previous`
    fx.write_feedback(1);
    let (resp, _) = run(&fx, &get(&panel_url()));
    assert!(review_json(&resp).0["previous"].is_null());

    let fx = Fx::new(1);
    let notes = json!([{"selector": "#a", "excerpt": "x", "cell": null, "text": "fix this"}]);
    let answers = json!({"choice": "yes"});
    let fb = json!({
        "review": ID, "revision": 1, "session": "s-1", "title": "My plan",
        "path": "/work/plan.html", "submitted": 1_700_000_100u64,
        "notes": notes, "answers": answers, "message": "hello"});
    fs::write(fx.feedback_path(1), serde_json::to_vec(&fb).unwrap()).unwrap();
    let meta_path = fx.dir().join("meta.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    m["revision"] = json!(2);
    fs::write(&meta_path, serde_json::to_vec(&m).unwrap()).unwrap();
    fx.write_revision(2, PLAN.as_bytes());

    let (resp, _) = run(&fx, &get(&panel_url()));
    let (v, _) = review_json(&resp);
    assert_eq!(v["revision"], 2);
    assert_eq!(v["previous"]["notes"], notes);
    assert_eq!(v["previous"]["answers"], answers);
    assert_eq!(v["previous"]["message"], "hello");
    assert_eq!(v["sent"], json!(false));
}

#[test]
fn panel_previous_null_when_previous_revision_has_no_feedback() {
    let fx = Fx::new(2);
    let (resp, _) = run(&fx, &get(&panel_url()));
    assert!(review_json(&resp).0["previous"].is_null());
}

#[test]
fn panel_title_with_script_end_is_escaped() {
    let fx = Fx::new(1);
    let title = "a </script><b>x</b> & c";
    fx.set_meta_title(title);
    let (resp, _) = run(&fx, &get(&panel_url()));
    let (v, raw) = review_json(&resp);
    assert!(!raw.contains('<'), "raw: {raw}");
    assert!(raw.to_lowercase().contains("\\u003c"), "raw: {raw}");
    assert_eq!(v["title"], title);
}

// ---- Artifact ----

#[test]
fn artifact_csp_and_status() {
    let fx = Fx::new(1);
    let (resp, _) = run(&fx, &get(&artifact_url(1)));
    assert_eq!(resp.status, 200);
    assert_eq!(header(&resp, "content-security-policy"), Some(ARTIFACT_CSP));
    assert_common_headers(&resp);
}

#[test]
fn artifact_serves_requested_older_revision() {
    let fx = Fx::new(2);
    fx.write_revision(1, b"<body>REV1-MARKER</body>");
    fx.write_revision(2, b"<body>REV2-MARKER</body>");
    let (resp, _) = run(&fx, &get(&artifact_url(1)));
    assert_eq!(resp.status, 200);
    let body = String::from_utf8(resp.body).unwrap();
    assert!(body.contains("REV1-MARKER") && !body.contains("REV2-MARKER"));
}

const MARK: &[u8] = b"<script data-plan-review>";

fn assert_injected(resp: &Response, prefix: &[u8], suffix: &[u8]) {
    assert_eq!(resp.status, 200);
    let b = &resp.body;
    assert!(b.starts_with(prefix), "prefix changed");
    assert!(b.ends_with(suffix), "suffix changed");
    assert!(b.len() > prefix.len() + suffix.len());
    let mid = &b[prefix.len()..b.len() - suffix.len()];
    assert!(
        mid.starts_with(MARK),
        "overlay does not start with the marker"
    );
    assert!(
        mid.ends_with(b"</script>"),
        "overlay does not end with </script>"
    );
}

#[test]
fn overlay_goes_before_body_end() {
    let fx = Fx::new(1);
    fx.write_revision(1, b"<html><body>hi</body></html>\n");
    let (resp, _) = run(&fx, &get(&artifact_url(1)));
    assert_injected(&resp, b"<html><body>hi", b"</body></html>\n");
}

#[test]
fn overlay_goes_before_last_body_end() {
    let fx = Fx::new(1);
    fx.write_revision(
        1,
        b"<body>a</body> text <pre>&lt;/body&gt;</pre></body>tail",
    );
    let (resp, _) = run(&fx, &get(&artifact_url(1)));
    assert_injected(
        &resp,
        b"<body>a</body> text <pre>&lt;/body&gt;</pre>",
        b"</body>tail",
    );
}

#[test]
fn overlay_body_end_is_case_insensitive_and_keeps_case() {
    let fx = Fx::new(1);
    fx.write_revision(1, b"<BODY>x</BODY></HTML>");
    let (resp, _) = run(&fx, &get(&artifact_url(1)));
    assert_injected(&resp, b"<BODY>x", b"</BODY></HTML>");
    fx.write_revision(1, b"<body>x</Body>z");
    let (resp, _) = run(&fx, &get(&artifact_url(1)));
    assert_injected(&resp, b"<body>x", b"</Body>z");
}

#[test]
fn overlay_appended_without_body_end() {
    let fx = Fx::new(1);
    let html = b"<h1>no body tag</h1>\xff\xfe";
    fx.write_revision(1, html);
    let (resp, _) = run(&fx, &get(&artifact_url(1)));
    assert_injected(&resp, html, b"");
}

#[test]
fn artifact_bytes_other_than_overlay_unchanged() {
    let fx = Fx::new(1);
    let mut html = b"<body>\xc3\xa9\x00\xff</body>".to_vec();
    html.extend_from_slice(b"\r\n");
    fx.write_revision(1, &html);
    let (resp, _) = run(&fx, &get(&artifact_url(1)));
    assert_injected(&resp, b"<body>\xc3\xa9\x00\xff", b"</body>\r\n");
    assert!(find(&resp.body, MARK).is_some());
    assert_eq!(rfind(&resp.body, MARK), find(&resp.body, MARK));
}

// ---- Submit: order of statuses ----

#[test]
fn submit_host_403_before_everything() {
    let fx = Fx::new(1);
    let r = req(
        "POST",
        &format!("/r/{ID}/1/submit?t=bad"),
        Some("evil:4388"),
        None,
        &vec![0; 1_048_577],
    );
    let (resp, calls) = run(&fx, &r);
    assert_eq!(resp.status, 403);
    assert!(calls.is_empty());
}

#[test]
fn submit_origin_403_missing_null_other() {
    let fx = Fx::new(1);
    let body = valid_body().to_string();
    for origin in [
        None,
        Some("null"),
        Some("http://evil.example.com"),
        Some("http://127.0.0.1:4389"),
        Some("http://localhost:4388"),
        Some("https://127.0.0.1:4388"),
        Some("http://127.0.0.1:4388/"),
        Some(""),
    ] {
        let r = req("POST", &submit_url(1), Some(HOST), origin, body.as_bytes());
        let (resp, calls) = run(&fx, &r);
        assert_eq!(resp.status, 403, "origin {origin:?}");
        assert_common_headers(&resp);
        assert!(calls.is_empty());
        assert!(!fx.feedback_path(1).exists());
    }
}

#[test]
fn submit_bad_origin_beats_bad_token_and_unknown_id() {
    let fx = Fx::new(1);
    for url in [
        format!("/r/{ID}/1/submit?t=bad"),
        format!("/r/ffffffffffffffff/1/submit?t={TOKEN}"),
        format!("/r/{ID}/9/submit?t=bad"),
    ] {
        for origin in [None, Some("null"), Some("http://evil.example.com")] {
            let (resp, _) = run(&fx, &req("POST", &url, Some(HOST), origin, b"{}"));
            assert_eq!(resp.status, 403, "{url} {origin:?}");
        }
    }
}

#[test]
fn submit_bad_origin_beats_malformed_revision() {
    let fx = Fx::new(1);
    let url = format!("/r/{ID}/x/submit?t={TOKEN}");
    for origin in [None, Some("null"), Some("http://evil.example.com")] {
        let (resp, calls) = run(&fx, &req("POST", &url, Some(HOST), origin, b"{}"));
        assert_eq!(resp.status, 403, "origin {origin:?}");
        assert!(calls.is_empty());
    }
    let (resp, _) = run(&fx, &post(&url, b"{}"));
    assert_eq!(resp.status, 404);
}

#[test]
fn submit_404_before_413() {
    let fx = Fx::new(1);
    let big = vec![b' '; 1_048_577];
    let (resp, _) = run(&fx, &post(&format!("/r/{ID}/1/submit?t=bad"), &big));
    assert_eq!(resp.status, 404);
    let (resp, _) = run(&fx, &post(&format!("/r/{ID}/2/submit?t={TOKEN}"), &big));
    assert_eq!(resp.status, 404);
}

#[test]
fn submit_413_above_limit_only() {
    let fx = Fx::new(1);
    let (resp, calls) = run(&fx, &post(&submit_url(1), &vec![b' '; 1_048_577]));
    assert_eq!(resp.status, 413);
    assert_common_headers(&resp);
    assert!(calls.is_empty());
    assert!(!fx.feedback_path(1).exists());

    let mut body = valid_body();
    body["message"] = json!("a".repeat(1_048_576 - body.to_string().len()));
    let raw = body.to_string();
    assert_eq!(raw.len(), 1_048_576);
    let (resp, _) = run(&fx, &post(&submit_url(1), raw.as_bytes()));
    assert_eq!(resp.status, 200);
}

#[test]
fn submit_413_before_409() {
    let fx = Fx::new(2);
    fx.write_feedback(2);
    let (resp, _) = run(&fx, &post(&submit_url(1), &vec![b' '; 1_048_577]));
    assert_eq!(resp.status, 413);
}

#[test]
fn submit_409_newer_revision() {
    let fx = Fx::new(2);
    let (resp, calls) = submit(&fx, &valid_body());
    assert_eq!(resp.status, 409);
    assert!(String::from_utf8_lossy(&resp.body).contains("a newer revision exists"));
    assert_common_headers(&resp);
    assert!(calls.is_empty());
    assert!(!fx.feedback_path(1).exists());
}

#[test]
fn submit_409_newer_beats_already_sent_and_bad_body() {
    let fx = Fx::new(2);
    fx.write_feedback(1);
    let (resp, _) = submit(&fx, &valid_body());
    assert_eq!(resp.status, 409);
    assert!(String::from_utf8_lossy(&resp.body).contains("a newer revision exists"));
    let (resp, _) = run(&fx, &post(&submit_url(1), b"not json"));
    assert_eq!(resp.status, 409);
}

#[test]
fn submit_409_already_sent() {
    let fx = Fx::new(1);
    fx.write_feedback(1);
    let before = fs::read(fx.feedback_path(1)).unwrap();
    let (resp, calls) = submit(&fx, &valid_body());
    assert_eq!(resp.status, 409);
    assert!(String::from_utf8_lossy(&resp.body).contains("already sent"));
    assert!(calls.is_empty());
    assert_eq!(fs::read(fx.feedback_path(1)).unwrap(), before);
}

#[test]
fn submit_409_already_sent_beats_bad_body() {
    let fx = Fx::new(1);
    fx.write_feedback(1);
    let (resp, _) = run(&fx, &post(&submit_url(1), b"not json"));
    assert_eq!(resp.status, 409);
}

#[test]
fn submit_sent_twice_second_is_409() {
    let fx = Fx::new(1);
    assert_eq!(submit(&fx, &valid_body()).0.status, 200);
    let (resp, calls) = submit(&fx, &valid_body());
    assert_eq!(resp.status, 409);
    assert!(calls.is_empty());
}

// ---- Submit: 400 ----

fn assert_400(body: Value) {
    let fx = Fx::new(1);
    let (resp, calls) = submit(&fx, &body);
    assert_eq!(resp.status, 400, "body {body}");
    assert_common_headers(&resp);
    assert!(calls.is_empty(), "published for {body}");
    assert!(!fx.feedback_path(1).exists(), "feedback written for {body}");
}

fn with(field: &str, value: Value) -> Value {
    let mut b = valid_body();
    b[field] = value;
    b
}

fn note(n: Value) -> Value {
    with("notes", json!([n]))
}

#[test]
fn submit_400_not_json_or_not_object() {
    let fx = Fx::new(1);
    for raw in ["", "not json", "[]", "null", "\"x\"", "42", "{"] {
        let (resp, calls) = run(&fx, &post(&submit_url(1), raw.as_bytes()));
        assert_eq!(resp.status, 400, "raw {raw:?}");
        assert!(calls.is_empty());
    }
    let (resp, _) = run(&fx, &post(&submit_url(1), b"\xff\xfe{}"));
    assert_eq!(resp.status, 400);
}

#[test]
fn submit_400_missing_fields() {
    for field in ["notes", "answers", "message"] {
        let mut b = valid_body();
        b.as_object_mut().unwrap().remove(field);
        assert_400(b);
    }
}

#[test]
fn submit_400_wrong_top_level_types() {
    assert_400(with("notes", json!({})));
    assert_400(with("notes", json!("x")));
    assert_400(with("notes", json!(null)));
    assert_400(with("answers", json!([])));
    assert_400(with("answers", json!("x")));
    assert_400(with("answers", json!(null)));
    assert_400(with("message", json!(1)));
    assert_400(with("message", json!(null)));
    assert_400(with("message", json!(["a"])));
}

#[test]
fn submit_400_wrong_note_shapes() {
    assert_400(with("notes", json!(["text"])));
    assert_400(with("notes", json!([null])));
    assert_400(note(json!({"excerpt": "x", "cell": null, "text": "t"})));
    assert_400(note(json!({"selector": "#a", "cell": null, "text": "t"})));
    assert_400(note(
        json!({"selector": "#a", "excerpt": "x", "cell": null}),
    ));
    assert_400(note(json!({"selector": "#a", "excerpt": "x", "text": "t"})));
    assert_400(note(
        json!({"selector": 1, "excerpt": "x", "cell": null, "text": "t"}),
    ));
    assert_400(note(
        json!({"selector": "#a", "excerpt": null, "cell": null, "text": "t"}),
    ));
    assert_400(note(
        json!({"selector": "#a", "excerpt": "x", "cell": null, "text": 5}),
    ));
    assert_400(note(
        json!({"selector": "#a", "excerpt": "x", "cell": "c", "text": "t"}),
    ));
    assert_400(note(
        json!({"selector": "#a", "excerpt": "x", "cell": [], "text": "t"}),
    ));
}

#[test]
fn submit_400_wrong_cell_shapes() {
    let n =
        |cell: Value| note(json!({"selector": "#a", "excerpt": "x", "cell": cell, "text": "t"}));
    assert_400(n(json!({"table": "t", "row": "r"})));
    assert_400(n(json!({"table": "t", "col": "c"})));
    assert_400(n(json!({"row": "r", "col": "c"})));
    assert_400(n(json!({"table": 1, "row": "r", "col": "c"})));
    assert_400(n(json!({"table": "t", "row": null, "col": "c"})));
    assert_400(n(json!({"table": "t", "row": "r", "col": 2})));
}

#[test]
fn submit_400_wrong_answer_value_types() {
    assert_400(with("answers", json!({"k": 1})));
    assert_400(with("answers", json!({"k": true})));
    assert_400(with("answers", json!({"k": null})));
    assert_400(with("answers", json!({"k": ["a"]})));
}

#[test]
fn submit_400_bad_answer_keys() {
    for key in [
        "",
        &"a".repeat(101),
        "a b",
        "a\nb",
        " ",
        "a/b",
        "é",
        "a=b",
        "a,b",
        "a\n",
    ] {
        assert_400(with("answers", json!({ key: "v" })));
    }
}

#[test]
fn submit_400_one_bad_answer_key_among_good_ones() {
    assert_400(with("answers", json!({"ok": "v", "bad key": "v"})));
}

#[test]
fn submit_400_blank_note_text() {
    for text in ["", " ", "  \t\n ", "\u{a0}", "\u{2028}"] {
        assert_400(note(
            json!({"selector": "#a", "excerpt": "x", "cell": null, "text": text}),
        ));
    }
}

#[test]
fn submit_400_blank_note_text_beside_good_note_and_message() {
    let b = json!({"notes": [
        {"selector": "#a", "excerpt": "x", "cell": null, "text": "good"},
        {"selector": "#b", "excerpt": "y", "cell": null, "text": "  "}],
        "answers": {"k": "v"}, "message": "hello"});
    assert_400(b);
}

#[test]
fn submit_400_nothing_to_send() {
    let fx = Fx::new(1);
    for message in ["", "  ", "\n\t \r\n"] {
        let b = json!({"notes": [], "answers": {}, "message": message});
        let (resp, calls) = submit(&fx, &b);
        assert_eq!(resp.status, 400, "message {message:?}");
        assert!(String::from_utf8_lossy(&resp.body).contains("nothing to send"));
        assert!(calls.is_empty());
        assert!(!fx.feedback_path(1).exists());
    }
}

// ---- Submit: success ----

#[test]
fn submit_200_writes_feedback_and_publishes_once() {
    let fx = Fx::new(2);
    let body = json!({
        "notes": [
            {"selector": "#a > p:nth-of-type(2)", "excerpt": "some text", "cell": null, "text": "fix this"},
            {"selector": "table", "excerpt": "cell", "cell": {"table": "T", "row": "R", "col": "C"}, "text": "and this"}],
        "answers": {"choice": "yes", "x.y-z_1": "no"},
        "message": "line1\nline2"});
    let r = post(&submit_url(2), body.to_string().as_bytes());
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    };
    let before = now();
    let (resp, calls) = run(&fx, &r);
    let after = now();
    assert_eq!(resp.status, 200);
    assert_eq!(
        String::from_utf8(resp.body.clone())
            .unwrap()
            .replace(' ', ""),
        "{\"ok\":true}"
    );
    assert_common_headers(&resp);

    assert_eq!(
        calls,
        vec![(
            "review.submitted".to_string(),
            json!({"review": ID, "revision": 2})
        )]
    );

    let path = fx.feedback_path(2);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let f: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(f["review"], ID);
    assert_eq!(f["revision"], 2);
    assert_eq!(f["session"], "s-1");
    assert_eq!(f["title"], "My plan");
    assert_eq!(f["path"], "/work/plan.html");
    let t = f["submitted"]
        .as_u64()
        .expect("submitted is a unix timestamp");
    assert!(t >= before && t <= after + 1, "submitted {t}");
    assert_eq!(f["notes"], body["notes"]);
    assert_eq!(f["answers"], body["answers"]);
    assert_eq!(f["message"], body["message"]);
    assert!(!fx.feedback_path(1).exists());
}

#[test]
fn submit_200_message_only() {
    let fx = Fx::new(1);
    let b = json!({"notes": [], "answers": {}, "message": "just a message"});
    let (resp, calls) = submit(&fx, &b);
    assert_eq!(resp.status, 200);
    assert_eq!(calls.len(), 1);
    assert!(fx.feedback_path(1).exists());
}

#[test]
fn submit_200_answers_only() {
    let fx = Fx::new(1);
    let b = json!({"notes": [], "answers": {"k": ""}, "message": " "});
    let (resp, calls) = submit(&fx, &b);
    assert_eq!(resp.status, 200);
    assert_eq!(calls.len(), 1);
}

#[test]
fn submit_200_answer_key_of_100_chars() {
    let fx = Fx::new(1);
    let key = "aZ09_.-".repeat(15)[..100].to_string();
    let b = json!({"notes": [], "answers": {key: "v"}, "message": ""});
    let (resp, _) = submit(&fx, &b);
    assert_eq!(resp.status, 200);
}

#[test]
fn submit_200_panel_afterwards_reports_sent() {
    let fx = Fx::new(1);
    assert_eq!(submit(&fx, &valid_body()).0.status, 200);
    let (resp, _) = run(&fx, &get(&panel_url()));
    assert_eq!(review_json(&resp).0["sent"], json!(true));
}

// ---- Submit: publisher failure ----

#[test]
fn submit_502_when_publisher_fails_and_feedback_is_removed() {
    let fx = Fx::new(1);
    let r = post(&submit_url(1), valid_body().to_string().as_bytes());
    let (resp, calls) = run_with(&fx, &r, true);
    assert_eq!(resp.status, 502);
    assert_common_headers(&resp);
    assert_eq!(
        calls,
        vec![(
            "review.submitted".to_string(),
            json!({"review": ID, "revision": 1})
        )]
    );
    assert!(!fx.feedback_path(1).exists());
    let (resp, calls) = run(&fx, &r);
    assert_eq!(resp.status, 200, "a retry after 502 must work");
    assert_eq!(calls.len(), 1);
}

// ---- API: Host, Origin, routing ----

fn api_get(url: &str) -> Request {
    req("GET", url, Some(HOST), None, b"")
}

fn api_post(url: &str, body: &[u8]) -> Request {
    req("POST", url, Some(HOST), None, body)
}

fn json_body(resp: &Response) -> Value {
    serde_json::from_slice(&resp.body)
        .unwrap_or_else(|e| panic!("{e}: {:?}", String::from_utf8_lossy(&resp.body)))
}

fn error_of(resp: &Response) -> String {
    json_body(resp)["error"].as_str().unwrap().to_string()
}

fn text_of(resp: &Response) -> String {
    String::from_utf8(resp.body.clone()).unwrap()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn pub_body(session: &str, path: &str, title: &str, html: &str) -> Value {
    json!({"session": session, "path": path, "title": title, "html": html})
}

fn good_pub() -> Value {
    pub_body("s-1", "/work/plan.html", "My plan", PLAN)
}

fn publish_v(fx: &Fx, v: &Value) -> (Response, Calls) {
    run(fx, &api_post("/api/publish", v.to_string().as_bytes()))
}

fn publish_raw(fx: &Fx, body: &[u8]) -> (Response, Calls) {
    run(fx, &api_post("/api/publish", body))
}

fn not_found_body() -> Vec<u8> {
    let fx = Fx::new(1);
    let (resp, _) = run(&fx, &get("/nothing"));
    assert_eq!(resp.status, 404);
    resp.body
}

/// A store with the review `ID` (session s-1, path /work/plan.html) updated now.
fn fresh_review() -> Fx {
    let fx = Fx::empty();
    fx.add(ID, TOKEN, "s-1", "My plan", 1, now_secs());
    let mut meta = fx.store.read_meta(ID).unwrap();
    meta.path = "/work/plan.html".to_string();
    fs::write(
        fx.root.join(ID).join("meta.json"),
        serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();
    fx
}

#[test]
fn api_any_origin_is_403_on_every_api_route() {
    let fx = fresh_review();
    fx.write_full_feedback(1, json!({"message": "hi"}));
    let body = good_pub().to_string();
    for origin in [
        Some(ORIGIN),
        Some("null"),
        Some("http://evil.example.com"),
        Some(""),
    ] {
        let reqs = [
            req("POST", "/api/publish", Some(HOST), origin, body.as_bytes()),
            req("GET", "/api/reviews", Some(HOST), origin, b""),
            req(
                "GET",
                &format!("/api/feedback/{ID}/1"),
                Some(HOST),
                origin,
                b"",
            ),
        ];
        for r in &reqs {
            let (resp, calls) = run(&fx, r);
            assert_eq!(resp.status, 403, "{} {} origin {origin:?}", r.method, r.url);
            assert_eq!(json_body(&resp), json!({"error": "forbidden"}));
            assert_common_headers(&resp);
            assert!(calls.is_empty());
        }
    }
    assert_eq!(fx.ids(), vec![ID.to_string()]);
    assert!(!fx.dir().join("r2.html").exists());
}

#[test]
fn api_host_other_than_addr_is_403() {
    let fx = fresh_review();
    let body = good_pub().to_string();
    let hosts = [
        None,
        Some("127.0.0.1"),
        Some("127.0.0.1:4389"),
        Some("localhost:4388"),
        Some("evil.example.com:4388"),
        Some(""),
    ];
    for h in hosts {
        let reqs = [
            req("POST", "/api/publish", h, None, body.as_bytes()),
            req("GET", "/api/reviews", h, None, b""),
            req("GET", &format!("/api/feedback/{ID}/1"), h, None, b""),
            req("GET", "/api/nothing", h, None, b""),
        ];
        for r in &reqs {
            let (resp, calls) = run(&fx, r);
            assert_eq!(resp.status, 403, "{} {} host {h:?}", r.method, r.url);
            assert_eq!(json_body(&resp), json!({"error": "forbidden"}));
            assert!(calls.is_empty());
        }
    }
    assert!(!fx.dir().join("r2.html").exists());
}

#[test]
fn api_wrong_method_or_path_is_404() {
    let fx = fresh_review();
    fx.write_full_feedback(1, json!({"message": "hi"}));
    let body = good_pub().to_string();
    let mut reqs = vec![];
    for m in ["GET", "PUT", "DELETE", "PATCH"] {
        reqs.push(req(m, "/api/publish", Some(HOST), None, body.as_bytes()));
    }
    for m in ["POST", "PUT", "DELETE"] {
        reqs.push(req(m, "/api/reviews", Some(HOST), None, body.as_bytes()));
        reqs.push(req(
            m,
            &format!("/api/feedback/{ID}/1"),
            Some(HOST),
            None,
            body.as_bytes(),
        ));
    }
    for m in ["GET", "POST"] {
        for u in [
            "/api/nothing".to_string(),
            "/api/reviews/x".to_string(),
            "/api/publish/x".to_string(),
            format!("/api/feedback/{ID}"),
            format!("/api/feedback/{ID}/1/x"),
        ] {
            reqs.push(req(m, &u, Some(HOST), None, body.as_bytes()));
        }
    }
    for r in &reqs {
        let (resp, calls) = run(&fx, r);
        assert_eq!(resp.status, 404, "{} {}", r.method, r.url);
        assert_common_headers(&resp);
        assert!(calls.is_empty());
    }
    assert!(!fx.dir().join("r2.html").exists());
}

// ---- API: publish ----

#[test]
fn publish_new_review_200_snapshot_and_event() {
    let fx = Fx::empty();
    let html = "<html><body>é\u{1F600} \"q\" \\ \n\r\n</body></html>";
    let v = pub_body("sess-1", "/work/plan.html", "My plan", html);
    let (resp, calls) = publish_v(&fx, &v);
    assert_eq!(resp.status, 200, "{}", text_of(&resp));
    assert_common_headers(&resp);
    assert!(header(&resp, "content-type")
        .unwrap()
        .starts_with("application/json"));
    let out = json_body(&resp);
    assert_eq!(out.as_object().unwrap().len(), 2, "{out}");
    let id = out["review"].as_str().unwrap().to_string();
    assert!(is_hex(&id, 16), "{id}");
    assert_eq!(out["revision"], json!(1));

    assert_eq!(fx.ids(), vec![id.clone()]);
    let meta = fx.store.read_meta(&id).unwrap();
    assert!(is_hex(&meta.token, 32), "{}", meta.token);
    assert_eq!(meta.session, "sess-1");
    assert_eq!(meta.path, "/work/plan.html");
    assert_eq!(meta.title, "My plan");
    assert_eq!(meta.revision, 1);
    assert!(meta.updated + 60 >= now_secs() && meta.updated <= now_secs() + 60);
    assert_eq!(
        fs::read(fx.root.join(&id).join("r1.html")).unwrap(),
        html.as_bytes()
    );
    assert_eq!(
        calls,
        vec![(
            "review.requested".to_string(),
            json!({"review": id, "revision": 1, "session": "sess-1",
                   "title": "My plan", "token": meta.token})
        )]
    );
}

#[test]
fn publish_again_reuses_id_and_token_for_the_next_revision() {
    let fx = Fx::empty();
    let (resp, _) = publish_v(&fx, &pub_body("s-1", "/work/plan.html", "One", "<p>v1</p>"));
    let first = json_body(&resp);
    let token = fx
        .store
        .read_meta(first["review"].as_str().unwrap())
        .unwrap()
        .token;

    let (resp, calls) = publish_v(&fx, &pub_body("s-1", "/work/plan.html", "Two", "<p>v2</p>"));
    assert_eq!(resp.status, 200);
    let second = json_body(&resp);
    assert_eq!(second["review"], first["review"]);
    assert_eq!(second["revision"], json!(2));
    let id = second["review"].as_str().unwrap();
    assert_eq!(fx.ids(), vec![id.to_string()]);
    let meta = fx.store.read_meta(id).unwrap();
    assert_eq!(meta.token, token);
    assert_eq!(meta.title, "Two");
    assert_eq!(meta.revision, 2);
    assert_eq!(
        fs::read_to_string(fx.root.join(id).join("r1.html")).unwrap(),
        "<p>v1</p>"
    );
    assert_eq!(
        fs::read_to_string(fx.root.join(id).join("r2.html")).unwrap(),
        "<p>v2</p>"
    );
    assert_eq!(
        calls,
        vec![(
            "review.requested".to_string(),
            json!({"review": id, "revision": 2, "session": "s-1", "title": "Two", "token": token})
        )]
    );
}

#[test]
fn publish_of_an_existing_review_continues_it() {
    let fx = fresh_review();
    let (resp, _) = publish_v(&fx, &good_pub());
    assert_eq!(resp.status, 200);
    assert_eq!(json_body(&resp), json!({"review": ID, "revision": 2}));
    assert_eq!(fx.store.read_meta(ID).unwrap().token, TOKEN);
}

#[test]
fn publish_other_path_or_session_makes_a_new_review() {
    let fx = fresh_review();
    for (session, path) in [("s-1", "/work/other.html"), ("s-2", "/work/plan.html")] {
        let (resp, _) = publish_v(&fx, &pub_body(session, path, "t", "x"));
        assert_eq!(resp.status, 200);
        let out = json_body(&resp);
        assert_ne!(out["review"], json!(ID));
        assert_eq!(out["revision"], json!(1));
        let meta = fx.store.read_meta(out["review"].as_str().unwrap()).unwrap();
        assert_ne!(meta.token, TOKEN);
    }
    assert_eq!(fx.ids().len(), 3);
    assert_eq!(fx.store.read_meta(ID).unwrap().revision, 1);
}

fn published_title(title: &str) -> String {
    let fx = Fx::empty();
    let (resp, calls) = publish_v(&fx, &pub_body("s-1", "/work/plan.html", title, "x"));
    assert_eq!(resp.status, 200, "title {title:?}: {}", text_of(&resp));
    let id = json_body(&resp)["review"].as_str().unwrap().to_string();
    let stored = fx.store.read_meta(&id).unwrap().title;
    assert_eq!(calls[0].1["title"], json!(stored));
    stored
}

#[test]
fn publish_title_is_flattened_cut_to_200_and_trimmed() {
    assert_eq!(
        published_title("  a\n\tb \u{2028} c\u{1}d\u{85}e  "),
        "a b cd e"
    );
    assert_eq!(published_title(&"é".repeat(300)), "é".repeat(200));
    let t = format!("{} {}", "a".repeat(199), "b".repeat(10));
    assert_eq!(published_title(&t), "a".repeat(199));
}

#[test]
fn publish_prunes_reviews_older_than_7_days() {
    let fx = Fx::empty();
    let (old, fresh) = ("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb");
    fx.add(old, TOKEN, "s-9", "old", 1, now_secs() - 8 * 86400);
    fx.add(fresh, TOKEN, "s-9", "fresh", 1, now_secs() - 6 * 86400);
    let (resp, _) = publish_v(&fx, &good_pub());
    assert_eq!(resp.status, 200);
    let new = json_body(&resp)["review"].as_str().unwrap().to_string();
    let mut want = vec![fresh.to_string(), new];
    want.sort();
    assert_eq!(fx.ids(), want);
    assert!(!fx.root.join(old).exists());
}

#[test]
fn publish_modes_are_0700_dirs_and_0600_files() {
    let fx = Fx::empty();
    let (resp, _) = publish_v(&fx, &good_pub());
    let id = json_body(&resp)["review"].as_str().unwrap().to_string();
    let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(fx.root.clone()), 0o700);
    assert_eq!(mode(fx.root.join(&id)), 0o700);
    assert_eq!(mode(fx.root.join(&id).join("meta.json")), 0o600);
    assert_eq!(mode(fx.root.join(&id).join("r1.html")), 0o600);
    assert_eq!(mode(fx.root.join(".lock")), 0o600);
}

#[test]
fn publish_session_names() {
    for s in ["a", "0", "s-1126-155455", "a.b_c-d", "A1"] {
        let fx = Fx::empty();
        let (resp, _) = publish_v(&fx, &pub_body(s, "/work/plan.html", "t", "x"));
        assert_eq!(resp.status, 200, "session {s:?}");
    }
    for s in ["", "-x", ".x", "a b", "a/b", "../x", "a\nb", "é", "a:b"] {
        let fx = Fx::empty();
        let (resp, calls) = publish_v(&fx, &pub_body(s, "/work/plan.html", "t", "x"));
        assert_eq!(resp.status, 400, "session {s:?}");
        assert_eq!(
            error_of(&resp),
            "$AGENT_SESSION is unset or not a valid session name"
        );
        assert_common_headers(&resp);
        assert!(calls.is_empty());
        assert!(fx.ids().is_empty());
    }
}

#[test]
fn publish_path_must_end_in_html() {
    for p in [
        "/work/plan.txt",
        "/work/plan.html.bak",
        "/work/plan",
        "/work/plan.htm",
        "",
    ] {
        let fx = Fx::empty();
        let (resp, calls) = publish_v(&fx, &pub_body("s-1", p, "t", "x"));
        assert_eq!(resp.status, 400, "path {p:?}");
        assert_eq!(error_of(&resp), "the file must end in .html");
        assert!(calls.is_empty());
        assert!(fx.ids().is_empty());
    }
    let fx = Fx::empty();
    let (resp, _) = publish_v(&fx, &pub_body("s-1", "/work/a b.html", "t", "x"));
    assert_eq!(resp.status, 200);
}

#[test]
fn publish_title_must_not_be_empty_after_oneline() {
    for t in ["", "   ", "\n\t", "\u{1}\u{2028}", "\u{7}"] {
        let fx = Fx::empty();
        let (resp, calls) = publish_v(&fx, &pub_body("s-1", "/work/plan.html", t, "x"));
        assert_eq!(resp.status, 400, "title {t:?}");
        assert_eq!(error_of(&resp), "the title is empty");
        assert!(calls.is_empty());
        assert!(fx.ids().is_empty());
    }
}

#[test]
fn publish_html_limit_is_2097152_bytes() {
    let fx = Fx::empty();
    let at = |html: String| publish_v(&fx, &pub_body("s-1", "/work/plan.html", "t", &html));
    let (resp, _) = at("a".repeat(2_097_152));
    assert_eq!(resp.status, 200);
    let (resp, calls) = at("é".repeat(1_048_576));
    assert_eq!(resp.status, 200, "2097152 bytes of 2-byte chars");
    assert_eq!(calls.len(), 1);

    let fx2 = Fx::empty();
    for html in ["a".repeat(2_097_153), "é".repeat(1_048_577)] {
        let (resp, calls) = publish_v(&fx2, &pub_body("s-1", "/work/plan.html", "t", &html));
        assert_eq!(resp.status, 413);
        assert_eq!(error_of(&resp), "the plan is larger than 2097152 bytes");
        assert_common_headers(&resp);
        assert!(calls.is_empty());
        assert!(fx2.ids().is_empty());
    }
}

#[test]
fn publish_body_limit_is_8_mib() {
    let fx = Fx::empty();
    let (resp, calls) = publish_raw(&fx, &vec![b' '; 8 * 1024 * 1024 + 1]);
    assert_eq!(resp.status, 413);
    assert_common_headers(&resp);
    assert!(calls.is_empty());
    let (resp, _) = publish_raw(&fx, &vec![b' '; 8 * 1024 * 1024]);
    assert_eq!(resp.status, 400);
    assert_eq!(error_of(&resp), "the request is malformed");
    assert!(fx.ids().is_empty());
}

#[test]
fn publish_malformed_bodies_are_400() {
    let good = good_pub();
    let mut bodies: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"nope".to_vec(),
        b"{".to_vec(),
        b"[]".to_vec(),
        b"null".to_vec(),
        b"42".to_vec(),
        b"{}".to_vec(),
        b"\xff\xfe".to_vec(),
    ];
    for field in ["session", "path", "title", "html"] {
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove(field);
        bodies.push(missing.to_string().into_bytes());
        for bad in [
            json!(1),
            json!(null),
            json!(["x"]),
            json!({"a": 1}),
            json!(true),
        ] {
            let mut v = good.clone();
            v[field] = bad;
            bodies.push(v.to_string().into_bytes());
        }
    }
    for body in &bodies {
        let fx = Fx::empty();
        let (resp, calls) = publish_raw(&fx, body);
        assert_eq!(resp.status, 400, "{:?}", String::from_utf8_lossy(body));
        assert_eq!(error_of(&resp), "the request is malformed");
        assert_common_headers(&resp);
        assert!(calls.is_empty());
        assert!(fx.ids().is_empty());
    }
}

#[test]
fn publish_check_order() {
    let big = "a".repeat(2_097_153);
    let fx = Fx::empty();
    let check = |v: Value, status: u16, msg: &str| {
        let (resp, calls) = publish_v(&fx, &v);
        assert_eq!(resp.status, status, "{msg}");
        assert_eq!(error_of(&resp), msg);
        assert!(calls.is_empty());
    };
    let malformed = json!({"session": "bad name", "path": "x.txt", "title": "", "html": 5});
    check(malformed, 400, "the request is malformed");
    check(
        pub_body("bad name", "x.txt", "", &big),
        400,
        "$AGENT_SESSION is unset or not a valid session name",
    );
    check(
        pub_body("s-1", "x.txt", "", &big),
        400,
        "the file must end in .html",
    );
    check(
        pub_body("s-1", "x.html", "", &big),
        400,
        "the title is empty",
    );
    check(
        pub_body("s-1", "x.html", "t", &big),
        413,
        "the plan is larger than 2097152 bytes",
    );
    assert!(fx.ids().is_empty());
}

#[test]
fn publish_store_error_is_500() {
    let fx = Fx::empty();
    fs::write(&fx.root, "not a directory").unwrap();
    let (resp, calls) = publish_v(&fx, &good_pub());
    let _ = fs::remove_file(&fx.root);
    assert_eq!(resp.status, 500);
    assert_common_headers(&resp);
    assert!(calls.is_empty());
}

#[test]
fn publish_event_failure_is_502_and_keeps_the_snapshot() {
    let fx = Fx::empty();
    let (resp, calls) = run_with(
        &fx,
        &api_post("/api/publish", good_pub().to_string().as_bytes()),
        true,
    );
    assert_eq!(resp.status, 502);
    assert_common_headers(&resp);
    assert_eq!(
        error_of(&resp),
        "event-pub failed (the snapshot r1 is kept, publish again for the next revision)"
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "review.requested");
    let ids = fx.ids();
    assert_eq!(ids.len(), 1);
    let id = &ids[0];
    assert_eq!(fx.store.read_meta(id).unwrap().revision, 1);
    assert_eq!(
        fs::read_to_string(fx.root.join(id).join("r1.html")).unwrap(),
        PLAN
    );

    let (resp, _) = publish_v(&fx, &good_pub());
    assert_eq!(resp.status, 200);
    assert_eq!(json_body(&resp), json!({"review": id, "revision": 2}));

    let (resp, _) = run_with(
        &fx,
        &api_post("/api/publish", good_pub().to_string().as_bytes()),
        true,
    );
    assert_eq!(resp.status, 502);
    assert_eq!(
        error_of(&resp),
        "event-pub failed (the snapshot r3 is kept, publish again for the next revision)"
    );
    assert!(fx.root.join(id).join("r3.html").exists());
}

// ---- API: reviews ----

fn list_body(fx: &Fx) -> String {
    let (resp, calls) = run(fx, &api_get("/api/reviews"));
    assert_eq!(resp.status, 200);
    assert_common_headers(&resp);
    assert!(header(&resp, "content-type")
        .unwrap()
        .starts_with("text/plain"));
    assert!(calls.is_empty());
    text_of(&resp)
}

fn line(id: &str, token: &str, state: &str, session: &str, rev: u64, title: &str) -> String {
    format!("{ORIGIN}/r/{id}?t={token}\t{state}\t{session}\tr{rev}\t{title}\n")
}

#[test]
fn reviews_empty_or_missing_store_is_an_empty_200() {
    let fx = Fx::empty();
    assert_eq!(list_body(&fx), "");
    assert!(!fx.root.exists(), "listing must not create the store");
    fs::create_dir_all(&fx.root).unwrap();
    assert_eq!(list_body(&fx), "");
}

#[test]
fn reviews_line_format_open_and_sent() {
    let fx = Fx::new(1);
    assert_eq!(list_body(&fx), line(ID, TOKEN, "open", "s-1", 1, "My plan"));
    fx.write_feedback(1);
    assert_eq!(list_body(&fx), line(ID, TOKEN, "sent", "s-1", 1, "My plan"));
}

#[test]
fn reviews_show_latest_revision_and_sent_iff_latest_has_feedback() {
    let fx = Fx::new(2);
    fx.write_feedback(1);
    assert_eq!(list_body(&fx), line(ID, TOKEN, "open", "s-1", 2, "My plan"));
    fx.write_feedback(2);
    assert_eq!(list_body(&fx), line(ID, TOKEN, "sent", "s-1", 2, "My plan"));
}

#[test]
fn reviews_are_newest_updated_first_then_id_ascending() {
    let fx = Fx::empty();
    let t = now_secs();
    let tok = |c: char| c.to_string().repeat(32);
    fx.add("1111111111111111", &tok('1'), "s-a", "A", 1, t - 100);
    fx.add("2222222222222222", &tok('2'), "s-b", "B", 1, t - 300);
    fx.add("3333333333333333", &tok('3'), "s-c", "C", 3, t - 200);
    fx.add("5555555555555555", &tok('5'), "s-e", "E", 1, t - 50);
    fx.add("4444444444444444", &tok('4'), "s-d", "D", 1, t - 50);
    fs::write(
        fx.root.join("3333333333333333").join("feedback-r3.json"),
        "{}",
    )
    .unwrap();
    let want = [
        line("4444444444444444", &tok('4'), "open", "s-d", 1, "D"),
        line("5555555555555555", &tok('5'), "open", "s-e", 1, "E"),
        line("1111111111111111", &tok('1'), "open", "s-a", 1, "A"),
        line("3333333333333333", &tok('3'), "sent", "s-c", 3, "C"),
        line("2222222222222222", &tok('2'), "open", "s-b", 1, "B"),
    ]
    .concat();
    assert_eq!(list_body(&fx), want);
}

#[test]
fn reviews_listing_is_read_only() {
    let fx = Fx::new(1);
    let meta = fx.dir().join("meta.json");
    let before = (
        fs::read(&meta).unwrap(),
        fs::metadata(&meta).unwrap().modified().unwrap(),
    );
    let ids = fx.ids();
    list_body(&fx);
    list_body(&fx);
    assert_eq!(fs::read(&meta).unwrap(), before.0);
    assert_eq!(fs::metadata(&meta).unwrap().modified().unwrap(), before.1);
    assert_eq!(fx.ids(), ids);
    assert!(!fx.dir().join("feedback-r1.json").exists());
}

// ---- API: feedback ----

impl Fx {
    /// feedback-r<n>.json as the submit route writes it, with `rest` overriding fields.
    fn write_full_feedback(&self, n: u64, rest: Value) {
        let mut f = json!({"review": ID, "revision": n, "session": "s-1", "title": "My plan",
                           "path": "/work/plan.html", "submitted": 1_700_000_000,
                           "notes": [], "answers": {}, "message": ""});
        for (k, v) in rest.as_object().unwrap() {
            f[k] = v.clone();
        }
        fs::write(self.feedback_path(n), f.to_string()).unwrap();
    }
}

fn feedback_header(n: u64) -> String {
    format!(
        "Feedback from the user on plan review \"My plan\" (r{n} of /work/plan.html), sent from the plan-review page.\n\
         Full feedback: curl -s {ORIGIN}/api/feedback/{ID}/{n} | jq -r .text\n\
         Revise, then edit /work/plan.html and run plan-review publish again for the next revision."
    )
}

fn feedback(fx: &Fx, n: u64) -> Response {
    run(fx, &api_get(&format!("/api/feedback/{ID}/{n}"))).0
}

#[test]
fn feedback_200_json_with_to_and_full_text() {
    let fx = Fx::new(1);
    fx.write_full_feedback(
        1,
        json!({
            "notes": [
                {"selector": "#a > p:nth-of-type(2)", "excerpt": "some words", "cell": null, "text": "fix this"},
                {"selector": "td", "excerpt": "ignored", "cell": {"table": "Costs", "row": "Rent", "col": "Q1"}, "text": "wrong number"},
                {"selector": "#b", "excerpt": "", "cell": null, "text": "no excerpt"},
            ],
            "answers": {"zeta": "1", "alpha": "yes", "mid": "x y"},
            "message": "first\n\nthird",
        }),
    );
    let resp = feedback(&fx, 1);
    assert_eq!(resp.status, 200);
    assert_common_headers(&resp);
    assert!(header(&resp, "content-type")
        .unwrap()
        .starts_with("application/json"));
    let v = json_body(&resp);
    assert_eq!(v.as_object().unwrap().len(), 2, "{v}");
    assert_eq!(v["to"], json!("s-1"));
    let want = format!(
        "{}\nNotes:\n\
         - [#a > p:nth-of-type(2) \"some words\"] fix this\n\
         - [table \"Costs\" row \"Rent\" col \"Q1\"] wrong number\n\
         - [#b] no excerpt\n\
         Page answers (set by the page's controls, possibly without the user): alpha=yes, mid=x y, zeta=1\n\
         Message typed by the user:\n  > first\n  >\n  > third",
        feedback_header(1)
    );
    assert_eq!(v["text"].as_str().unwrap().trim_end_matches('\n'), want);
}

#[test]
fn feedback_message_only_and_needs_no_token() {
    let fx = Fx::new(1);
    fx.write_full_feedback(1, json!({"message": "hello"}));
    let resp = feedback(&fx, 1);
    assert_eq!(resp.status, 200);
    assert_eq!(
        json_body(&resp)["text"]
            .as_str()
            .unwrap()
            .trim_end_matches('\n'),
        format!(
            "{}\nMessage typed by the user:\n  > hello",
            feedback_header(1)
        )
    );
}

#[test]
fn feedback_to_is_the_meta_session() {
    let fx = Fx::new(1);
    fx.write_full_feedback(1, json!({"session": "sess-other", "message": "hi"}));
    assert_eq!(json_body(&feedback(&fx, 1))["to"], json!("s-1"));
}

#[test]
fn feedback_text_is_not_truncated() {
    let fx = Fx::new(1);
    let msg = "w".repeat(20_000);
    fx.write_full_feedback(1, json!({"message": msg}));
    let text = json_body(&feedback(&fx, 1))["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.ends_with(&format!("  > {msg}")), "cut");
    assert!(!text.contains("truncated"));
}

#[test]
fn feedback_older_revisions_are_still_served() {
    let fx = Fx::new(2);
    fx.write_full_feedback(1, json!({"message": "old words"}));
    let resp = feedback(&fx, 1);
    assert_eq!(resp.status, 200);
    let text = json_body(&resp)["text"].as_str().unwrap().to_string();
    assert!(text.contains("(r1 of /work/plan.html)"), "{text}");
    assert!(
        text.contains(&format!("{ORIGIN}/api/feedback/{ID}/1 | jq")),
        "{text}"
    );
    assert!(
        text.trim_end_matches('\n').ends_with("  > old words"),
        "{text}"
    );
    assert_eq!(feedback(&fx, 2).status, 404);
}

#[test]
fn feedback_404_with_the_shared_not_found_body() {
    let want = not_found_body();
    let fx = Fx::new(2);
    fx.write_full_feedback(1, json!({"message": "hi"}));
    let mut urls = vec![
        format!("/api/feedback/{ID}/0"),
        format!("/api/feedback/{ID}/2"),
        format!("/api/feedback/{ID}/3"),
        format!("/api/feedback/{ID}/x"),
        format!("/api/feedback/{ID}/-1"),
        format!("/api/feedback/{ID}/"),
        "/api/feedback/ffffffffffffffff/1".to_string(),
        format!("/api/feedback/{}/1", ID.to_uppercase()),
        format!("/api/feedback/{}/1", &ID[..15]),
        format!("/api/feedback/{ID}0/1"),
        "/api/feedback/../1".to_string(),
        format!("/api/feedback/..%2F{ID}/1"),
    ];
    urls.push(format!("/api/feedback/{ID}/1.json"));
    for u in &urls {
        let (resp, calls) = run(&fx, &api_get(u));
        assert_eq!(resp.status, 404, "{u}");
        assert_common_headers(&resp);
        assert_eq!(resp.body, want, "{u}");
        assert!(calls.is_empty());
    }
}

#[test]
fn feedback_404_for_missing_meta_or_invalid_feedback_file() {
    let want = not_found_body();
    let fx = Fx::new(1);
    fs::write(fx.feedback_path(1), "not json").unwrap();
    let resp = feedback(&fx, 1);
    assert_eq!(resp.status, 404);
    assert_eq!(resp.body, want);

    fx.write_full_feedback(1, json!({"message": "hi"}));
    assert_eq!(feedback(&fx, 1).status, 200);
    fs::remove_file(fx.dir().join("meta.json")).unwrap();
    let resp = feedback(&fx, 1);
    assert_eq!(resp.status, 404);
    assert_eq!(resp.body, want);

    let fx = Fx::empty();
    assert_eq!(feedback(&fx, 1).status, 404);
}
