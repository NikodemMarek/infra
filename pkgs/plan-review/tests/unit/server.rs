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
