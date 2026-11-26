mod common;

use common::Env;
use serde::Serialize;
use serde_json::{json, Value};

const GL_URL: &str = "https://gitlab.example.com/g/p/-/merge_requests/7";
const GH_URL: &str = "https://github.com/o/r/pull/3";
const GL_DRAFTS: &str = "projects/g%2Fp/merge_requests/7/draft_notes";
const GL_DISC: &str = "projects/g%2Fp/merge_requests/7/discussions?per_page=100";
const GL_VERS: &str = "projects/g%2Fp/merge_requests/7/versions?per_page=100";
const GL_VER_12: &str = "projects/g%2Fp/merge_requests/7/versions/12";
const GL_VER_11: &str = "projects/g%2Fp/merge_requests/7/versions/11";
const GH_REVIEWS: &str = "repos/o/r/pulls/3/reviews";

const DIFF: &str = "@@ -1,3 +1,4 @@\n ctx1\n-old2\n+new2\n+new3\n ctx4\n";
const HUNKS: &str = "@@ -1,2 +1,2 @@\n a\n-b\n+B\n@@ -10,2 +10,3 @@\n x\n-y\n+Y\n+Z\n";
const NOCOUNT: &str = "@@ -1 +1 @@\n-a\n+b\n";
const NONEWLINE: &str =
    "@@ -1,2 +1,2 @@\n x\n-a\n\\ No newline at end of file\n+b\n\\ No newline at end of file\n";
const NEWFILE: &str = "@@ -0,0 +1,2 @@\n+a\n+b\n";
const DELFILE: &str = "@@ -1,2 +0,0 @@\n-a\n-b\n";

fn gl() -> Env {
    Env::outside().cred(&["gitlab.example.com"])
}

fn gh() -> Env {
    Env::outside().cred(&["github.com"])
}

fn gl_ready(e: &mut Env) {
    e.route(&format!("POST {GL_DRAFTS}"), json!({"id": 1}));
    e.route(&format!("GET {GL_DRAFTS}"), json!([]));
    e.route(
        &format!("GET {GL_VERS}"),
        json!([
            {"id": 12, "head_commit_sha": "head12", "base_commit_sha": "base12", "start_commit_sha": "start12"},
            {"id": 11, "head_commit_sha": "head11", "base_commit_sha": "base11", "start_commit_sha": "start11"}
        ]),
    );
    e.route(
        &format!("GET {GL_VER_12}"),
        json!({"id": 12, "head_commit_sha": "head12", "base_commit_sha": "base12", "start_commit_sha": "start12",
               "diffs": [
                   {"old_path": "a.rs", "new_path": "a.rs", "diff": DIFF},
                   {"old_path": "old.rs", "new_path": "new.rs", "diff": DIFF},
                   {"old_path": "h.rs", "new_path": "h.rs", "diff": HUNKS},
                   {"old_path": "n.rs", "new_path": "n.rs", "diff": NOCOUNT},
                   {"old_path": "nl.rs", "new_path": "nl.rs", "diff": NONEWLINE},
                   {"old_path": "newf.rs", "new_path": "newf.rs", "new_file": true, "diff": NEWFILE},
                   {"old_path": "del.rs", "new_path": "del.rs", "deleted_file": true, "diff": DELFILE}
               ]}),
    );
    e.route(
        &format!("GET {GL_VER_11}"),
        json!({"id": 11, "head_commit_sha": "head11", "base_commit_sha": "base11", "start_commit_sha": "start11",
               "diffs": [{"old_path": "a.rs", "new_path": "a.rs", "diff": DIFF}]}),
    );
}

fn gh_lookup(reviews: Value, head: &str) -> Value {
    json!({"data": {"repository": {"pullRequest": {
    "headRefOid": head,
    "reviews": {"nodes": reviews},
    "reviewThreads": {"nodes": [
        {"id": "PRRT_a", "comments": {"nodes": [{"fullDatabaseId": "3000000001"}]}},
        {"id": "PRRT_b", "comments": {"nodes": [{"fullDatabaseId": "3000000002"}]}}
    ]}}}}})
}

fn gh_ready(e: &mut Env) {
    e.route(
        &format!("POST {GH_REVIEWS}"),
        json!({"id": 9, "node_id": "PRR_9"}),
    );
    e.route("POST graphql Review", gh_lookup(json!([]), "abc"));
    e.route(
        "POST graphql AddReply",
        json!({"data": {"addPullRequestReviewThreadReply": {"comment": {"id": "C1"}}}}),
    );
    e.route(
        "POST graphql UpdateReview",
        json!({"data": {"updatePullRequestReview": {"pullRequestReview": {"id": "PRR_P"}}}}),
    );
    e.route(
        "POST graphql AddThread",
        json!({"data": {"addPullRequestReviewThread": {"thread": {"id": "T1"}}}}),
    );
}

fn gh_pending(e: &mut Env, old_body: Value, commit: &str) {
    gh_pending_head(e, old_body, commit, commit);
}

fn gh_pending_head(e: &mut Env, old_body: Value, commit: &str, head: &str) {
    e.route(
        "POST graphql Review",
        gh_lookup(
            json!([gh_review_node("PRR_P", old_body, commit, json!([]))]),
            head,
        ),
    );
}

#[derive(Serialize)]
struct Draft {
    id: String,
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread: Option<String>,
}

fn draft(id: &str, body: &str) -> Draft {
    Draft {
        id: id.into(),
        body: body.into(),
        path: None,
        line: None,
        side: None,
        thread: None,
    }
}

impl Draft {
    fn at(mut self, path: &str, line: u64) -> Draft {
        self.path = Some(path.into());
        self.line = Some(line);
        self
    }
    fn path_only(mut self, path: &str) -> Draft {
        self.path = Some(path.into());
        self
    }
    fn side(mut self, side: &str) -> Draft {
        self.side = Some(side.into());
        self
    }
    fn thread(mut self, thread: &str) -> Draft {
        self.thread = Some(thread.into());
        self
    }
}

fn fnv1a(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

// Expected `pr drafts` output: sorted by id (string order), digest over the printed list.
fn expected(mut drafts: Vec<Draft>) -> (String, Value) {
    drafts.sort_by(|a, b| a.id.cmp(&b.id));
    let list = serde_json::to_string(&drafts).unwrap();
    let digest = fnv1a(list.as_bytes());
    let v = json!({"digest": digest, "drafts": serde_json::from_str::<Value>(&list).unwrap()});
    (digest, v)
}

fn digest_of(drafts: Vec<Draft>) -> String {
    expected(drafts).0
}

fn gh_review_node(id: &str, body: Value, commit: &str, comments: Value) -> Value {
    let total = comments.as_array().map_or(0, |c| c.len());
    gh_review_node_total(id, body, commit, comments, total)
}

fn gh_review_node_total(
    id: &str,
    body: Value,
    commit: &str,
    comments: Value,
    total: usize,
) -> Value {
    json!({"id": id, "body": body, "viewerDidAuthor": true,
           "commit": {"oid": commit}, "comments": {"totalCount": total, "nodes": comments}})
}

fn ops(e: &Env) -> Vec<String> {
    e.api_calls()
        .iter()
        .map(|c| match body(c).get("operationName") {
            Some(o) => o.as_str().unwrap().to_string(),
            None => c["path"].as_str().unwrap().to_string(),
        })
        .collect()
}

fn writes(e: &Env) -> Vec<Value> {
    e.api_calls()
        .into_iter()
        .filter(|c| c["method"] != "GET")
        .filter(|c| body_op(c).as_deref() != Some("Review"))
        .collect()
}

fn body_op(call: &Value) -> Option<String> {
    let b: Value = serde_json::from_str(call["body"].as_str()?).ok()?;
    b.get("operationName")?.as_str().map(String::from)
}

fn body(call: &Value) -> Value {
    serde_json::from_str(call["body"].as_str().unwrap()).unwrap()
}

fn gl_note(summary: &str) -> Value {
    let mut e = gl();
    gl_ready(&mut e);
    let input = json!({"summary": summary}).to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL], &input);
    assert_eq!(out.json(), json!({"url": GL_URL, "drafts": 1}));
    body(&writes(&e)[0])["note"].clone()
}

// validation

fn invalid(input: &str, host: &str, expect: &[&str]) {
    let e = if host == "gl" { gl() } else { gh() };
    let url = if host == "gl" { GL_URL } else { GH_URL };
    let out = e.forge_stdin(&["pr", "review", url], input);
    out.fails();
    for x in expect {
        assert!(out.stderr.contains(x), "missing {x:?} in {}", out.stderr);
    }
    assert!(e.api_calls().is_empty(), "api called: {:?}", e.api_calls());
    assert!(e.auth_calls().is_empty(), "auth called");
}

#[test]
fn nothing_to_submit() {
    invalid("{}", "gl", &["nothing to submit"]);
    invalid(
        r#"{"comments":[],"replies":[]}"#,
        "gh",
        &["nothing to submit"],
    );
}

#[test]
fn unparsable_and_unknown_fields() {
    invalid("not json", "gl", &["invalid review JSON"]);
    invalid(
        r#"{"summary":"x","extra":1}"#,
        "gl",
        &["invalid review JSON"],
    );
    invalid(
        r#"{"comments":[{"path":"a","line":1,"body":"b","nope":1}]}"#,
        "gl",
        &["invalid review JSON"],
    );
    invalid(
        r#"{"replies":[{"thread":"t","body":"b","x":1}]}"#,
        "gl",
        &["invalid review JSON"],
    );
    invalid(
        r#"{"comments":[{"path":"a","line":1,"side":"middle","body":"b"}]}"#,
        "gl",
        &["invalid review JSON"],
    );
}

#[test]
fn all_validation_problems_reported_at_once() {
    let input = json!({
        "summary": "  \n",
        "comments": [
            {"path": "a", "line": 1, "body": "ok"},
            {"path": "a", "line": 1, "body": " "},
            {"path": "", "line": 1, "body": "b"},
            {"path": "a", "line": 0, "body": "b"}
        ],
        "replies": [
            {"thread": "t", "body": ""},
            {"thread": " ", "body": "b"}
        ]
    })
    .to_string();
    invalid(
        &input,
        "gl",
        &[
            "summary: empty",
            "comments[1]: empty body",
            "comments[2]: empty path",
            "comments[3]: line must be >= 1",
            "replies[0]: empty body",
            "replies[1]: empty thread",
        ],
    );
}

#[test]
fn github_resolve_not_supported() {
    invalid(
        r#"{"replies":[{"thread":"1","body":"b","resolve":true}]}"#,
        "gh",
        &["replies[0]: resolve is not supported on GitHub"],
    );
}

#[test]
fn missing_pr_is_usage_error() {
    let e = gl();
    e.forge_stdin(&["pr", "review"], "{}").usage_error();
}

// gitlab

#[test]
fn gitlab_summary_only() {
    let mut e = gl();
    gl_ready(&mut e);
    let out = e.forge_stdin(&["pr", "review", GL_URL], r#"{"summary":"looks good"}"#);
    assert_eq!(out.json(), json!({"url": GL_URL, "drafts": 1}));
    let w = writes(&e);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0]["method"], "POST");
    assert_eq!(w[0]["path"], GL_DRAFTS);
    assert_eq!(w[0]["host"], "gitlab.example.com");
    assert_eq!(body(&w[0]), json!({"note": "looks good"}));
    let paths: Vec<_> = e.api_calls().iter().map(|c| c["path"].clone()).collect();
    assert!(!paths
        .iter()
        .any(|p| p.as_str().unwrap().contains("versions")));
    assert!(!paths
        .iter()
        .any(|p| p.as_str().unwrap().contains("discussions")));
}

#[test]
fn gitlab_comment_positions() {
    let mut e = gl();
    gl_ready(&mut e);
    let input = json!({"comments": [
        {"path": "a.rs", "line": 2, "body": "added"},
        {"path": "a.rs", "line": 1, "body": "ctx new"},
        {"path": "a.rs", "line": 4, "side": "new", "body": "ctx new 4"},
        {"path": "a.rs", "line": 2, "side": "old", "body": "removed"},
        {"path": "a.rs", "line": 3, "side": "old", "body": "ctx old"},
        {"path": "new.rs", "line": 3, "body": "renamed"}
    ]})
    .to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL], &input);
    assert_eq!(out.json(), json!({"url": GL_URL, "drafts": 6}));
    let w = writes(&e);
    assert_eq!(w.len(), 6);
    let pos = |path: &str, extra: Value| {
        let mut p = json!({
            "position_type": "text", "base_sha": "base12", "start_sha": "start12",
            "head_sha": "head12", "old_path": path, "new_path": path
        });
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        p
    };
    let expect = [
        ("added", pos("a.rs", json!({"new_line": 2}))),
        (
            "ctx new",
            pos("a.rs", json!({"new_line": 1, "old_line": 1})),
        ),
        (
            "ctx new 4",
            pos("a.rs", json!({"new_line": 4, "old_line": 3})),
        ),
        ("removed", pos("a.rs", json!({"old_line": 2}))),
        (
            "ctx old",
            pos("a.rs", json!({"old_line": 3, "new_line": 4})),
        ),
        (
            "renamed",
            json!({"position_type": "text", "base_sha": "base12", "start_sha": "start12",
                   "head_sha": "head12", "old_path": "old.rs", "new_path": "new.rs", "new_line": 3}),
        ),
    ];
    for (c, (note, p)) in w.iter().zip(expect.iter()) {
        assert_eq!(c["path"], GL_DRAFTS);
        assert_eq!(body(c), json!({"note": note, "position": p}));
    }
}

fn gl_position(path: &str, line: u64, side: &str) -> Option<Value> {
    let mut e = gl();
    gl_ready(&mut e);
    let input =
        json!({"comments": [{"path": path, "line": line, "side": side, "body": "b"}]}).to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL], &input);
    if out.code == 0 {
        Some(body(&writes(&e)[0])["position"].clone())
    } else {
        out.fails();
        assert!(writes(&e).is_empty());
        None
    }
}

type Lines = Option<(Option<u64>, Option<u64>)>;

#[test]
fn gitlab_hunk_edge_cases() {
    // (path, line, side, expected (old_line, new_line)); None = rejected
    let cases: &[(&str, u64, &str, Lines)] = &[
        ("h.rs", 1, "new", Some((Some(1), Some(1)))),
        ("h.rs", 2, "new", Some((None, Some(2)))),
        ("h.rs", 2, "old", Some((Some(2), None))),
        ("h.rs", 10, "new", Some((Some(10), Some(10)))),
        ("h.rs", 11, "old", Some((Some(11), None))),
        ("h.rs", 11, "new", Some((None, Some(11)))),
        ("h.rs", 12, "new", Some((None, Some(12)))),
        ("h.rs", 5, "new", None),
        ("h.rs", 5, "old", None),
        ("h.rs", 13, "new", None),
        ("n.rs", 1, "old", Some((Some(1), None))),
        ("n.rs", 1, "new", Some((None, Some(1)))),
        ("n.rs", 2, "new", None),
        ("nl.rs", 1, "new", Some((Some(1), Some(1)))),
        ("nl.rs", 2, "old", Some((Some(2), None))),
        ("nl.rs", 2, "new", Some((None, Some(2)))),
        ("nl.rs", 3, "new", None),
        ("newf.rs", 1, "new", Some((None, Some(1)))),
        ("newf.rs", 2, "new", Some((None, Some(2)))),
        ("newf.rs", 3, "new", None),
        ("newf.rs", 1, "old", None),
        ("del.rs", 2, "old", Some((Some(2), None))),
        ("del.rs", 1, "old", Some((Some(1), None))),
        ("del.rs", 3, "old", None),
        ("del.rs", 1, "new", None),
        ("old.rs", 3, "new", Some((None, Some(3)))),
    ];
    for (path, line, side, expect) in cases {
        let got = gl_position(path, *line, side);
        let ctx = format!("{path}:{line} ({side})");
        match (expect, got) {
            (None, None) => {}
            (Some((old, new)), Some(p)) => {
                assert_eq!(
                    p.get("old_line").and_then(Value::as_u64),
                    *old,
                    "{ctx}: {p}"
                );
                assert_eq!(
                    p.get("new_line").and_then(Value::as_u64),
                    *new,
                    "{ctx}: {p}"
                );
                if *path == "old.rs" {
                    assert_eq!(p["old_path"], "old.rs");
                    assert_eq!(p["new_path"], "new.rs");
                }
            }
            (e, g) => panic!("{ctx}: expected {e:?}, got {g:?}"),
        }
    }
}

#[test]
fn gitlab_commit_selects_version() {
    let mut e = gl();
    gl_ready(&mut e);
    let input = json!({"comments": [{"path": "a.rs", "line": 2, "body": "b"}]}).to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL, "--commit", "head11"], &input);
    assert_eq!(out.json()["drafts"], 1);
    let b = body(&writes(&e)[0]);
    assert_eq!(b["position"]["head_sha"], "head11");
    assert_eq!(b["position"]["base_sha"], "base11");
    assert_eq!(b["position"]["start_sha"], "start11");
}

#[test]
fn gitlab_commit_not_a_version() {
    let mut e = gl();
    gl_ready(&mut e);
    let input = json!({"comments": [{"path": "a.rs", "line": 2, "body": "b"}]}).to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL, "--commit", "head1"], &input);
    out.fails();
    assert!(out.stderr.contains("commit head1 is not a version of !7"));
    assert!(writes(&e).is_empty());
}

#[test]
fn gitlab_comment_not_in_diff() {
    let mut e = gl();
    gl_ready(&mut e);
    let input = json!({"comments": [
        {"path": "zzz.rs", "line": 1, "body": "b"},
        {"path": "a.rs", "line": 99, "body": "b"},
        {"path": "a.rs", "line": 4, "side": "old", "body": "b"},
        {"path": "a.rs", "line": 3, "body": "b"}
    ]})
    .to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL], &input);
    out.fails();
    for x in [
        "comments[0]: zzz.rs is not in the diff",
        "comments[1]: a.rs:99 (new) is not in the diff",
        "comments[2]: a.rs:4 (old) is not in the diff",
    ] {
        assert!(out.stderr.contains(x), "missing {x:?} in {}", out.stderr);
    }
    assert!(writes(&e).is_empty());
}

fn gl_two_notes() -> Value {
    json!([{"id": 1, "note": "x"}, {"id": 2, "note": "y"}])
}

fn gl_two_digest() -> String {
    digest_of(vec![draft("1", "x"), draft("2", "y")])
}

#[test]
fn gitlab_review_reads_drafts_first() {
    let mut e = gl();
    gl_ready(&mut e);
    let out = e.forge_stdin(&["pr", "review", GL_URL], r#"{"summary":"s"}"#);
    assert_eq!(out.json(), json!({"url": GL_URL, "drafts": 1}));
    let calls = e.api_calls();
    assert!(calls
        .iter()
        .any(|c| c["path"] == GL_DRAFTS && c["method"] == "GET"));
}

#[test]
fn gitlab_existing_drafts_without_flag_refused() {
    let mut e = gl();
    gl_ready(&mut e);
    e.route(&format!("GET {GL_DRAFTS}"), gl_two_notes());
    let out = e.forge_stdin(&["pr", "review", GL_URL], r#"{"summary":"s"}"#);
    out.fails();
    assert!(
        out.stderr.contains(
            "you have 2 drafts on !7 that you have not read or that changed since: read them with `forge pr drafts` and pass --drafts DIGEST"
        ),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains(&gl_two_digest()), "{}", out.stderr);
    assert!(writes(&e).is_empty(), "{:?}", writes(&e));
}

#[test]
fn gitlab_existing_drafts_wrong_digest_refused() {
    let mut e = gl();
    gl_ready(&mut e);
    e.route(&format!("GET {GL_DRAFTS}"), gl_two_notes());
    let out = e.forge_stdin(
        &["pr", "review", GL_URL, "--drafts", "0000000000000000"],
        r#"{"summary":"s"}"#,
    );
    out.fails();
    assert!(
        out.stderr.contains("you have 2 drafts on !7"),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains(&gl_two_digest()), "{}", out.stderr);
    assert!(writes(&e).is_empty(), "{:?}", writes(&e));
}

#[test]
fn gitlab_existing_drafts_right_digest_adds_alongside() {
    let mut e = gl();
    gl_ready(&mut e);
    e.route(&format!("GET {GL_DRAFTS}"), gl_two_notes());
    let d = gl_two_digest();
    let out = e.forge_stdin(
        &["pr", "review", GL_URL, "--drafts", &d],
        r#"{"summary":"s"}"#,
    );
    assert_eq!(out.json(), json!({"url": GL_URL, "drafts": 1}));
    let w = writes(&e);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0]["path"], GL_DRAFTS);
    assert_eq!(body(&w[0]), json!({"note": "s"}));
}

#[test]
fn gitlab_drafts_flag_ignored_without_drafts() {
    let mut e = gl();
    gl_ready(&mut e);
    let out = e.forge_stdin(
        &["pr", "review", GL_URL, "--drafts", "0123456789abcdef"],
        r#"{"summary":"s"}"#,
    );
    assert_eq!(out.json()["drafts"], 1);
    assert_eq!(writes(&e).len(), 1);
}

#[test]
fn gitlab_drafts_listing() {
    let mut e = gl();
    e.route(
        &format!("GET {GL_DRAFTS}"),
        json!([
            {"id": 5, "note": "general", "position": null},
            {"id": 10, "note": "new side", "discussion_id": null, "position": {
                "new_path": "a.rs", "old_path": "a.rs", "new_line": 4, "old_line": null}},
            {"id": 9, "note": "old side", "position": {
                "new_path": "b.rs", "old_path": "b-old.rs", "new_line": null, "old_line": 2}},
            {"id": 11, "note": "reply", "discussion_id": "d1"},
            {"id": 12, "note": "ctx", "position": {
                "new_path": "c.rs", "old_path": "c.rs", "new_line": 7, "old_line": 6}}
        ]),
    );
    let out = e.forge(&["pr", "drafts", GL_URL]);
    let (_, want) = expected(vec![
        draft("5", "general"),
        draft("10", "new side").at("a.rs", 4).side("new"),
        draft("9", "old side").at("b-old.rs", 2).side("old"),
        draft("11", "reply").thread("d1"),
        draft("12", "ctx").at("c.rs", 7).side("new"),
    ]);
    assert_eq!(out.json(), want);
    let ids: Vec<_> = want["drafts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["10", "11", "12", "5", "9"]);
    assert!(writes(&e).is_empty());
}

#[test]
fn gitlab_drafts_empty_has_digest() {
    let mut e = gl();
    e.route(&format!("GET {GL_DRAFTS}"), json!([]));
    let out = e.forge(&["pr", "drafts", GL_URL]);
    assert_eq!(out.json(), json!({"digest": fnv1a(b"[]"), "drafts": []}));
}

#[test]
fn gitlab_drafts_api_error() {
    let mut e = gl();
    e.route_error(&format!("GET {GL_DRAFTS}"), "boom");
    e.forge(&["pr", "drafts", GL_URL]).fails();
}

#[test]
fn drafts_missing_pr_is_usage_error() {
    gl().forge(&["pr", "drafts"]).usage_error();
}

#[test]
fn gitlab_replies() {
    let mut e = gl();
    gl_ready(&mut e);
    e.route(
        &format!("GET {GL_DISC}"),
        json!([
            {"id": "d1", "individual_note": false, "notes": []},
            {"id": "d2", "individual_note": true, "notes": []}
        ]),
    );
    let input = json!({"replies": [
        {"thread": "d1", "body": "done"},
        {"thread": "d1", "body": "fixed", "resolve": true}
    ]})
    .to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL], &input);
    assert_eq!(out.json(), json!({"url": GL_URL, "drafts": 2}));
    let w = writes(&e);
    assert_eq!(w.len(), 2);
    assert_eq!(
        body(&w[0]),
        json!({"note": "done", "in_reply_to_discussion_id": "d1", "resolve_discussion": false})
    );
    assert_eq!(
        body(&w[1]),
        json!({"note": "fixed", "in_reply_to_discussion_id": "d1", "resolve_discussion": true})
    );
    let paths: Vec<_> = e.api_calls().iter().map(|c| c["path"].clone()).collect();
    assert!(!paths
        .iter()
        .any(|p| p.as_str().unwrap().contains("versions")));
}

#[test]
fn gitlab_reply_thread_not_found() {
    let mut e = gl();
    gl_ready(&mut e);
    e.route(
        &format!("GET {GL_DISC}"),
        json!([
            {"id": "d1", "individual_note": false, "notes": []},
            {"id": "d2", "individual_note": true, "notes": []}
        ]),
    );
    let input = json!({"replies": [
        {"thread": "nope", "body": "b"},
        {"thread": "d2", "body": "b"}
    ]})
    .to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL], &input);
    out.fails();
    assert!(out
        .stderr
        .contains("replies[0]: thread nope not found on !7"));
    assert!(out.stderr.contains("replies[1]: thread d2 not found on !7"));
    assert!(writes(&e).is_empty());
}

#[test]
fn gitlab_write_order_summary_comments_replies() {
    let mut e = gl();
    gl_ready(&mut e);
    e.route(
        &format!("GET {GL_DISC}"),
        json!([{"id": "d1", "individual_note": false, "notes": []}]),
    );
    let input = json!({
        "replies": [{"thread": "d1", "body": "r"}],
        "comments": [{"path": "a.rs", "line": 2, "body": "c1"}, {"path": "a.rs", "line": 1, "body": "c2"}],
        "summary": "s"
    })
    .to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL], &input);
    assert_eq!(out.json()["drafts"], 4);
    let notes: Vec<_> = writes(&e)
        .iter()
        .map(|c| body(c)["note"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(notes, ["s", "c1", "c2", "r"]);
}

#[test]
fn gitlab_write_failure_reports_progress() {
    let mut e = gl();
    gl_ready(&mut e);
    e.route_seq(
        &format!("POST {GL_DRAFTS}"),
        vec![json!({"id": 1}), json!({"__error": "boom"})],
    );
    let input = json!({"summary": "s", "comments": [{"path": "a.rs", "line": 2, "body": "c"}],
                       "replies": []})
    .to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL], &input);
    out.fails();
    assert!(
        out.stderr
            .contains("(1 of 2 drafts created; publish or delete them in the web UI)"),
        "{}",
        out.stderr
    );
    assert_eq!(writes(&e).len(), 2);
}

#[test]
fn gitlab_escape_slash_commands() {
    assert_eq!(gl_note("/merge"), json!("\\/merge"));
    assert_eq!(gl_note("x\n/assign @a"), json!("x\n\\/assign @a"));
    assert_eq!(gl_note("  /merge"), json!("  /merge"));
    assert_eq!(gl_note("a/b"), json!("a/b"));
}

#[test]
fn gitlab_escape_respects_code_blocks() {
    assert_eq!(
        gl_note("```suggestion\n// k\n```"),
        json!("```suggestion\n// k\n```")
    );
    assert_eq!(gl_note("```\n/a\n```\n/b"), json!("```\n/a\n```\n\\/b"));
    assert_eq!(gl_note("```\n/a"), json!("```\n\\/a"));
    assert_eq!(gl_note("~~~\n/a\n~~~"), json!("~~~\n\\/a\n~~~"));
    assert_eq!(
        gl_note("```\n```\n```\n/merge\n```"),
        json!("```\n```\n```\n\\/merge\n```")
    );
}

#[test]
fn gitlab_escape_html_escapes_everything() {
    assert_eq!(
        gl_note("<div>\n</div>\n```\n/a\n```"),
        json!("<div>\n</div>\n```\n\\/a\n```")
    );
}

#[test]
fn gitlab_escape_applies_to_comments_and_replies() {
    let mut e = gl();
    gl_ready(&mut e);
    e.route(
        &format!("GET {GL_DISC}"),
        json!([{"id": "d1", "individual_note": false, "notes": []}]),
    );
    let input = json!({
        "comments": [{"path": "a.rs", "line": 2, "body": "/merge"}],
        "replies": [{"thread": "d1", "body": "/close"}]
    })
    .to_string();
    e.forge_stdin(&["pr", "review", GL_URL], &input).json();
    let w = writes(&e);
    assert_eq!(body(&w[0])["note"], "\\/merge");
    assert_eq!(body(&w[1])["note"], "\\/close");
}

// github

#[test]
fn github_review_with_summary_and_comments() {
    let mut e = gh();
    gh_ready(&mut e);
    let input = json!({"summary": "overall", "comments": [
        {"path": "a.rs", "line": 5, "body": "/merge stays"},
        {"path": "b.rs", "line": 2, "side": "old", "body": "gone"}
    ]})
    .to_string();
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input);
    assert_eq!(out.json(), json!({"url": GH_URL, "drafts": 3}));
    assert_eq!(ops(&e), ["Review", GH_REVIEWS]);
    let w = writes(&e);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0]["path"], GH_REVIEWS);
    assert_eq!(w[0]["host"], "github.com");
    assert_eq!(
        body(&w[0]),
        json!({"body": "overall", "comments": [
            {"path": "a.rs", "line": 5, "side": "RIGHT", "body": "/merge stays"},
            {"path": "b.rs", "line": 2, "side": "LEFT", "body": "gone"}
        ]})
    );
    assert!(body(&w[0]).get("event").is_none());
}

#[test]
fn github_review_without_summary_and_commit() {
    let mut e = gh();
    gh_ready(&mut e);
    let input = json!({"comments": [{"path": "a.rs", "line": 5, "body": "c"}]}).to_string();
    let out = e.forge_stdin(&["pr", "review", GH_URL, "--commit", "abc"], &input);
    assert_eq!(out.json(), json!({"url": GH_URL, "drafts": 1}));
    let b = body(&writes(&e)[0]);
    assert_eq!(b["commit_id"], "abc");
    assert!(b.get("body").is_none(), "{b}");

    let mut e = gh();
    gh_ready(&mut e);
    e.forge_stdin(&["pr", "review", GH_URL], &input).json();
    assert!(body(&writes(&e)[0]).get("commit_id").is_none());
}

#[test]
fn github_replies() {
    let mut e = gh();
    gh_ready(&mut e);
    let input = json!({
        "summary": "s",
        "replies": [{"thread": "3000000002", "body": "one"}, {"thread": "3000000001", "body": "two"}]
    })
    .to_string();
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input);
    assert_eq!(out.json(), json!({"url": GH_URL, "drafts": 3}));
    let calls = e.api_calls();
    let paths: Vec<String> = calls
        .iter()
        .map(|c| {
            format!(
                "{} {}",
                c["method"].as_str().unwrap(),
                c["path"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        paths,
        [
            "POST graphql",
            &format!("POST {GH_REVIEWS}"),
            "POST graphql",
            "POST graphql"
        ]
    );
    let lookup = body(&calls[0]);
    assert_eq!(lookup["operationName"], "Review");
    assert_eq!(
        lookup["variables"],
        json!({"owner": "o", "name": "r", "number": 3})
    );
    assert_eq!(body(&calls[1]), json!({"body": "s", "comments": []}));
    let r1 = body(&calls[2]);
    assert_eq!(r1["operationName"], "AddReply");
    assert_eq!(
        r1["variables"],
        json!({"review": "PRR_9", "thread": "PRRT_b", "body": "one"})
    );
    assert_eq!(
        body(&calls[3])["variables"],
        json!({"review": "PRR_9", "thread": "PRRT_a", "body": "two"})
    );
}

#[test]
fn github_reply_thread_not_found() {
    let mut e = gh();
    gh_ready(&mut e);
    let input =
        json!({"summary": "s", "replies": [{"thread": "3999999999", "body": "b"}]}).to_string();
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input);
    out.fails();
    assert!(out
        .stderr
        .contains("replies[0]: thread 3999999999 not found on #3"));
    assert!(writes(&e).iter().all(|c| c["path"] == "graphql"));
}

#[test]
fn github_review_failure_is_zero_of_n() {
    let mut e = gh();
    gh_ready(&mut e);
    e.route_error(&format!("POST {GH_REVIEWS}"), "Unprocessable Entity");
    let input =
        json!({"summary": "s", "comments": [{"path": "a.rs", "line": 5, "body": "c"}]}).to_string();
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input);
    out.fails();
    assert!(
        out.stderr
            .contains("(0 of 2 drafts created; publish or delete them in the web UI)"),
        "{}",
        out.stderr
    );
}

#[test]
fn github_partial_failure_reports_progress() {
    let mut e = gh();
    gh_ready(&mut e);
    e.route_seq(
        "POST graphql AddReply",
        vec![
            json!({"data": {"addPullRequestReviewThreadReply": {"comment": {"id": "C1"}}}}),
            json!({"__error": "boom"}),
        ],
    );
    let input = json!({
        "summary": "s",
        "comments": [{"path": "a.rs", "line": 5, "body": "c"}],
        "replies": [{"thread": "3000000001", "body": "r1"}, {"thread": "3000000002", "body": "r2"}]
    })
    .to_string();
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input);
    out.fails();
    assert!(
        out.stderr
            .contains("pending review created with 3 of 4 drafts"),
        "{}",
        out.stderr
    );
}

#[test]
fn github_graphql_errors_fail_even_on_success_exit() {
    let mut e = gh();
    gh_ready(&mut e);
    e.route(
        "POST graphql AddReply",
        json!({"data": null, "errors": [{"message": "thread is locked"}]}),
    );
    let input = json!({"replies": [{"thread": "3000000001", "body": "b"}]}).to_string();
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input);
    out.fails();
    assert!(out.stderr.contains("thread is locked"), "{}", out.stderr);
}

#[test]
fn github_graphql_lookup_errors_fail() {
    let mut e = gh();
    gh_ready(&mut e);
    e.route(
        "POST graphql Review",
        json!({"errors": [{"message": "no such repo"}]}),
    );
    let input = json!({"replies": [{"thread": "3000000001", "body": "b"}]}).to_string();
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input);
    out.fails();
    assert!(out.stderr.contains("no such repo"), "{}", out.stderr);
    assert!(!writes(&e).iter().any(|c| c["path"] == GH_REVIEWS));
}

#[test]
fn github_lookup_errors_without_replies_fail_before_writes() {
    let mut e = gh();
    gh_ready(&mut e);
    e.route(
        "POST graphql Review",
        json!({"errors": [{"message": "no such repo"}]}),
    );
    let out = e.forge_stdin(&["pr", "review", GH_URL], r#"{"summary":"s"}"#);
    out.fails();
    assert!(out.stderr.contains("no such repo"), "{}", out.stderr);
    assert!(writes(&e).is_empty(), "{:?}", writes(&e));
}

#[test]
fn github_reviews_null_or_missing_uses_rest() {
    let null_reviews =
        json!({"headRefOid": "abc", "reviews": null, "reviewThreads": {"nodes": []}});
    let missing_reviews = json!({"headRefOid": "abc", "reviewThreads": {"nodes": []}});
    for pr in [null_reviews, missing_reviews] {
        let mut e = gh();
        gh_ready(&mut e);
        e.route(
            "POST graphql Review",
            json!({"data": {"repository": {"pullRequest": pr}}}),
        );
        let out = e.forge_stdin(&["pr", "review", GH_URL], r#"{"summary":"s"}"#);
        assert_eq!(out.json()["drafts"], 1);
        assert_eq!(ops(&e), ["Review", GH_REVIEWS]);
    }
}

#[test]
fn github_pending_review_on_older_commit_than_head_fails() {
    let mut e = gh();
    gh_ready(&mut e);
    gh_pending_head(&mut e, json!(""), "oldc", "headc");
    let out = e.forge_stdin(&["pr", "review", GH_URL], r#"{"summary":"s"}"#);
    out.fails();
    assert!(
        out.stderr.contains(
            "you have a pending review on #3 at commit oldc: publish or delete it to review headc"
        ),
        "{}",
        out.stderr
    );
    assert!(writes(&e).is_empty(), "{:?}", writes(&e));
    assert_eq!(ops(&e), ["Review"]);
}

#[test]
fn github_pending_review_commit_flag_overrides_head() {
    let mut e = gh();
    gh_ready(&mut e);
    gh_pending_head(&mut e, json!(""), "oldc", "headc");
    let out = e.forge_stdin(
        &["pr", "review", GH_URL, "--commit", "oldc"],
        r#"{"summary":"s"}"#,
    );
    assert_eq!(out.json()["drafts"], 1);
    assert_eq!(ops(&e), ["Review", "UpdateReview"]);
}

// github: existing pending review

fn pending_run(input: Value, args: &[&str], old_body: Value, commit: &str) -> (Env, common::Out) {
    let mut e = gh();
    gh_ready(&mut e);
    gh_pending(&mut e, old_body, commit);
    let mut a = vec!["pr", "review", GH_URL];
    a.extend_from_slice(args);
    let out = e.forge_stdin(&a, &input.to_string());
    (e, out)
}

#[test]
fn github_pending_review_is_extended() {
    let input = json!({
        "summary": "new",
        "comments": [
            {"path": "a.rs", "line": 5, "body": "c1"},
            {"path": "b.rs", "line": 2, "side": "old", "body": "c2"}
        ],
        "replies": [{"thread": "3000000002", "body": "r"}]
    });
    let (e, out) = pending_run(input, &[], json!("  "), "abc");
    assert_eq!(out.json(), json!({"url": GH_URL, "drafts": 4}));
    assert_eq!(
        ops(&e),
        [
            "Review",
            "UpdateReview",
            "AddThread",
            "AddThread",
            "AddReply"
        ]
    );
    let calls = e.api_calls();
    assert_eq!(
        body(&calls[1])["variables"],
        json!({"review": "PRR_P", "body": "new"})
    );
    assert_eq!(
        body(&calls[2])["variables"],
        json!({"review": "PRR_P", "path": "a.rs", "line": 5, "side": "RIGHT", "body": "c1"})
    );
    assert_eq!(
        body(&calls[3])["variables"],
        json!({"review": "PRR_P", "path": "b.rs", "line": 2, "side": "LEFT", "body": "c2"})
    );
    assert_eq!(
        body(&calls[4])["variables"],
        json!({"review": "PRR_P", "thread": "PRRT_b", "body": "r"})
    );
}

#[test]
fn github_pending_review_empty_body_uses_summary_alone() {
    for old in [json!(""), json!("  \n"), Value::Null] {
        let (e, out) = pending_run(json!({"summary": "new"}), &[], old, "abc");
        assert_eq!(out.json()["drafts"], 1);
        assert_eq!(ops(&e), ["Review", "UpdateReview"]);
        assert_eq!(
            body(&e.api_calls()[1])["variables"],
            json!({"review": "PRR_P", "body": "new"})
        );
    }
}

#[test]
fn github_pending_review_without_summary_skips_update() {
    let input = json!({"comments": [{"path": "a.rs", "line": 5, "body": "c"}]});
    let d = digest_of(vec![draft("PRR_P", "old")]);
    let (e, out) = pending_run(input, &["--drafts", &d], json!("old"), "abc");
    assert_eq!(out.json()["drafts"], 1);
    assert_eq!(ops(&e), ["Review", "AddThread"]);
}

fn existing_comments() -> Value {
    json!([
        {"id": "PRRC_2", "body": "second", "path": "b.rs", "line": 9, "replyTo": null},
        {"id": "PRRC_1", "body": "first", "path": "a.rs", "line": 3,
         "replyTo": {"fullDatabaseId": "3000000001"}}
    ])
}

fn existing_drafts(summary: &str) -> Vec<Draft> {
    let mut v = vec![
        draft("PRRC_2", "second").at("b.rs", 9),
        draft("PRRC_1", "first").at("a.rs", 3).thread("3000000001"),
    ];
    if !summary.trim().is_empty() {
        v.push(draft("PRR_P", summary));
    }
    v
}

fn pending_with_comments(body: &str) -> Env {
    let mut e = gh();
    gh_ready(&mut e);
    e.route(
        "POST graphql Review",
        gh_lookup(
            json!([gh_review_node(
                "PRR_P",
                json!(body),
                "abc",
                existing_comments()
            )]),
            "abc",
        ),
    );
    e
}

#[test]
fn github_pending_existing_comments_without_flag_refused() {
    let e = pending_with_comments("");
    let input = json!({"comments": [{"path": "a.rs", "line": 5, "body": "c"}]});
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input.to_string());
    out.fails();
    assert!(
        out.stderr.contains(
            "you have 2 drafts on #3 that you have not read or that changed since: read them with `forge pr drafts` and pass --drafts DIGEST"
        ),
        "{}",
        out.stderr
    );
    assert!(
        !out.stderr.contains(&digest_of(existing_drafts(""))),
        "{}",
        out.stderr
    );
    assert_eq!(ops(&e), ["Review"]);
}

#[test]
fn github_pending_wrong_digest_refused() {
    let e = pending_with_comments("");
    let input = json!({"comments": [{"path": "a.rs", "line": 5, "body": "c"}]});
    let out = e.forge_stdin(
        &["pr", "review", GH_URL, "--drafts", "0000000000000000"],
        &input.to_string(),
    );
    out.fails();
    assert!(
        out.stderr.contains("you have 2 drafts on #3"),
        "{}",
        out.stderr
    );
    assert_eq!(ops(&e), ["Review"]);
}

#[test]
fn github_pending_right_digest_proceeds_with_blank_body_summary() {
    let e = pending_with_comments("");
    let d = digest_of(existing_drafts(""));
    let input = json!({"summary": "new", "comments": [{"path": "a.rs", "line": 5, "body": "c"}]});
    let out = e.forge_stdin(
        &["pr", "review", GH_URL, "--drafts", &d],
        &input.to_string(),
    );
    assert_eq!(out.json()["drafts"], 2);
    assert_eq!(ops(&e), ["Review", "UpdateReview", "AddThread"]);
    assert_eq!(
        body(&e.api_calls()[1])["variables"],
        json!({"review": "PRR_P", "body": "new"})
    );
}

#[test]
fn github_pending_nonblank_body_counts_as_draft() {
    let e = pending_with_comments("old summary");
    let input = json!({"comments": [{"path": "a.rs", "line": 5, "body": "c"}]});
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input.to_string());
    out.fails();
    assert!(
        out.stderr.contains("you have 3 drafts on #3"),
        "{}",
        out.stderr
    );
    assert_eq!(ops(&e), ["Review"]);

    let e = pending_with_comments("old summary");
    let d = digest_of(existing_drafts("old summary"));
    let out = e.forge_stdin(
        &["pr", "review", GH_URL, "--drafts", &d],
        &input.to_string(),
    );
    assert_eq!(out.json()["drafts"], 1);
    assert_eq!(ops(&e), ["Review", "AddThread"]);
}

#[test]
fn github_pending_summary_on_nonblank_body_refused() {
    let e = pending_with_comments("old summary");
    let d = digest_of(existing_drafts("old summary"));
    let input = json!({"summary": "new", "comments": [{"path": "a.rs", "line": 5, "body": "c"}]});
    let out = e.forge_stdin(
        &["pr", "review", GH_URL, "--drafts", &d],
        &input.to_string(),
    );
    out.fails();
    assert!(
        out.stderr.contains(
            "your pending review on #3 already has a summary: add the remark as a comment"
        ),
        "{}",
        out.stderr
    );
    assert_eq!(ops(&e), ["Review"]);
}

#[test]
fn github_drafts_flag_ignored_without_drafts() {
    let (e, out) = pending_run(
        json!({"summary": "s"}),
        &["--drafts", "0123456789abcdef"],
        json!(""),
        "abc",
    );
    assert_eq!(out.json()["drafts"], 1);
    assert_eq!(ops(&e), ["Review", "UpdateReview"]);

    let mut e = gh();
    gh_ready(&mut e);
    let out = e.forge_stdin(
        &["pr", "review", GH_URL, "--drafts", "0123456789abcdef"],
        r#"{"summary":"s"}"#,
    );
    assert_eq!(out.json()["drafts"], 1);
    assert_eq!(ops(&e), ["Review", GH_REVIEWS]);
}

#[test]
fn github_drafts_listing() {
    let mut e = gh();
    e.route(
        "POST graphql Review",
        gh_lookup(
            json!([gh_review_node(
                "PRR_P",
                json!("the summary"),
                "abc",
                json!([
                    {"id": "PRRC_3", "body": "numeric reply", "path": "c.rs", "line": 1,
                     "replyTo": {"fullDatabaseId": 3000000002u64}},
                    {"id": "PRRC_2", "body": "file level", "path": "b.rs", "line": null,
                     "replyTo": null},
                    {"id": "PRRC_1", "body": "first", "path": "a.rs", "line": 3,
                     "replyTo": {"fullDatabaseId": "3000000001"}}
                ])
            )]),
            "abc",
        ),
    );
    let out = e.forge(&["pr", "drafts", GH_URL]);
    let (_, want) = expected(vec![
        draft("PRR_P", "the summary"),
        draft("PRRC_1", "first").at("a.rs", 3).thread("3000000001"),
        draft("PRRC_2", "file level").path_only("b.rs"),
        draft("PRRC_3", "numeric reply")
            .at("c.rs", 1)
            .thread("3000000002"),
    ]);
    assert_eq!(out.json(), want);
    for d in want["drafts"].as_array().unwrap() {
        assert!(d.get("side").is_none());
    }
    assert!(writes(&e).is_empty());
}

#[test]
fn github_drafts_blank_review_body_omitted() {
    let mut e = gh();
    e.route(
        "POST graphql Review",
        gh_lookup(
            json!([gh_review_node(
                "PRR_P",
                Value::Null,
                "abc",
                json!([{"id": "PRRC_1", "body": "c", "path": "a.rs", "line": 3, "replyTo": null}])
            )]),
            "abc",
        ),
    );
    let out = e.forge(&["pr", "drafts", GH_URL]);
    let (_, want) = expected(vec![draft("PRRC_1", "c").at("a.rs", 3)]);
    assert_eq!(out.json(), want);
}

#[test]
fn github_drafts_none_is_empty_list_with_digest() {
    let mut e = gh();
    gh_ready(&mut e);
    let out = e.forge(&["pr", "drafts", GH_URL]);
    assert_eq!(out.json(), json!({"digest": fnv1a(b"[]"), "drafts": []}));
}

#[test]
fn github_drafts_ignore_someone_elses_pending_review() {
    let mut e = gh();
    e.route(
        "POST graphql Review",
        gh_lookup(
            json!([{"id": "PRR_X", "body": "theirs", "viewerDidAuthor": false,
                    "commit": {"oid": "abc"},
                    "comments": {"totalCount": 1, "nodes": [{"id": "PRRC_9", "body": "x", "path": "a.rs",
                                            "line": 1, "replyTo": null}]}}]),
            "abc",
        ),
    );
    let out = e.forge(&["pr", "drafts", GH_URL]);
    assert_eq!(out.json(), json!({"digest": fnv1a(b"[]"), "drafts": []}));
}

#[test]
fn github_drafts_graphql_error_fails() {
    let mut e = gh();
    e.route(
        "POST graphql Review",
        json!({"errors": [{"message": "no such repo"}]}),
    );
    let out = e.forge(&["pr", "drafts", GH_URL]);
    out.fails();
    assert!(out.stderr.contains("no such repo"), "{}", out.stderr);
}

#[test]
fn github_pending_review_of_someone_else_is_ignored() {
    let mut e = gh();
    gh_ready(&mut e);
    e.route(
        "POST graphql Review",
        gh_lookup(
            json!([{"id": "PRR_X", "body": "b", "viewerDidAuthor": false,
                          "commit": {"oid": "abc"}}]),
            "abc",
        ),
    );
    let input = json!({"summary": "s", "comments": [{"path": "a.rs", "line": 5, "body": "c"}]});
    let out = e.forge_stdin(
        &["pr", "review", GH_URL, "--commit", "zzz"],
        &input.to_string(),
    );
    assert_eq!(out.json()["drafts"], 2);
    assert_eq!(ops(&e), ["Review", GH_REVIEWS]);
}

#[test]
fn github_pending_review_commit_mismatch_fails_before_writes() {
    let (e, out) = pending_run(
        json!({"summary": "s"}),
        &["--commit", "def"],
        json!(" "),
        "abc",
    );
    out.fails();
    assert!(
        out.stderr.contains(
            "you have a pending review on #3 at commit abc: publish or delete it to review def"
        ),
        "{}",
        out.stderr
    );
    assert_eq!(ops(&e), ["Review"]);
}

#[test]
fn github_pending_review_matching_commit_proceeds() {
    let (e, out) = pending_run(
        json!({"summary": "s"}),
        &["--commit", "abc"],
        json!(""),
        "abc",
    );
    assert_eq!(out.json()["drafts"], 1);
    assert_eq!(ops(&e), ["Review", "UpdateReview"]);
}

#[test]
fn github_pending_thread_failure_reports_progress() {
    let mut e = gh();
    gh_ready(&mut e);
    gh_pending(&mut e, json!(""), "abc");
    e.route_seq(
        "POST graphql AddThread",
        vec![
            json!({"data": {"addPullRequestReviewThread": {"thread": {"id": "T1"}}}}),
            json!({"__error": "boom"}),
        ],
    );
    let input = json!({"summary": "s", "comments": [
        {"path": "a.rs", "line": 5, "body": "c1"},
        {"path": "a.rs", "line": 6, "body": "c2"}
    ]});
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input.to_string());
    out.fails();
    assert!(
        out.stderr
            .contains("(2 of 3 drafts created; publish or delete them in the web UI)"),
        "{}",
        out.stderr
    );
}

#[test]
fn github_pending_mutation_graphql_errors_fail() {
    let mut e = gh();
    gh_ready(&mut e);
    gh_pending(&mut e, json!(""), "abc");
    e.route(
        "POST graphql AddThread",
        json!({"data": null, "errors": [{"message": "line not in diff"}]}),
    );
    let input = json!({"comments": [{"path": "a.rs", "line": 5, "body": "c"}]});
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input.to_string());
    out.fails();
    assert!(out.stderr.contains("line not in diff"), "{}", out.stderr);
    assert!(out
        .stderr
        .contains("(0 of 1 drafts created; publish or delete them in the web UI)"));
}

// amendment

const TOO_MANY: &str =
    "your pending review on #3 has more than 100 comments: publish part of it first";

fn pending_overflow() -> Env {
    let mut e = gh();
    gh_ready(&mut e);
    e.route(
        "POST graphql Review",
        gh_lookup(
            json!([gh_review_node_total(
                "PRR_P",
                json!(""),
                "abc",
                existing_comments(),
                101
            )]),
            "abc",
        ),
    );
    e
}

#[test]
fn github_drafts_more_than_100_comments_fails() {
    let e = pending_overflow();
    let out = e.forge(&["pr", "drafts", GH_URL]);
    out.fails();
    assert!(out.stderr.contains(TOO_MANY), "{}", out.stderr);
    assert!(writes(&e).is_empty());
}

#[test]
fn github_review_more_than_100_comments_fails_without_writes() {
    let input = json!({"summary": "s", "comments": [{"path": "a.rs", "line": 5, "body": "c"}]});
    let e = pending_overflow();
    let out = e.forge_stdin(&["pr", "review", GH_URL], &input.to_string());
    out.fails();
    assert!(writes(&e).is_empty(), "{:?}", writes(&e));

    let e = pending_overflow();
    let d = digest_of(existing_drafts(""));
    let out = e.forge_stdin(
        &["pr", "review", GH_URL, "--drafts", &d],
        &input.to_string(),
    );
    out.fails();
    assert!(out.stderr.contains(TOO_MANY), "{}", out.stderr);
    assert!(writes(&e).is_empty(), "{:?}", writes(&e));
}

#[test]
fn github_total_count_equal_to_nodes_works() {
    let mut e = gh();
    e.route(
        "POST graphql Review",
        gh_lookup(
            json!([gh_review_node_total(
                "PRR_P",
                json!(""),
                "abc",
                existing_comments(),
                2
            )]),
            "abc",
        ),
    );
    let out = e.forge(&["pr", "drafts", GH_URL]);
    assert_eq!(out.json(), expected(existing_drafts("")).1);
}

#[test]
fn gitlab_drafts_position_of_nulls_has_no_location() {
    let mut e = gl();
    e.route(
        &format!("GET {GL_DRAFTS}"),
        json!([
            {"id": 1, "note": "nulls", "position": {
                "position_type": "text", "base_sha": null, "start_sha": null, "head_sha": null,
                "old_path": null, "new_path": null, "old_line": null, "new_line": null}},
            {"id": 2, "note": "missing"},
            {"id": 3, "note": "paths only", "position": {
                "position_type": "text", "old_path": "a.rs", "new_path": "a.rs",
                "old_line": null, "new_line": null}},
            {"id": 4, "note": "old", "position": {
                "position_type": "text", "old_path": "o.rs", "new_path": null,
                "old_line": 5, "new_line": null}}
        ]),
    );
    let out = e.forge(&["pr", "drafts", GL_URL]);
    let (_, want) = expected(vec![
        draft("1", "nulls"),
        draft("2", "missing"),
        draft("3", "paths only"),
        draft("4", "old").at("o.rs", 5).side("old"),
    ]);
    assert_eq!(out.json(), want);
}

#[test]
fn gitlab_changed_body_since_read_is_refused() {
    let mut e = gl();
    e.route(&format!("GET {GL_DRAFTS}"), gl_two_notes());
    let d = e.forge(&["pr", "drafts", GL_URL]).json()["digest"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(d, gl_two_digest());

    let mut e = gl();
    gl_ready(&mut e);
    e.route(
        &format!("GET {GL_DRAFTS}"),
        json!([{"id": 1, "note": "x"}, {"id": 2, "note": "y edited"}]),
    );
    let out = e.forge_stdin(
        &["pr", "review", GL_URL, "--drafts", &d],
        r#"{"summary":"s"}"#,
    );
    out.fails();
    assert!(
        out.stderr.contains("you have 2 drafts on !7"),
        "{}",
        out.stderr
    );
    assert!(writes(&e).is_empty(), "{:?}", writes(&e));
}

#[test]
fn github_changed_body_since_read_is_refused() {
    let node = |second: &str| {
        gh_review_node(
            "PRR_P",
            json!(""),
            "abc",
            json!([
                {"id": "PRRC_1", "body": "first", "path": "a.rs", "line": 3, "replyTo": null},
                {"id": "PRRC_2", "body": second, "path": "b.rs", "line": 9, "replyTo": null}
            ]),
        )
    };
    let mut e = gh();
    e.route(
        "POST graphql Review",
        gh_lookup(json!([node("second")]), "abc"),
    );
    let d = e.forge(&["pr", "drafts", GH_URL]).json()["digest"]
        .as_str()
        .unwrap()
        .to_string();

    let mut e = gh();
    gh_ready(&mut e);
    e.route(
        "POST graphql Review",
        gh_lookup(json!([node("second edited")]), "abc"),
    );
    let input = json!({"comments": [{"path": "a.rs", "line": 5, "body": "c"}]});
    let out = e.forge_stdin(
        &["pr", "review", GH_URL, "--drafts", &d],
        &input.to_string(),
    );
    out.fails();
    assert!(
        out.stderr.contains("you have 2 drafts on #3"),
        "{}",
        out.stderr
    );
    assert!(writes(&e).is_empty(), "{:?}", writes(&e));
}

#[test]
fn gitlab_refusal_with_comments_and_replies_makes_no_post() {
    let mut e = gl();
    gl_ready(&mut e);
    e.route(&format!("GET {GL_DRAFTS}"), gl_two_notes());
    e.route(
        &format!("GET {GL_DISC}"),
        json!([{"id": "d1", "individual_note": false, "notes": []}]),
    );
    let input = json!({
        "comments": [{"path": "a.rs", "line": 2, "body": "c"}],
        "replies": [{"thread": "d1", "body": "r"}]
    })
    .to_string();
    let out = e.forge_stdin(&["pr", "review", GL_URL], &input);
    out.fails();
    assert!(
        out.stderr.contains("you have 2 drafts on !7"),
        "{}",
        out.stderr
    );
    assert!(writes(&e).is_empty(), "{:?}", writes(&e));
}
