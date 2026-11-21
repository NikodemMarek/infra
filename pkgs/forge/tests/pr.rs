mod common;

use common::{merge, Env};
use serde_json::{json, Value};

const GL_URL: &str = "https://gitlab.example.com/g/p/-/merge_requests/7";
const GH_URL: &str = "https://github.com/o/r/pull/3";
const GL_VIEW: &str = "GET projects/g%2Fp/merge_requests/7";
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

fn gl_view(over: Value) -> Value {
    let mut e = Env::outside().cred(&["gitlab.example.com"]);
    e.route(GL_VIEW, merge(gl_raw(), over));
    e.forge(&["pr", "view", GL_URL]).json()
}

fn gh_view(over: Value) -> Value {
    let mut e = Env::outside().cred(&["github.com"]);
    e.route(GH_VIEW, merge(gh_raw(), over));
    e.forge(&["pr", "view", GH_URL]).json()
}

// GitLab mapping

#[test]
fn gitlab_open_mr() {
    assert_eq!(gl_view(json!({})), gl_exp());
}

#[test]
fn gitlab_locked_is_open() {
    assert_eq!(gl_view(json!({"state": "locked"})), gl_exp());
}

#[test]
fn gitlab_closed() {
    assert_eq!(
        gl_view(json!({"state": "closed"})),
        merge(gl_exp(), json!({"state": "closed"}))
    );
}

#[test]
fn gitlab_merged_lists_merge_then_squash_commit() {
    let v = gl_view(json!({
        "state": "merged", "merge_commit_sha": "m1", "squash_commit_sha": "s1"
    }));
    assert_eq!(
        v,
        merge(gl_exp(), json!({"state": "merged", "landed": ["m1", "s1"]}))
    );
}

#[test]
fn gitlab_landed_squash_only() {
    let v = gl_view(json!({"state": "merged", "squash_commit_sha": "s1"}));
    assert_eq!(v["landed"], json!(["s1"]));
}

#[test]
fn gitlab_landed_merge_only() {
    let v = gl_view(json!({"state": "merged", "merge_commit_sha": "m1"}));
    assert_eq!(v["landed"], json!(["m1"]));
}

#[test]
fn gitlab_landed_skips_empty_strings() {
    let v = gl_view(json!({
        "state": "merged", "merge_commit_sha": "", "squash_commit_sha": "s1"
    }));
    assert_eq!(v["landed"], json!(["s1"]));
    let v = gl_view(json!({
        "state": "merged", "merge_commit_sha": "", "squash_commit_sha": ""
    }));
    assert_eq!(v["landed"], json!([]));
}

#[test]
fn gitlab_empty_sha_is_null() {
    let v = gl_view(json!({"sha": ""}));
    assert_eq!(v["head_sha"], Value::Null);
    assert_eq!(v, merge(gl_exp(), json!({"head_sha": null})));
}

#[test]
fn gitlab_null_description_is_empty_body() {
    assert_eq!(gl_view(json!({"description": null}))["body"], "");
}

#[test]
fn gitlab_draft_prefixes_are_stripped() {
    for title in [
        "Draft: feat: x",
        "[Draft] feat: x",
        "(Draft) feat: x",
        "draft: feat: x",
        "DRAFT: feat: x",
        "[DRAFT]   feat: x",
        "(draft) feat: x",
        "Draft:   feat: x",
    ] {
        let v = gl_view(json!({"draft": true, "title": title}));
        assert_eq!(v["draft"], true, "{title}");
        assert_eq!(v["title"], "feat: x", "{title}");
    }
}

#[test]
fn gitlab_work_in_progress_is_draft() {
    let v = gl_view(json!({"work_in_progress": true, "title": "Draft: feat: x"}));
    assert_eq!(v["draft"], true);
    assert_eq!(v["title"], "feat: x");
}

#[test]
fn gitlab_draft_without_prefix_keeps_title() {
    let v = gl_view(json!({"draft": true}));
    assert_eq!(v["draft"], true);
    assert_eq!(v["title"], "feat: x");
}

#[test]
fn gitlab_only_a_leading_draft_marker_is_stripped() {
    let v = gl_view(json!({"draft": true, "title": "Draft: fix Draft: handling"}));
    assert_eq!(v["title"], "fix Draft: handling");
    let v = gl_view(json!({"draft": true, "title": "fix (Draft) handling"}));
    assert_eq!(v["title"], "fix (Draft) handling");
}

// GitHub mapping

#[test]
fn github_open_pr_ignores_test_merge_commit() {
    assert_eq!(gh_view(json!({})), gh_exp());
}

#[test]
fn github_merged_at_means_merged() {
    let v = gh_view(json!({"state": "closed", "merged_at": "2026-11-01T10:00:00Z"}));
    assert_eq!(
        v,
        merge(gh_exp(), json!({"state": "merged", "landed": ["mc1"]}))
    );
}

#[test]
fn github_closed_unmerged() {
    let v = gh_view(json!({"state": "closed"}));
    assert_eq!(v, merge(gh_exp(), json!({"state": "closed"})));
}

#[test]
fn github_draft() {
    let v = gh_view(json!({"draft": true}));
    assert_eq!(v, merge(gh_exp(), json!({"draft": true})));
}

// CRLF normalization

#[test]
fn gitlab_crlf_in_title_and_body_is_normalized() {
    let v = gl_view(json!({
        "draft": true, "title": "Draft: a\r\nb", "description": "x\r\ny\r\n\r\nz\rw"
    }));
    assert_eq!(v["title"], "a\nb");
    assert_eq!(v["draft"], true);
    assert_eq!(v["body"], "x\ny\n\nz\rw");
}

#[test]
fn github_crlf_in_title_and_body_is_normalized() {
    let v = gh_view(json!({"title": "a\r\nb", "body": "x\r\ny\r\n\r\nz\rw"}));
    assert_eq!(v["title"], "a\nb");
    assert_eq!(v["body"], "x\ny\n\nz\rw");
}

#[test]
fn list_crlf_is_normalized() {
    let mut e = gh_repo();
    e.route(
        "GET repos/o/r/pulls?state=all&sort=updated&direction=desc&per_page=100",
        json!([merge(gh_raw(), json!({"title": "a\r\nb", "body": "c\r\nd"}))]),
    );
    let v = e.forge(&["pr", "list"]).json();
    assert_eq!(v[0]["title"], "a\nb");
    assert_eq!(v[0]["body"], "c\nd");
}

// Null and missing fields

fn without(mut v: Value, path: &[&str]) -> Value {
    let (last, parents) = path.split_last().unwrap();
    let mut cur = &mut v;
    for p in parents {
        cur = &mut cur[*p];
    }
    cur.as_object_mut().unwrap().remove(*last);
    v
}

#[test]
fn github_null_body_is_empty_body() {
    assert_eq!(gh_view(json!({"body": null}))["body"], "");
}

#[test]
fn github_null_head_sha_is_null() {
    let v = gh_view(json!({"head": {"ref": "nm/x", "sha": null}}));
    assert_eq!(v, merge(gh_exp(), json!({"head_sha": null})));
}

#[test]
fn github_missing_head_sha_is_null() {
    let mut e = Env::outside().cred(&["github.com"]);
    e.route(GH_VIEW, without(gh_raw(), &["head", "sha"]));
    let v = e.forge(&["pr", "view", GH_URL]).json();
    assert_eq!(v, merge(gh_exp(), json!({"head_sha": null})));
}

#[test]
fn gitlab_null_sha_is_null() {
    let v = gl_view(json!({"sha": null}));
    assert_eq!(v, merge(gl_exp(), json!({"head_sha": null})));
}

#[test]
fn gitlab_missing_sha_is_null() {
    let mut e = Env::outside().cred(&["gitlab.example.com"]);
    e.route(GL_VIEW, without(gl_raw(), &["sha"]));
    let v = e.forge(&["pr", "view", GL_URL]).json();
    assert_eq!(v, merge(gl_exp(), json!({"head_sha": null})));
}

#[test]
fn github_merged_without_merge_commit_has_no_landed() {
    let v = gh_view(json!({
        "state": "closed", "merged_at": "2026-11-01T10:00:00Z", "merge_commit_sha": null
    }));
    assert_eq!(v["state"], "merged");
    assert_eq!(v["landed"], json!([]));
}

// pr view

#[test]
fn view_not_found_fails_with_api_error_text() {
    let mut e = Env::outside().cred(&["gitlab.example.com"]);
    e.route_error(GL_VIEW, "404 Not Found");
    let out = e.forge(&["pr", "view", GL_URL]);
    out.fails();
    assert!(out.stderr.contains("404 Not Found"), "{}", out.stderr);
    let mut e = Env::outside().cred(&["github.com"]);
    e.route_error(GH_VIEW, "HTTP 404: Not Found");
    let out = e.forge(&["pr", "view", GH_URL]);
    out.fails();
    assert!(out.stderr.contains("HTTP 404: Not Found"), "{}", out.stderr);
}

// pr list

fn gl_repo() -> Env {
    Env::in_repo(&[("origin", "git@gitlab.example.com:g/p.git")], "").cred(&["gitlab.example.com"])
}

fn gh_repo() -> Env {
    Env::in_repo(&[("origin", "https://github.com/o/r.git")], "").cred(&["github.com"])
}

fn only_call(e: &Env) -> Value {
    let calls = e.api_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    calls.into_iter().next().unwrap()
}

#[test]
fn gitlab_list_default_orders_by_updated_with_limit_100() {
    let mut e = gl_repo();
    let key =
        "GET projects/g%2Fp/merge_requests?state=all&order_by=updated_at&sort=desc&per_page=100";
    e.route(key, json!([gl_raw()]));
    let v = e.forge(&["pr", "list"]).json();
    assert_eq!(v, json!([gl_exp()]));
    let c = only_call(&e);
    assert_eq!(c["tool"], "glab");
    assert_eq!(c["host"], "gitlab.example.com");
    assert_eq!(c["method"], "GET");
    assert_eq!(c["path"], key.strip_prefix("GET ").unwrap());
}

#[test]
fn gitlab_list_head_orders_by_created_and_encodes_branch() {
    let mut e = gl_repo();
    e.route(
        "GET projects/g%2Fp/merge_requests?source_branch=nm%2Fx%26y&state=all&order_by=created_at&sort=desc&per_page=1",
        json!([gl_raw()]),
    );
    let v = e
        .forge(&["pr", "list", "--head", "nm/x&y", "--limit", "1"])
        .json();
    assert_eq!(v, json!([gl_exp()]));
    assert_eq!(only_call(&e)["method"], "GET");
}

#[test]
fn gitlab_list_keeps_api_order_and_maps_each() {
    let mut e = gl_repo();
    let second = merge(
        gl_raw(),
        json!({"iid": 5, "state": "merged", "merge_commit_sha": "m1",
               "title": "Draft: other", "draft": true,
               "web_url": "https://gitlab.example.com/g/p/-/merge_requests/5"}),
    );
    e.route(
        "GET projects/g%2Fp/merge_requests?state=all&order_by=updated_at&sort=desc&per_page=20",
        json!([gl_raw(), second]),
    );
    let v = e.forge(&["pr", "list", "--limit", "20"]).json();
    let a = v.as_array().unwrap();
    assert_eq!(a.len(), 2);
    assert_eq!(a[0], gl_exp());
    assert_eq!(a[1]["number"], 5);
    assert_eq!(a[1]["ref"], "!5");
    assert_eq!(a[1]["state"], "merged");
    assert_eq!(a[1]["draft"], true);
    assert_eq!(a[1]["title"], "other");
    assert_eq!(a[1]["landed"], json!(["m1"]));
}

#[test]
fn gitlab_list_empty() {
    let mut e = gl_repo();
    e.route(
        "GET projects/g%2Fp/merge_requests?state=all&order_by=updated_at&sort=desc&per_page=100",
        json!([]),
    );
    let out = e.forge(&["pr", "list"]);
    assert_eq!(out.json(), json!([]));
    assert_eq!(out.stdout, "[]\n");
}

#[test]
fn github_list_default_orders_by_updated_with_limit_100() {
    let mut e = gh_repo();
    let key = "GET repos/o/r/pulls?state=all&sort=updated&direction=desc&per_page=100";
    e.route(key, json!([gh_raw()]));
    let v = e.forge(&["pr", "list"]).json();
    assert_eq!(v, json!([gh_exp()]));
    let c = only_call(&e);
    assert_eq!(c["tool"], "gh");
    assert_eq!(c["host"], "github.com");
    assert_eq!(c["method"], "GET");
    assert_eq!(c["path"], key.strip_prefix("GET ").unwrap());
}

#[test]
fn github_list_head_orders_by_created_and_encodes_branch() {
    let mut e = gh_repo();
    e.route(
        "GET repos/o/r/pulls?head=o:nm%2Fx%26y&state=all&sort=created&direction=desc&per_page=1",
        json!([gh_raw()]),
    );
    let v = e
        .forge(&["pr", "list", "--head", "nm/x&y", "--limit", "1"])
        .json();
    assert_eq!(v, json!([gh_exp()]));
}

#[test]
fn gitlab_list_head_without_limit_uses_per_page_100() {
    let mut e = gl_repo();
    e.route(
        "GET projects/g%2Fp/merge_requests?source_branch=b&state=all&order_by=created_at&sort=desc&per_page=100",
        json!([gl_raw()]),
    );
    let v = e.forge(&["pr", "list", "--head", "b"]).json();
    assert_eq!(v, json!([gl_exp()]));
}

#[test]
fn github_list_head_without_limit_uses_per_page_100() {
    let mut e = gh_repo();
    e.route(
        "GET repos/o/r/pulls?head=o:b&state=all&sort=created&direction=desc&per_page=100",
        json!([gh_raw()]),
    );
    let v = e.forge(&["pr", "list", "--head", "b"]).json();
    assert_eq!(v, json!([gh_exp()]));
}

#[test]
fn github_list_maps_each_and_landed_only_when_merged() {
    let mut e = gh_repo();
    let merged = merge(
        gh_raw(),
        json!({"number": 2, "state": "closed", "merged_at": "2026-11-01T10:00:00Z",
               "html_url": "https://github.com/o/r/pull/2"}),
    );
    e.route(
        "GET repos/o/r/pulls?state=all&sort=updated&direction=desc&per_page=100",
        json!([gh_raw(), merged]),
    );
    let v = e.forge(&["pr", "list"]).json();
    let a = v.as_array().unwrap();
    assert_eq!(a.len(), 2);
    assert_eq!(a[0], gh_exp());
    assert_eq!(a[1]["state"], "merged");
    assert_eq!(a[1]["landed"], json!(["mc1"]));
    assert_eq!(a[1]["ref"], "#2");
}

#[test]
fn github_list_empty() {
    let mut e = gh_repo();
    e.route(
        "GET repos/o/r/pulls?state=all&sort=updated&direction=desc&per_page=100",
        json!([]),
    );
    assert_eq!(e.forge(&["pr", "list"]).stdout, "[]\n");
}

#[test]
fn list_limit_out_of_range_is_usage_error() {
    let e = gl_repo();
    for limit in ["0", "101", "-1", "abc", "1000"] {
        e.forge(&["pr", "list", "--limit", limit]).usage_error();
    }
    assert!(e.api_calls().is_empty());
}

#[test]
fn list_limit_bounds_are_accepted() {
    for limit in ["1", "100"] {
        let mut e = gl_repo();
        e.route(
            &format!(
                "GET projects/g%2Fp/merge_requests?state=all&order_by=updated_at&sort=desc&per_page={limit}"
            ),
            json!([]),
        );
        assert_eq!(e.forge(&["pr", "list", "--limit", limit]).json(), json!([]));
    }
}

#[test]
fn list_api_error_fails_with_its_text() {
    let mut e = gl_repo();
    e.route_error(
        "GET projects/g%2Fp/merge_requests?state=all&order_by=updated_at&sort=desc&per_page=100",
        "401 Unauthorized",
    );
    let out = e.forge(&["pr", "list"]);
    out.fails();
    assert!(out.stderr.contains("401 Unauthorized"), "{}", out.stderr);
}
