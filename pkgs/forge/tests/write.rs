mod common;

use common::{merge, Env};
use serde_json::{json, Value};
use std::fs;

const GL_URL: &str = "https://gitlab.example.com/g/p/-/merge_requests/7";
const GH_URL: &str = "https://github.com/o/r/pull/3";
const GL_VIEW: &str = "GET projects/g%2Fp/merge_requests/7";
const GL_PUT: &str = "PUT projects/g%2Fp/merge_requests/7";
const GL_POST: &str = "POST projects/g%2Fp/merge_requests";
const GH_PATCH: &str = "PATCH repos/o/r/pulls/3";
const GH_POST: &str = "POST repos/o/r/pulls";
const GH_VIEW: &str = "GET repos/o/r/pulls/3";

fn gl_raw() -> Value {
    json!({
        "iid": 7, "state": "opened", "draft": false, "work_in_progress": false,
        "title": "feat: x", "description": "body",
        "source_branch": "nm/x", "target_branch": "main", "sha": "abc123",
        "merge_commit_sha": null, "squash_commit_sha": null,
        "web_url": GL_URL
    })
}

fn gl_exp() -> Value {
    json!({
        "forge": "gitlab", "number": 7, "ref": "!7", "url": GL_URL,
        "state": "open", "draft": false, "title": "feat: x", "body": "body",
        "base": "main", "head": "nm/x", "head_sha": "abc123", "landed": []
    })
}

fn gh_raw() -> Value {
    json!({
        "number": 3, "state": "open", "merged_at": null, "draft": false,
        "title": "feat: x", "body": "body",
        "base": {"ref": "main"}, "head": {"ref": "nm/x", "sha": "abc123"},
        "merge_commit_sha": "mc1", "html_url": GH_URL
    })
}

fn gh_exp() -> Value {
    json!({
        "forge": "github", "number": 3, "ref": "#3", "url": GH_URL,
        "state": "open", "draft": false, "title": "feat: x", "body": "body",
        "base": "main", "head": "nm/x", "head_sha": "abc123", "landed": []
    })
}

fn gl_env() -> Env {
    Env::outside().cred(&["gitlab.example.com"])
}

fn gh_env() -> Env {
    Env::outside().cred(&["github.com"])
}

fn gl_repo() -> Env {
    Env::in_repo(&[("origin", "git@gitlab.example.com:g/p.git")], "").cred(&["gitlab.example.com"])
}

fn gh_repo() -> Env {
    Env::in_repo(&[("origin", "https://github.com/o/r")], "").cred(&["github.com"])
}

fn writes(e: &Env) -> Vec<Value> {
    e.api_calls()
        .into_iter()
        .filter(|c| c["method"] != "GET")
        .collect()
}

fn body(call: &Value) -> Value {
    serde_json::from_str(call["body"].as_str().unwrap()).unwrap()
}

fn assert_json_write(call: &Value) {
    let h: Vec<String> = call["headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_ascii_lowercase().replace(' ', ""))
        .collect();
    assert!(
        h.iter().any(|x| x == "content-type:application/json"),
        "missing JSON Content-Type: {call}"
    );
    assert!(
        !call["body"].as_str().unwrap().is_empty(),
        "no body on stdin (--input -): {call}"
    );
}

// create

#[test]
fn gitlab_create() {
    let mut e = gl_repo();
    e.route(GL_POST, gl_raw());
    let out = e.forge(&[
        "pr", "create", "--head", "nm/x", "--base", "main", "--title", "feat: x", "--body", "body",
    ]);
    assert_eq!(out.json(), gl_exp());
    let w = writes(&e);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0]["method"], "POST");
    assert_eq!(w[0]["path"], "projects/g%2Fp/merge_requests");
    assert_eq!(w[0]["host"], "gitlab.example.com");
    assert_json_write(&w[0]);
    assert_eq!(
        body(&w[0]),
        json!({
            "source_branch": "nm/x", "target_branch": "main", "title": "feat: x",
            "description": "body", "remove_source_branch": true
        })
    );
}

#[test]
fn gitlab_create_draft() {
    let mut e = gl_repo();
    e.route(
        GL_POST,
        merge(gl_raw(), json!({"title": "Draft: feat: x", "draft": true})),
    );
    let out = e.forge(&[
        "pr", "create", "--head", "nm/x", "--base", "main", "--title", "feat: x", "--draft",
    ]);
    assert_eq!(out.json(), merge(gl_exp(), json!({"draft": true})));
    let b = body(&writes(&e)[0]);
    assert_eq!(b["title"], "Draft: feat: x");
    assert_eq!(b["remove_source_branch"], true);
}

#[test]
fn gitlab_create_without_body_sends_string_description() {
    let mut e = gl_repo();
    e.route(GL_POST, gl_raw());
    e.forge(&[
        "pr", "create", "--head", "nm/x", "--base", "main", "--title", "t",
    ])
    .json();
    let b = body(&writes(&e)[0]);
    assert!(b["description"].is_string(), "{b}");
}

#[test]
fn github_create() {
    let mut e = gh_repo();
    e.route(GH_POST, gh_raw());
    let out = e.forge(&[
        "pr", "create", "--head", "nm/x", "--base", "main", "--title", "feat: x", "--body", "body",
    ]);
    assert_eq!(out.json(), gh_exp());
    let w = writes(&e);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0]["method"], "POST");
    assert_eq!(w[0]["path"], "repos/o/r/pulls");
    assert_eq!(w[0]["host"], "github.com");
    assert_json_write(&w[0]);
    assert_eq!(
        body(&w[0]),
        json!({
            "head": "nm/x", "base": "main", "title": "feat: x", "body": "body", "draft": false
        })
    );
}

#[test]
fn github_create_draft_is_json_bool() {
    let mut e = gh_repo();
    e.route(GH_POST, merge(gh_raw(), json!({"draft": true})));
    let out = e.forge(&[
        "pr", "create", "--head", "nm/x", "--base", "main", "--title", "feat: x", "--draft",
    ]);
    assert_eq!(out.json(), merge(gh_exp(), json!({"draft": true})));
    let b = body(&writes(&e)[0]);
    assert_eq!(b["draft"], json!(true));
    assert_eq!(b["title"], "feat: x");
}

#[test]
fn github_create_without_body_sends_string_body() {
    let mut e = gh_repo();
    e.route(GH_POST, gh_raw());
    e.forge(&[
        "pr", "create", "--head", "nm/x", "--base", "main", "--title", "t",
    ])
    .json();
    let b = body(&writes(&e)[0]);
    assert!(b["body"].is_string(), "{b}");
}

#[test]
fn create_api_error_exits_1_with_text() {
    let mut e = gl_repo();
    e.route_error(GL_POST, "Another open merge request already exists");
    let out = e.forge(&[
        "pr", "create", "--head", "nm/x", "--base", "main", "--title", "t",
    ]);
    out.fails();
    assert!(out
        .stderr
        .contains("Another open merge request already exists"));
}

#[test]
fn create_dash_leading_values_are_sent_verbatim() {
    let mut e = gl_repo();
    e.route(GL_POST, gl_raw());
    e.forge(&[
        "pr",
        "create",
        "--head",
        "nm/x",
        "--base",
        "main",
        "--title",
        "-t",
        "--body",
        "- item\n- two",
    ])
    .json();
    let b = body(&writes(&e)[0]);
    assert_eq!(b["description"], "- item\n- two");
    assert_eq!(b["title"], "-t");
}

#[test]
fn edit_dash_leading_values_are_sent_verbatim() {
    let mut e = gh_env();
    e.route(GH_PATCH, gh_raw());
    e.forge(&["pr", "edit", GH_URL, "--body", "-x", "--title", "-t"])
        .json();
    assert_eq!(body(&writes(&e)[0]), json!({"title": "-t", "body": "-x"}));

    let mut e = gl_env();
    e.route(GL_VIEW, gl_raw());
    e.route(GL_PUT, gl_raw());
    e.forge(&["pr", "edit", GL_URL, "--body", "-x", "--title", "-t"])
        .json();
    assert_eq!(
        body(&writes(&e)[0]),
        json!({"title": "-t", "description": "-x"})
    );
}

#[test]
fn create_large_body_is_sent_intact() {
    let big = "0123456789abcdef".repeat(120 * 1024 / 16);
    for gitlab in [true, false] {
        let (mut e, route, key) = if gitlab {
            (gl_repo(), gl_raw(), "description")
        } else {
            (gh_repo(), gh_raw(), "body")
        };
        e.route(if gitlab { GL_POST } else { GH_POST }, route);
        e.forge(&[
            "pr", "create", "--head", "nm/x", "--base", "main", "--title", "t", "--body", &big,
        ])
        .json();
        let w = writes(&e);
        assert_eq!(w.len(), 1);
        let sent = body(&w[0]);
        assert_eq!(sent[key].as_str().unwrap().len(), big.len());
        assert_eq!(sent[key], big.as_str());
    }
}

#[test]
fn create_special_characters_are_sent_intact() {
    let text = "@leading at\nline \"quoted\" 'single' \\ back\n\tTab \u{e9}\u{4e16}\u{754c} \u{1f600}\n$HOME `x`\n";
    let mut e = gl_repo();
    e.route(GL_POST, gl_raw());
    e.forge(&[
        "pr", "create", "--head", "nm/x", "--base", "main", "--title", text, "--body", text,
    ])
    .json();
    let b = body(&writes(&e)[0]);
    assert_eq!(b["description"], text);
    assert_eq!(b["title"], text);

    let mut e = gh_repo();
    e.route(GH_POST, gh_raw());
    e.forge(&[
        "pr", "create", "--head", "nm/x", "--base", "main", "--title", "t", "--body", text,
    ])
    .json();
    assert_eq!(body(&writes(&e)[0])["body"], text);
}

#[test]
fn create_requires_flags() {
    let e = gl_repo();
    e.forge(&["pr", "create", "--head", "b", "--base", "main"])
        .usage_error();
    e.forge(&["pr", "create", "--base", "main", "--title", "t"])
        .usage_error();
    e.forge(&["pr", "create", "--head", "b", "--title", "t"])
        .usage_error();
    assert!(e.api_calls().is_empty());
}

#[test]
fn create_without_credential_makes_no_api_call() {
    let e = Env::in_repo(&[("origin", "git@gitlab.example.com:g/p.git")], "");
    e.forge(&[
        "pr", "create", "--head", "nm/x", "--base", "main", "--title", "t",
    ])
    .fails();
    assert!(e.api_calls().is_empty());
}

// edit

#[test]
fn edit_without_flags_is_view_and_writes_nothing() {
    let mut e = gl_env();
    e.route(GL_VIEW, gl_raw());
    assert_eq!(e.forge(&["pr", "edit", GL_URL]).json(), gl_exp());
    assert!(writes(&e).is_empty());

    let mut e = gh_env();
    e.route(GH_VIEW, gh_raw());
    assert_eq!(e.forge(&["pr", "edit", GH_URL]).json(), gh_exp());
    assert!(writes(&e).is_empty());
}

#[test]
fn gitlab_edit_sends_only_given_fields() {
    let mut e = gl_env();
    e.route(GL_PUT, merge(gl_raw(), json!({"target_branch": "dev"})));
    let out = e.forge(&["pr", "edit", GL_URL, "--base", "dev"]);
    assert_eq!(out.json(), merge(gl_exp(), json!({"base": "dev"})));
    assert!(e.api_calls().iter().all(|c| c["method"] != "GET"));
    let w = writes(&e);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0]["method"], "PUT");
    assert_eq!(w[0]["path"], "projects/g%2Fp/merge_requests/7");
    assert_json_write(&w[0]);
    assert_eq!(body(&w[0]), json!({"target_branch": "dev"}));
}

#[test]
fn gitlab_edit_all_fields() {
    let mut e = gl_env();
    e.route(GL_VIEW, gl_raw());
    e.route(
        GL_PUT,
        merge(
            gl_raw(),
            json!({"title": "new", "description": "nb", "target_branch": "dev"}),
        ),
    );
    let out = e.forge(&[
        "pr", "edit", GL_URL, "--title", "new", "--base", "dev", "--body", "nb",
    ]);
    assert_eq!(
        out.json(),
        merge(
            gl_exp(),
            json!({"title": "new", "body": "nb", "base": "dev"})
        )
    );
    assert_eq!(
        body(&writes(&e)[0]),
        json!({"title": "new", "target_branch": "dev", "description": "nb"})
    );
}

#[test]
fn gitlab_edit_empty_body_is_sent() {
    let mut e = gl_env();
    e.route(GL_VIEW, gl_raw());
    e.route(GL_PUT, merge(gl_raw(), json!({"description": ""})));
    let out = e.forge(&["pr", "edit", GL_URL, "--body", ""]);
    assert_eq!(out.json()["body"], "");
    assert_eq!(body(&writes(&e)[0]), json!({"description": ""}));
}

#[test]
fn gitlab_edit_draft_title_keeps_draft_prefix() {
    let mut e = gl_env();
    e.route(
        GL_VIEW,
        merge(gl_raw(), json!({"title": "Draft: old", "draft": true})),
    );
    e.route(
        GL_PUT,
        merge(gl_raw(), json!({"title": "Draft: new", "draft": true})),
    );
    let out = e.forge(&["pr", "edit", GL_URL, "--title", "new"]);
    assert_eq!(
        out.json(),
        merge(gl_exp(), json!({"title": "new", "draft": true}))
    );
    assert_eq!(body(&writes(&e)[0]), json!({"title": "Draft: new"}));
}

#[test]
fn gitlab_edit_draft_title_with_prefix_is_not_doubled() {
    let mut e = gl_env();
    e.route(
        GL_VIEW,
        merge(gl_raw(), json!({"title": "Draft: old", "draft": true})),
    );
    e.route(
        GL_PUT,
        merge(gl_raw(), json!({"title": "Draft: foo", "draft": true})),
    );
    let out = e.forge(&["pr", "edit", GL_URL, "--title", "Draft: foo"]);
    assert_eq!(out.json()["title"], "foo");
    assert_eq!(body(&writes(&e)[0]), json!({"title": "Draft: foo"}));
}

#[test]
fn gitlab_edit_work_in_progress_counts_as_draft() {
    let mut e = gl_env();
    let raw = json!({"title": "Draft: old", "draft": false, "work_in_progress": true});
    e.route(GL_VIEW, merge(gl_raw(), raw));
    e.route(
        GL_PUT,
        merge(
            gl_raw(),
            json!({"title": "Draft: S", "draft": false, "work_in_progress": true}),
        ),
    );
    let out = e.forge(&["pr", "edit", GL_URL, "--title", "S"]);
    assert_eq!(out.json()["title"], "S");
    assert_eq!(body(&writes(&e)[0]), json!({"title": "Draft: S"}));
}

#[test]
fn gitlab_edit_non_draft_title_with_prefix_as_is() {
    let mut e = gl_env();
    e.route(GL_VIEW, gl_raw());
    e.route(GL_PUT, merge(gl_raw(), json!({"title": "Draft: foo"})));
    e.forge(&["pr", "edit", GL_URL, "--title", "Draft: foo"]);
    assert_eq!(body(&writes(&e)[0]), json!({"title": "Draft: foo"}));
}

#[test]
fn gitlab_edit_non_draft_title_as_is() {
    let mut e = gl_env();
    e.route(GL_VIEW, gl_raw());
    e.route(GL_PUT, merge(gl_raw(), json!({"title": "new"})));
    e.forge(&["pr", "edit", GL_URL, "--title", "new"]).json();
    assert_eq!(body(&writes(&e)[0]), json!({"title": "new"}));
}

#[test]
fn github_edit_sends_only_given_fields() {
    let mut e = gh_env();
    e.route(GH_PATCH, merge(gh_raw(), json!({"title": "new"})));
    let out = e.forge(&["pr", "edit", GH_URL, "--title", "new"]);
    assert_eq!(out.json(), merge(gh_exp(), json!({"title": "new"})));
    let w = writes(&e);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0]["method"], "PATCH");
    assert_eq!(w[0]["path"], "repos/o/r/pulls/3");
    assert_json_write(&w[0]);
    assert_eq!(body(&w[0]), json!({"title": "new"}));
}

#[test]
fn github_edit_all_fields() {
    let mut e = gh_env();
    e.route(
        GH_PATCH,
        merge(
            gh_raw(),
            json!({"title": "new", "body": "nb", "base": {"ref": "dev"}}),
        ),
    );
    let out = e.forge(&[
        "pr", "edit", GH_URL, "--title", "new", "--base", "dev", "--body", "nb",
    ]);
    assert_eq!(
        out.json(),
        merge(
            gh_exp(),
            json!({"title": "new", "body": "nb", "base": "dev"})
        )
    );
    assert_eq!(
        body(&writes(&e)[0]),
        json!({"title": "new", "base": "dev", "body": "nb"})
    );
}

#[test]
fn edit_without_credential_makes_no_api_call() {
    let e = Env::outside();
    e.forge(&["pr", "edit", GL_URL, "--title", "x"]).fails();
    e.forge(&["pr", "edit", GH_URL, "--title", "x"]).fails();
    assert!(e.api_calls().is_empty());
}

// close / reopen

#[test]
fn gitlab_close_and_reopen() {
    for (cmd, event, state, exp) in [
        ("close", "close", "closed", "closed"),
        ("reopen", "reopen", "opened", "open"),
    ] {
        let mut e = gl_env();
        e.route(GL_PUT, merge(gl_raw(), json!({"state": state})));
        let out = e.forge(&["pr", cmd, GL_URL]);
        assert_eq!(out.json(), merge(gl_exp(), json!({"state": exp})));
        let w = writes(&e);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0]["method"], "PUT");
        assert_eq!(w[0]["path"], "projects/g%2Fp/merge_requests/7");
        assert_json_write(&w[0]);
        assert_eq!(body(&w[0]), json!({"state_event": event}));
    }
}

#[test]
fn github_close_and_reopen() {
    for (cmd, state) in [("close", "closed"), ("reopen", "open")] {
        let mut e = gh_env();
        e.route(GH_PATCH, merge(gh_raw(), json!({"state": state})));
        let out = e.forge(&["pr", cmd, GH_URL]);
        assert_eq!(out.json(), merge(gh_exp(), json!({"state": state})));
        let w = writes(&e);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0]["method"], "PATCH");
        assert_eq!(w[0]["path"], "repos/o/r/pulls/3");
        assert_json_write(&w[0]);
        assert_eq!(body(&w[0]), json!({"state": state}));
    }
}

#[test]
fn refused_reopen_exits_1_with_error_text() {
    let mut e = gh_env();
    e.route_error(GH_PATCH, "Cannot reopen: head branch was force-pushed");
    let out = e.forge(&["pr", "reopen", GH_URL]);
    out.fails();
    assert!(out
        .stderr
        .contains("Cannot reopen: head branch was force-pushed"));

    let mut e = gl_env();
    e.route_error(GL_PUT, "Cannot reopen merged MR");
    let out = e.forge(&["pr", "reopen", GL_URL]);
    out.fails();
    assert!(out.stderr.contains("Cannot reopen merged MR"));
    assert!(out.stderr.starts_with("forge: "));
}

#[test]
fn close_by_number_uses_repo_remote() {
    let mut e = gl_repo();
    e.route(GL_PUT, merge(gl_raw(), json!({"state": "closed"})));
    e.forge(&["pr", "close", "7"]).json();
    assert_eq!(writes(&e)[0]["path"], "projects/g%2Fp/merge_requests/7");
}

#[test]
fn close_reopen_without_credential_make_no_api_call() {
    let e = Env::outside();
    e.forge(&["pr", "close", GL_URL]).fails();
    e.forge(&["pr", "reopen", GH_URL]).fails();
    assert!(e.api_calls().is_empty());
}

// upload

fn file_in(e: &Env, name: &str) -> String {
    let p = e.dir.join(name);
    fs::write(&p, b"data").unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn gitlab_upload_returns_url() {
    let mut e = gl_repo();
    e.route(
        "POST projects/g%2Fp/uploads",
        json!({"alt": "a", "url": "/uploads/h/a.png", "markdown": "![a](/uploads/h/a.png)"}),
    );
    let f = file_in(&e, "a.png");
    let out = e.forge(&["upload", &f]);
    assert_eq!(out.json(), json!({"url": "/uploads/h/a.png"}));
    let calls = e.api_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["method"], "POST");
    assert_eq!(calls[0]["path"], "projects/g%2Fp/uploads");
    assert_eq!(calls[0]["host"], "gitlab.example.com");
    assert_eq!(calls[0]["form"], format!("file=@{f}"));
}

#[test]
fn upload_on_github_fails_without_api_call() {
    let e = gh_repo();
    let f = file_in(&e, "a.png");
    let out = e.forge(&["upload", &f]);
    out.fails();
    assert!(out.stderr.contains("github has no file upload API"));
    assert!(e.api_calls().is_empty());
}

#[test]
fn upload_missing_file_fails_without_api_call() {
    let e = gl_repo();
    let missing = e.dir.join("nope.png");
    let out = e.forge(&["upload", missing.to_str().unwrap()]);
    out.fails();
    assert!(out.stderr.contains("nope.png"), "{}", out.stderr);
    assert!(e.api_calls().is_empty());
}

#[test]
fn upload_without_credential_makes_no_api_call() {
    let e = Env::in_repo(&[("origin", "git@gitlab.example.com:g/p.git")], "");
    let f = file_in(&e, "a.png");
    e.forge(&["upload", &f]).fails();
    assert!(e.api_calls().is_empty());
}

#[test]
fn upload_requires_file_argument() {
    gl_repo().forge(&["upload"]).usage_error();
}
