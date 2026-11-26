mod common;

use common::Env;
use serde_json::{json, Value};

const GL_URL: &str = "https://gitlab.example.com/g/p/-/merge_requests/7";
const GH_URL: &str = "https://github.com/o/r/pull/3";
const GL_MR: &str = "projects/g%2Fp/merge_requests/7";
const GH_PULL: &str = "repos/o/r/pulls/3";
const GL_DISC: &str = "projects/g%2Fp/merge_requests/7/discussions?per_page=100";
const GL_JOBS: &str = "projects/g%2Fp/pipelines/55/jobs?scope%5B%5D=failed&per_page=100";
const GH_REVIEWS: &str = "repos/o/r/pulls/3/reviews?per_page=100";
const GH_ISSUE: &str = "repos/o/r/issues/3/comments?per_page=100";
const GH_INLINE: &str = "repos/o/r/pulls/3/comments?per_page=100";

fn gl() -> Env {
    Env::outside().cred(&["gitlab.example.com"])
}

fn gh() -> Env {
    Env::outside().cred(&["github.com"])
}

fn get(path: &str) -> String {
    format!("GET {path}")
}

fn calls(e: &Env) -> Vec<(String, String, bool)> {
    e.api_calls()
        .iter()
        .map(|c| {
            (
                c["method"].as_str().unwrap().to_string(),
                c["path"].as_str().unwrap().to_string(),
                c["paginate"] == json!(true),
            )
        })
        .collect()
}

fn call(method: &str, path: &str, paginate: bool) -> (String, String, bool) {
    (method.to_string(), path.to_string(), paginate)
}

// checks: GitLab

fn gl_mr(pipeline: Value) -> Value {
    json!({"iid": 7, "state": "opened", "sha": "mrsha1", "web_url": GL_URL, "head_pipeline": pipeline})
}

fn gl_pipeline(status: &str) -> Value {
    json!({"id": 55, "sha": "plsha1", "status": status, "web_url": "https://gitlab.example.com/g/p/-/pipelines/55"})
}

fn gl_checks(pipeline: Value) -> (Env, Value) {
    let mut e = gl();
    e.route(&get(GL_MR), gl_mr(pipeline));
    let v = e.forge(&["pr", "checks", GL_URL]).json();
    (e, v)
}

fn gl_state(status: &str) -> Value {
    let (e, v) = gl_checks(gl_pipeline(status));
    assert_eq!(v["sha"], "plsha1");
    assert_eq!(v["url"], "https://gitlab.example.com/g/p/-/pipelines/55");
    assert_eq!(calls(&e).len(), 1, "{:?}", calls(&e));
    v
}

#[test]
fn gitlab_checks_no_pipeline_is_none_with_mr_sha() {
    let (e, v) = gl_checks(Value::Null);
    assert_eq!(
        v,
        json!({"sha": "mrsha1", "state": "none", "url": null, "failed": []})
    );
    assert_eq!(calls(&e), vec![call("GET", GL_MR, false)]);
}

#[test]
fn gitlab_checks_no_pipeline_empty_mr_sha_is_null() {
    let mut e = gl();
    let mut mr = gl_mr(Value::Null);
    mr["sha"] = json!("");
    e.route(&get(GL_MR), mr);
    let v = e.forge(&["pr", "checks", GL_URL]).json();
    assert_eq!(
        v,
        json!({"sha": null, "state": "none", "url": null, "failed": []})
    );
}

#[test]
fn gitlab_checks_success() {
    assert_eq!(
        gl_state("success"),
        json!({"sha": "plsha1", "state": "success",
               "url": "https://gitlab.example.com/g/p/-/pipelines/55", "failed": []})
    );
}

#[test]
fn gitlab_checks_canceled_and_skipped_are_canceled() {
    assert_eq!(gl_state("canceled")["state"], "canceled");
    assert_eq!(gl_state("skipped")["state"], "canceled");
}

#[test]
fn gitlab_checks_other_statuses_are_pending() {
    for s in [
        "created",
        "waiting_for_resource",
        "preparing",
        "pending",
        "running",
        "manual",
        "scheduled",
    ] {
        let v = gl_state(s);
        assert_eq!(v["state"], "pending", "{s}");
        assert_eq!(v["failed"], json!([]), "{s}");
    }
}

#[test]
fn gitlab_checks_failed_lists_jobs_with_exact_request() {
    let mut e = gl();
    e.route(&get(GL_MR), gl_mr(gl_pipeline("failed")));
    e.route(
        &get(GL_JOBS),
        json!([
            {"id": 101, "name": "lint", "web_url": "https://gitlab.example.com/j/1", "allow_failure": false},
            {"id": 102, "name": "flaky", "web_url": "https://gitlab.example.com/j/2", "allow_failure": true},
            {"id": 103, "name": "test", "web_url": "https://gitlab.example.com/j/3", "allow_failure": false}
        ]),
    );
    let v = e.forge(&["pr", "checks", GL_URL]).json();
    assert_eq!(
        v,
        json!({
            "sha": "plsha1", "state": "failed",
            "url": "https://gitlab.example.com/g/p/-/pipelines/55",
            "failed": [
                {"name": "lint", "url": "https://gitlab.example.com/j/1", "id": 101},
                {"name": "test", "url": "https://gitlab.example.com/j/3", "id": 103}
            ]
        })
    );
    assert_eq!(
        calls(&e),
        vec![call("GET", GL_MR, false), call("GET", GL_JOBS, false)]
    );
}

#[test]
fn gitlab_checks_failed_with_no_listed_jobs_has_empty_failed() {
    let mut e = gl();
    e.route(&get(GL_MR), gl_mr(gl_pipeline("failed")));
    e.route(&get(GL_JOBS), json!([]));
    let v = e.forge(&["pr", "checks", GL_URL]).json();
    assert_eq!(v["state"], "failed");
    assert_eq!(v["failed"], json!([]));
}

#[test]
fn gitlab_checks_not_found_fails() {
    let mut e = gl();
    e.route_error(&get(GL_MR), "404 Not Found");
    let out = e.forge(&["pr", "checks", GL_URL]);
    out.fails();
    assert!(out.stderr.contains("404 Not Found"), "{}", out.stderr);
}

#[test]
fn gitlab_checks_jobs_error_fails() {
    let mut e = gl();
    e.route(&get(GL_MR), gl_mr(gl_pipeline("failed")));
    e.route_error(&get(GL_JOBS), "500 Server Error");
    e.forge(&["pr", "checks", GL_URL]).fails();
}

// checks: GitHub

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn gh_checks_path() -> String {
    format!("repos/o/r/commits/{SHA}/check-runs?filter=latest&per_page=100")
}

fn gh_pull(sha: Value) -> Value {
    json!({"number": 3, "state": "open", "html_url": GH_URL, "head": {"ref": "x", "sha": sha}})
}

fn gh_checks(runs: Value) -> (Env, Value) {
    let mut e = gh();
    e.route(&get(GH_PULL), gh_pull(json!(SHA)));
    e.route(
        &get(&gh_checks_path()),
        json!({"total_count": runs.as_array().unwrap().len(), "check_runs": runs}),
    );
    let v = e.forge(&["pr", "checks", GH_URL]).json();
    (e, v)
}

fn run(name: &str, status: &str, conclusion: Value) -> Value {
    json!({"id": 900 + name.len(), "name": name, "status": status, "conclusion": conclusion,
           "html_url": format!("https://github.com/o/r/runs/{name}")})
}

#[test]
fn github_checks_no_sha_is_none_without_further_call() {
    for sha in [Value::Null, json!("")] {
        let mut e = gh();
        e.route(&get(GH_PULL), gh_pull(sha));
        let v = e.forge(&["pr", "checks", GH_URL]).json();
        assert_eq!(
            v,
            json!({"sha": null, "state": "none", "url": null, "failed": []})
        );
        assert_eq!(calls(&e), vec![call("GET", GH_PULL, false)]);
    }
}

#[test]
fn github_checks_no_runs_is_none_with_sha_and_url() {
    let (e, v) = gh_checks(json!([]));
    assert_eq!(
        v,
        json!({"sha": SHA, "state": "none", "url": format!("{GH_URL}/checks"), "failed": []})
    );
    assert_eq!(
        calls(&e),
        vec![
            call("GET", GH_PULL, false),
            call("GET", &gh_checks_path(), false)
        ]
    );
}

#[test]
fn github_checks_success() {
    let (_, v) = gh_checks(json!([
        run("a", "completed", json!("success")),
        run("b", "completed", json!("skipped")),
        run("c", "completed", json!("neutral"))
    ]));
    assert_eq!(
        v,
        json!({"sha": SHA, "state": "success", "url": format!("{GH_URL}/checks"), "failed": []})
    );
}

#[test]
fn github_checks_failure_and_timed_out_listed_in_order() {
    let (_, v) = gh_checks(json!([
        run("build", "completed", json!("failure")),
        run("ok", "completed", json!("success")),
        run("slow", "completed", json!("timed_out"))
    ]));
    assert_eq!(
        v,
        json!({
            "sha": SHA, "state": "failed", "url": format!("{GH_URL}/checks"),
            "failed": [
                {"name": "build", "url": "https://github.com/o/r/runs/build", "id": 905},
                {"name": "slow", "url": "https://github.com/o/r/runs/slow", "id": 904}
            ]
        })
    );
}

#[test]
fn github_checks_failed_wins_over_pending() {
    let (_, v) = gh_checks(json!([
        run("a", "in_progress", Value::Null),
        run("b", "completed", json!("failure"))
    ]));
    assert_eq!(v["state"], "failed");
    assert_eq!(v["failed"].as_array().unwrap().len(), 1);
}

#[test]
fn github_checks_incomplete_is_pending() {
    for status in ["queued", "in_progress", "waiting", "pending"] {
        let (_, v) = gh_checks(json!([
            run("a", "completed", json!("success")),
            run("b", status, Value::Null)
        ]));
        assert_eq!(v["state"], "pending", "{status}");
        assert_eq!(v["failed"], json!([]));
    }
}

#[test]
fn github_checks_never_canceled() {
    let (_, v) = gh_checks(json!([
        run("a", "completed", json!("cancelled")),
        run("b", "completed", json!("success"))
    ]));
    assert_eq!(v["state"], "success");
    assert_eq!(v["failed"], json!([]));
}

#[test]
fn github_checks_not_found_fails() {
    let mut e = gh();
    e.route_error(&get(GH_PULL), "HTTP 404: Not Found");
    let out = e.forge(&["pr", "checks", GH_URL]);
    out.fails();
    assert!(out.stderr.contains("HTTP 404"), "{}", out.stderr);
}

#[test]
fn github_checks_check_runs_error_fails() {
    let mut e = gh();
    e.route(&get(GH_PULL), gh_pull(json!(SHA)));
    e.route_error(&get(&gh_checks_path()), "HTTP 500");
    e.forge(&["pr", "checks", GH_URL]).fails();
}

// comments: GitLab

fn note(id: u64, created: &str, body: Value) -> Value {
    json!({"id": id, "type": null, "system": false, "created_at": created,
           "author": {"username": "alice"}, "body": body})
}

fn single(n: Value) -> Value {
    json!({"id": format!("d{}", n["id"]), "individual_note": true, "notes": [n]})
}

fn gl_discussions(discussions: Value) -> (Env, Value) {
    let mut e = gl();
    e.route(&get(GL_DISC), discussions);
    let v = e.forge(&["pr", "comments", GL_URL]).json();
    (e, v)
}

fn gl_comments(notes: Value) -> (Env, Value) {
    let ds: Vec<Value> = notes
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .map(single)
        .collect();
    gl_discussions(json!(ds))
}

fn gl_note_url(id: u64) -> String {
    format!("https://gitlab.example.com/g/p/-/merge_requests/7#note_{id}")
}

fn ids(v: &Value) -> Vec<(String, u64)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["kind"].as_str().unwrap().to_string(),
                c["id"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn k(kind: &str, id: u64) -> (String, u64) {
    (kind.to_string(), id)
}

#[test]
fn gitlab_comments_only_system_notes_is_empty() {
    let mut s = note(1, "2026-01-01T10:00:00Z", json!("assigned"));
    s["system"] = json!(true);
    let (e, v) = gl_comments(json!([s]));
    assert_eq!(v, json!([]));
    assert_eq!(calls(&e), vec![call("GET", GL_DISC, true)]);
    assert_eq!(e.api_calls()[0]["output"], "ndjson");
}

#[test]
fn gitlab_comments_empty_is_empty_array() {
    let (_, v) = gl_comments(json!([]));
    assert_eq!(v, json!([]));
}

#[test]
fn gitlab_comments_plain_note_mapping() {
    let (_, v) = gl_comments(json!([note(5, "2026-01-01T10:00:00Z", json!("hello"))]));
    assert_eq!(
        v,
        json!([{
            "id": 5, "kind": "comment", "author": "alice", "association": null,
            "created_at": "2026-01-01T10:00:00Z", "body": "hello", "url": gl_note_url(5),
            "path": null, "line": null, "review_state": null, "thread": null
        }])
    );
}

#[test]
fn gitlab_comments_null_body_is_empty_string() {
    let (_, v) = gl_comments(json!([note(5, "2026-01-01T10:00:00Z", Value::Null)]));
    assert_eq!(v[0]["body"], "");
}

#[test]
fn gitlab_comments_drops_system_keeps_others() {
    let mut s = note(1, "2026-01-01T09:00:00Z", json!("merged"));
    s["system"] = json!(true);
    let (_, v) = gl_comments(json!([s, note(2, "2026-01-01T10:00:00Z", json!("x"))]));
    assert_eq!(ids(&v), vec![k("comment", 2)]);
}

#[test]
fn gitlab_diffnote_uses_new_path_and_line() {
    let mut n = note(6, "2026-01-01T10:00:00Z", json!("fix"));
    n["type"] = json!("DiffNote");
    n["position"] =
        json!({"new_path": "a/new.rs", "old_path": "a/old.rs", "new_line": 12, "old_line": 9});
    let (_, v) = gl_comments(json!([n]));
    assert_eq!(v[0]["kind"], "inline");
    assert_eq!(v[0]["path"], "a/new.rs");
    assert_eq!(v[0]["line"], 12);
    assert_eq!(v[0]["url"], gl_note_url(6));
    assert_eq!(v[0]["review_state"], Value::Null);
    assert_eq!(v[0]["association"], Value::Null);
}

#[test]
fn gitlab_diffnote_falls_back_to_old_line() {
    let mut n = note(6, "2026-01-01T10:00:00Z", json!("fix"));
    n["type"] = json!("DiffNote");
    n["position"] =
        json!({"new_path": "a.rs", "old_path": "a.rs", "new_line": null, "old_line": 7});
    let (_, v) = gl_comments(json!([n]));
    assert_eq!(v[0]["line"], 7);
}

#[test]
fn gitlab_diffnote_falls_back_to_old_path() {
    let mut n = note(6, "2026-01-01T10:00:00Z", json!("fix"));
    n["type"] = json!("DiffNote");
    n["position"] =
        json!({"new_path": null, "old_path": "gone.rs", "new_line": null, "old_line": 2});
    let (_, v) = gl_comments(json!([n]));
    assert_eq!(v[0]["path"], "gone.rs");
}

#[test]
fn gitlab_other_note_type_is_comment() {
    let mut n = note(6, "2026-01-01T10:00:00Z", json!("x"));
    n["type"] = json!("DiscussionNote");
    let (_, v) = gl_comments(json!([n]));
    assert_eq!(v[0]["kind"], "comment");
    assert_eq!(v[0]["path"], Value::Null);
    assert_eq!(v[0]["line"], Value::Null);
}

#[test]
fn gitlab_comments_sorted_oldest_first_from_desc_fetch() {
    let (_, v) = gl_comments(json!([
        note(3, "2026-01-01T12:00:00Z", json!("c")),
        note(2, "2026-01-01T11:00:00Z", json!("b")),
        note(1, "2026-01-01T10:00:00Z", json!("a"))
    ]));
    assert_eq!(
        ids(&v),
        vec![k("comment", 1), k("comment", 2), k("comment", 3)]
    );
}

#[test]
fn gitlab_comments_ties_keep_fetch_order() {
    let (_, v) = gl_comments(json!([
        note(9, "2026-01-01T10:00:00Z", json!("z")),
        note(4, "2026-01-01T10:00:00Z", json!("y")),
        note(7, "2026-01-01T09:00:00Z", json!("x"))
    ]));
    assert_eq!(
        ids(&v),
        vec![k("comment", 7), k("comment", 9), k("comment", 4)]
    );
}

#[test]
fn gitlab_comments_not_found_fails() {
    let mut e = gl();
    e.route_error(&get(GL_DISC), "404 Not Found");
    let out = e.forge(&["pr", "comments", GL_URL]);
    out.fails();
    assert!(out.stderr.contains("404 Not Found"), "{}", out.stderr);
}

// comments: GitHub

fn review(id: u64, state: &str, body: Value, at: &str) -> Value {
    json!({"id": id, "state": state, "body": body, "submitted_at": at,
           "html_url": format!("https://github.com/o/r/pull/3#pullrequestreview-{id}"),
           "user": {"login": "bob"}, "author_association": "MEMBER"})
}

fn issue_comment(id: u64, at: &str, body: Value) -> Value {
    json!({"id": id, "created_at": at, "body": body,
           "html_url": format!("https://github.com/o/r/pull/3#issuecomment-{id}"),
           "user": {"login": "carol"}, "author_association": "NONE"})
}

fn inline(id: u64, at: &str, review_id: Value) -> Value {
    json!({"id": id, "created_at": at, "body": "inl", "path": "src/x.rs", "line": 4,
           "original_line": 2, "pull_request_review_id": review_id,
           "html_url": format!("https://github.com/o/r/pull/3#discussion_r{id}"),
           "user": {"login": "dave"}, "author_association": "OWNER"})
}

fn gh_env(reviews: Value, issue: Value, inl: Value) -> Env {
    let mut e = gh();
    e.route(&get(GH_REVIEWS), reviews);
    e.route(&get(GH_ISSUE), issue);
    e.route(&get(GH_INLINE), inl);
    e
}

fn gh_comments(reviews: Value, issue: Value, inl: Value) -> Value {
    gh_env(reviews, issue, inl)
        .forge(&["pr", "comments", GH_URL])
        .json()
}

#[test]
fn github_comments_empty_is_empty_array() {
    assert_eq!(gh_comments(json!([]), json!([]), json!([])), json!([]));
}

#[test]
fn github_comments_use_paginate_slurp_on_all_three_lists_in_order() {
    let e = gh_env(json!([]), json!([]), json!([]));
    e.forge(&["pr", "comments", GH_URL]).json();
    assert_eq!(
        calls(&e),
        vec![
            call("GET", GH_REVIEWS, true),
            call("GET", GH_ISSUE, true),
            call("GET", GH_INLINE, true)
        ]
    );
    for c in e.api_calls() {
        assert_eq!(c["slurp"], json!(true), "{c}");
        assert_eq!(c["host"], "github.com");
    }
}

#[test]
fn github_issue_comment_mapping() {
    let v = gh_comments(
        json!([]),
        json!([issue_comment(11, "2026-01-01T10:00:00Z", json!("hi"))]),
        json!([]),
    );
    assert_eq!(
        v,
        json!([{
            "id": 11, "kind": "comment", "author": "carol", "association": "NONE",
            "created_at": "2026-01-01T10:00:00Z", "body": "hi",
            "url": "https://github.com/o/r/pull/3#issuecomment-11",
            "path": null, "line": null, "review_state": null, "thread": null
        }])
    );
}

#[test]
fn github_null_body_is_empty_string() {
    let v = gh_comments(
        json!([]),
        json!([issue_comment(11, "2026-01-01T10:00:00Z", Value::Null)]),
        json!([]),
    );
    assert_eq!(v[0]["body"], "");
}

#[test]
fn github_inline_mapping_and_line_fallback() {
    let mut a = inline(21, "2026-01-01T10:00:00Z", json!(5));
    let mut b = inline(22, "2026-01-01T10:01:00Z", Value::Null);
    b["line"] = Value::Null;
    a["author_association"] = json!("COLLABORATOR");
    let v = gh_comments(json!([]), json!([]), json!([a, b]));
    assert_eq!(
        v[0],
        json!({
            "id": 21, "kind": "inline", "author": "dave", "association": "COLLABORATOR",
            "created_at": "2026-01-01T10:00:00Z", "body": "inl",
            "url": "https://github.com/o/r/pull/3#discussion_r21",
            "path": "src/x.rs", "line": 4, "review_state": null, "thread": "21"
        })
    );
    assert_eq!(v[1]["line"], 2);
}

#[test]
fn github_review_mapping_state_lowercased() {
    let v = gh_comments(
        json!([review(
            31,
            "CHANGES_REQUESTED",
            json!("fix it"),
            "2026-01-01T10:00:00Z"
        )]),
        json!([]),
        json!([]),
    );
    assert_eq!(
        v,
        json!([{
            "id": 31, "kind": "review", "author": "bob", "association": "MEMBER",
            "created_at": "2026-01-01T10:00:00Z", "body": "fix it",
            "url": "https://github.com/o/r/pull/3#pullrequestreview-31",
            "path": null, "line": null, "review_state": "changes_requested", "thread": null
        }])
    );
}

#[test]
fn github_review_filters() {
    let t = "2026-01-01T10:00:00Z";
    let v = gh_comments(
        json!([
            review(1, "PENDING", json!("p"), t),
            review(2, "DISMISSED", json!("d"), t),
            review(3, "COMMENTED", json!(""), t),
            review(4, "COMMENTED", Value::Null, t),
            review(5, "COMMENTED", json!(" \n\t "), t),
            review(6, "COMMENTED", json!("real"), t),
            review(7, "APPROVED", json!(""), t),
            review(8, "CHANGES_REQUESTED", Value::Null, t)
        ]),
        json!([]),
        json!([]),
    );
    assert_eq!(
        ids(&v),
        vec![k("review", 6), k("review", 7), k("review", 8)]
    );
}

#[test]
fn github_approved_review_with_empty_body_kept() {
    let v = gh_comments(
        json!([review(7, "APPROVED", Value::Null, "2026-01-01T10:00:00Z")]),
        json!([]),
        json!([]),
    );
    assert_eq!(v[0]["review_state"], "approved");
    assert_eq!(v[0]["body"], "");
    assert_eq!(v[0]["kind"], "review");
}

#[test]
fn github_pending_review_and_its_inline_comment_dropped() {
    let t = "2026-01-01T10:00:00Z";
    let v = gh_comments(
        json!([review(100, "PENDING", json!("draft"), t)]),
        json!([issue_comment(11, "2026-01-01T10:05:00Z", json!("hi"))]),
        json!([inline(21, t, json!(100))]),
    );
    assert_eq!(ids(&v), vec![k("comment", 11)]);
}

#[test]
fn github_inline_of_other_reviews_kept() {
    let t = "2026-01-01T10:00:00Z";
    let v = gh_comments(
        json!([
            review(100, "PENDING", json!("draft"), t),
            review(101, "APPROVED", json!(""), t)
        ]),
        json!([]),
        json!([
            inline(21, t, json!(100)),
            inline(22, t, json!(101)),
            inline(23, t, Value::Null)
        ]),
    );
    assert_eq!(
        ids(&v),
        vec![k("review", 101), k("inline", 22), k("inline", 23)]
    );
}

#[test]
fn github_sorted_by_created_at_review_inline_issue() {
    let v = gh_comments(
        json!([review(1, "COMMENTED", json!("r"), "2026-01-01T10:01:00Z")]),
        json!([issue_comment(2, "2026-01-01T10:02:00Z", json!("i"))]),
        json!([inline(3, "2026-01-01T10:01:00Z", Value::Null)]),
    );
    assert_eq!(
        ids(&v),
        vec![k("review", 1), k("inline", 3), k("comment", 2)]
    );
}

#[test]
fn github_ties_keep_fetch_order_across_lists() {
    let t = "2026-01-01T10:00:00Z";
    let v = gh_comments(
        json!([review(1, "APPROVED", json!(""), t)]),
        json!([
            issue_comment(2, t, json!("i")),
            issue_comment(5, t, json!("j"))
        ]),
        json!([inline(3, t, Value::Null)]),
    );
    assert_eq!(
        ids(&v),
        vec![
            k("review", 1),
            k("comment", 2),
            k("comment", 5),
            k("inline", 3)
        ]
    );
}

#[test]
fn github_sorting_is_by_created_at_not_id() {
    let v = gh_comments(
        json!([]),
        json!([
            issue_comment(1, "2026-01-02T10:00:00Z", json!("late")),
            issue_comment(2, "2026-01-01T10:00:00Z", json!("early"))
        ]),
        json!([]),
    );
    assert_eq!(ids(&v), vec![k("comment", 2), k("comment", 1)]);
}

#[test]
fn github_lists_split_over_pages_are_flattened() {
    let mut e = gh();
    e.route(
        &get(GH_REVIEWS),
        json!({"__pages": [
            [review(1, "APPROVED", json!(""), "2026-01-01T10:00:00Z")],
            [review(2, "APPROVED", json!(""), "2026-01-01T10:01:00Z")]
        ]}),
    );
    e.route(
        &get(GH_ISSUE),
        json!({"__pages": [
            [issue_comment(3, "2026-01-01T10:02:00Z", json!("a"))],
            [],
            [issue_comment(4, "2026-01-01T10:03:00Z", json!("b"))]
        ]}),
    );
    e.route(
        &get(GH_INLINE),
        json!({"__pages": [
            [inline(5, "2026-01-01T10:04:00Z", Value::Null)],
            [inline(6, "2026-01-01T10:05:00Z", Value::Null)]
        ]}),
    );
    let v = e.forge(&["pr", "comments", GH_URL]).json();
    assert_eq!(
        ids(&v),
        vec![
            k("review", 1),
            k("review", 2),
            k("comment", 3),
            k("comment", 4),
            k("inline", 5),
            k("inline", 6)
        ]
    );
}

#[test]
fn github_pending_review_on_second_page_still_filters_inline() {
    let mut e = gh();
    e.route(
        &get(GH_REVIEWS),
        json!({"__pages": [
            [review(1, "APPROVED", json!(""), "2026-01-01T10:00:00Z")],
            [review(2, "PENDING", json!("d"), "2026-01-01T10:01:00Z")]
        ]}),
    );
    e.route(&get(GH_ISSUE), json!([]));
    e.route(
        &get(GH_INLINE),
        json!({"__pages": [
            [inline(5, "2026-01-01T10:04:00Z", json!(2))],
            [inline(6, "2026-01-01T10:05:00Z", json!(1))]
        ]}),
    );
    let v = e.forge(&["pr", "comments", GH_URL]).json();
    assert_eq!(ids(&v), vec![k("review", 1), k("inline", 6)]);
}

#[test]
fn github_comments_any_list_error_fails() {
    for bad in [GH_REVIEWS, GH_ISSUE, GH_INLINE] {
        let mut e = gh_env(json!([]), json!([]), json!([]));
        e.route_error(&get(bad), "HTTP 404: Not Found");
        e.forge(&["pr", "comments", GH_URL]).fails();
    }
}

// comments: threads

#[test]
fn gitlab_comments_thread_is_discussion_id_unless_individual_note() {
    let mut reply = note(2, "2026-01-01T11:00:00Z", json!("reply"));
    reply["type"] = json!("DiffNote");
    reply["position"] =
        json!({"new_path": "a.rs", "old_path": "a.rs", "new_line": 3, "old_line": null});
    let mut root = reply.clone();
    root["id"] = json!(1);
    root["created_at"] = json!("2026-01-01T10:00:00Z");
    let mut sys = note(3, "2026-01-01T12:00:00Z", json!("resolved"));
    sys["system"] = json!(true);
    let (e, v) = gl_discussions(json!([
        {"id": "abc123", "individual_note": false, "notes": [root, reply, sys]},
        single(note(4, "2026-01-01T13:00:00Z", json!("alone")))
    ]));
    let threads: Vec<(u64, Value)> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["id"].as_u64().unwrap(), c["thread"].clone()))
        .collect();
    assert_eq!(
        threads,
        vec![(1, json!("abc123")), (2, json!("abc123")), (4, Value::Null)]
    );
    assert_eq!(v[1]["kind"], "inline");
    assert_eq!(v[1]["url"], gl_note_url(2));
    assert_eq!(calls(&e), vec![call("GET", GL_DISC, true)]);
}

#[test]
fn github_inline_thread_is_root_id_for_replies_and_own_id_for_roots() {
    let root = inline(21, "2026-01-01T10:00:00Z", json!(5));
    let mut reply = inline(22, "2026-01-01T10:01:00Z", json!(5));
    reply["in_reply_to_id"] = json!(21);
    let v = gh_comments(
        json!([review(31, "COMMENTED", json!("r"), "2026-01-01T09:00:00Z")]),
        json!([issue_comment(11, "2026-01-01T11:00:00Z", json!("i"))]),
        json!([root, reply]),
    );
    let threads: Vec<(u64, Value)> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["id"].as_u64().unwrap(), c["thread"].clone()))
        .collect();
    assert_eq!(
        threads,
        vec![
            (31, Value::Null),
            (21, json!("21")),
            (22, json!("21")),
            (11, Value::Null)
        ]
    );
}

// job logs

#[test]
fn gitlab_job_log_prints_tail_stripped_from_trace() {
    let mut e = gl();
    e.route(
        "GET projects/g%2Fp/jobs/9/trace",
        json!({"__text": "l1\r\n\u{1b}[31;1mred\u{1b}[0m\r\nl3\r\nl4\r\n"}),
    );
    let out = e.forge(&["pr", "job-log", GL_URL, "9", "--tail", "3"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout.trim_end_matches('\n'), "red\nl3\nl4");
    assert_eq!(
        calls(&e),
        vec![call("GET", "projects/g%2Fp/jobs/9/trace", false)]
    );
    for c in e.api_calls() {
        assert_eq!(c["escapes"], json!(false), "{c}");
    }
}

#[test]
fn github_job_log_default_tail_is_40_lines() {
    let mut e = gh();
    let text: String = (1..=50).map(|i| format!("line{i}\n")).collect();
    e.route("GET repos/o/r/actions/jobs/9/logs", json!({"__text": text}));
    let out = e.forge(&["pr", "job-log", GH_URL, "9"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines.len(), 40, "{}", out.stdout);
    assert_eq!(lines[0], "line11");
    assert_eq!(lines[39], "line50");
    assert_eq!(
        calls(&e),
        vec![call("GET", "repos/o/r/actions/jobs/9/logs", false)]
    );
}

#[test]
fn github_job_log_allows_escape_sequences_and_strips_them() {
    let mut e = gh();
    e.route(
        "GET repos/o/r/actions/jobs/9/logs",
        json!({"__text": "l1\n\u{1b}[31;1mred\u{1b}[0m\nl3\n"}),
    );
    let out = e.forge(&["pr", "job-log", GH_URL, "9", "--tail", "3"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout.trim_end_matches('\n'), "l1\nred\nl3");
    assert!(!out.stdout.contains('\u{1b}'), "{}", out.stdout);
    let api = e.api_calls();
    assert_eq!(api.len(), 1);
    assert_eq!(api[0]["escapes"], json!(true), "{}", api[0]);
}

#[test]
fn job_log_read_error_fails() {
    let mut e = gl();
    e.route_error("GET projects/g%2Fp/jobs/9/trace", "404 Not Found");
    let out = e.forge(&["pr", "job-log", GL_URL, "9"]);
    assert_ne!(out.code, 0);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
    assert!(!out.stderr.is_empty());
}
