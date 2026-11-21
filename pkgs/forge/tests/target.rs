mod common;

use common::Env;
use serde_json::{json, Value};

fn repo_json(e: &Env, args: &[&str]) -> Value {
    e.forge(args).json()
}

fn target(forge: &str, host: &str, project: &str) -> Value {
    json!({"forge": forge, "host": host, "project": project})
}

#[test]
fn scp_like_gitlab_origin() {
    let e = Env::in_repo(&[("origin", "git@gitlab.example.com:g/sub/p.git")], "")
        .cred(&["gitlab.example.com"]);
    assert_eq!(
        repo_json(&e, &["repo"]),
        target("gitlab", "gitlab.example.com", "g/sub/p")
    );
}

#[test]
fn https_github_origin() {
    let e = Env::in_repo(&[("origin", "https://github.com/o/r")], "").cred(&["github.com"]);
    assert_eq!(
        repo_json(&e, &["repo"]),
        target("github", "github.com", "o/r")
    );
}

#[test]
fn https_trailing_slash_and_dot_git_dropped() {
    let e = Env::in_repo(&[("origin", "https://github.com/o/r/")], "").cred(&["github.com"]);
    assert_eq!(
        repo_json(&e, &["repo"]),
        target("github", "github.com", "o/r")
    );
    let e = Env::in_repo(&[("origin", "https://github.com/o/r.git")], "").cred(&["github.com"]);
    assert_eq!(
        repo_json(&e, &["repo"]),
        target("github", "github.com", "o/r")
    );
}

#[test]
fn https_port_dropped() {
    let e = Env::in_repo(&[("origin", "https://gitlab.example.com:8443/g/p.git")], "")
        .cred(&["gitlab.example.com"]);
    assert_eq!(
        repo_json(&e, &["repo"]),
        target("gitlab", "gitlab.example.com", "g/p")
    );
}

#[test]
fn ssh_url_with_port() {
    let e = Env::in_repo(&[("origin", "ssh://git@h:2222/g/p.git")], "").cred(&["h"]);
    assert_eq!(repo_json(&e, &["repo"]), target("gitlab", "h", "g/p"));
}

#[test]
fn git_scheme_url() {
    let e = Env::in_repo(&[("origin", "git://gitlab.example.com/g/p.git")], "")
        .cred(&["gitlab.example.com"]);
    assert_eq!(
        repo_json(&e, &["repo"]),
        target("gitlab", "gitlab.example.com", "g/p")
    );
}

#[test]
fn kind_defaults_to_github_when_host_contains_github_any_case() {
    let e = Env::in_repo(&[("origin", "git@MyGitHub.corp:o/r.git")], "").cred(&["MyGitHub.corp"]);
    assert_eq!(repo_json(&e, &["repo"])["forge"], "github");
}

#[test]
fn host_override_keeps_project_from_remote() {
    let e = Env::in_repo(
        &[("origin", "git@alias:g/p.git")],
        "[forge]\nhost = \"gitlab.work\"\n",
    )
    .cred(&["gitlab.work"]);
    assert_eq!(
        repo_json(&e, &["repo"]),
        target("gitlab", "gitlab.work", "g/p")
    );
}

#[test]
fn host_and_project_override_need_no_remote() {
    let e = Env::in_repo(&[], "[forge]\nhost = \"gitlab.work\"\nproject = \"x/y\"\n")
        .cred(&["gitlab.work"]);
    assert_eq!(
        repo_json(&e, &["repo"]),
        target("gitlab", "gitlab.work", "x/y")
    );
}

#[test]
fn host_and_project_override_ignore_missing_remote_name() {
    let e = Env::in_repo(&[], "[forge]\nhost = \"gitlab.work\"\nproject = \"x/y\"\n")
        .cred(&["gitlab.work"]);
    assert_eq!(
        repo_json(&e, &["--remote", "nope", "repo"]),
        target("gitlab", "gitlab.work", "x/y")
    );
}

#[test]
fn project_override_keeps_host_from_remote() {
    let e = Env::in_repo(
        &[("origin", "git@gitlab.example.com:g/p.git")],
        "[forge]\nproject = \"x/y\"\n",
    )
    .cred(&["gitlab.example.com"]);
    assert_eq!(
        repo_json(&e, &["repo"]),
        target("gitlab", "gitlab.example.com", "x/y")
    );
}

#[test]
fn kind_override_github() {
    let e = Env::in_repo(
        &[("origin", "git@git.example.com:o/r.git")],
        "[forge]\nkind = \"github\"\n",
    )
    .cred(&["git.example.com"]);
    assert_eq!(
        repo_json(&e, &["repo"]),
        target("github", "git.example.com", "o/r")
    );
}

#[test]
fn kind_override_beats_host_heuristic() {
    let e = Env::in_repo(
        &[("origin", "git@github.example.com:o/r.git")],
        "[forge]\nkind = \"gitlab\"\n",
    )
    .cred(&["github.example.com"]);
    assert_eq!(repo_json(&e, &["repo"])["forge"], "gitlab");
}

#[test]
fn unknown_kind_fails() {
    let e = Env::in_repo(
        &[("origin", "git@git.example.com:o/r.git")],
        "[forge]\nkind = \"gitea\"\n",
    )
    .cred(&["git.example.com"]);
    let out = e.forge(&["repo"]);
    out.fails();
    assert!(out.stderr.contains("unknown forge.kind"), "{}", out.stderr);
}

#[test]
fn named_remote_is_used() {
    let e = Env::in_repo(
        &[
            ("origin", "git@gitlab.example.com:g/p.git"),
            ("up", "https://github.com/o/r.git"),
        ],
        "",
    )
    .cred(&["gitlab.example.com", "github.com"]);
    assert_eq!(
        repo_json(&e, &["--remote", "up", "repo"]),
        target("github", "github.com", "o/r")
    );
}

#[test]
fn missing_remote_fails() {
    let e = Env::in_repo(&[("origin", "git@gitlab.example.com:g/p.git")], "")
        .cred(&["gitlab.example.com"]);
    let out = e.forge(&["--remote", "up", "repo"]);
    out.fails();
    assert!(
        out.stderr.contains("remote 'up' not found"),
        "{}",
        out.stderr
    );
}

#[test]
fn no_origin_remote_fails() {
    let e =
        Env::in_repo(&[("up", "git@gitlab.example.com:g/p.git")], "").cred(&["gitlab.example.com"]);
    e.forge(&["repo"]).fails();
}

#[test]
fn repo_outside_jj_repo_fails() {
    let e = Env::outside().cred(&["gitlab.example.com"]);
    e.forge(&["repo"]).fails();
}

#[test]
fn remote_flag_after_subcommand_is_usage_error() {
    let e = Env::in_repo(&[("origin", "git@gitlab.example.com:g/p.git")], "")
        .cred(&["gitlab.example.com"]);
    e.forge(&["pr", "list", "--remote", "origin"]).usage_error();
    assert!(e.api_calls().is_empty());
}

#[test]
fn unknown_subcommand_abbreviation_is_usage_error() {
    let e = Env::in_repo(&[("origin", "git@gitlab.example.com:g/p.git")], "")
        .cred(&["gitlab.example.com"]);
    e.forge(&["pr", "vi", "1"]).usage_error();
    assert!(e.api_calls().is_empty());
}

// URL targets

#[test]
fn gitlab_url_with_suffix_works_outside_a_repo() {
    let mut e = Env::outside().cred(&["gitlab.example.com"]);
    e.route(
        "GET projects/g%2Fp/merge_requests/7",
        json!({"iid": 7, "state": "opened", "title": "t", "description": "",
               "source_branch": "s", "target_branch": "main", "sha": "abc",
               "web_url": "https://gitlab.example.com/g/p/-/merge_requests/7"}),
    );
    let v = e
        .forge(&[
            "pr",
            "view",
            "https://gitlab.example.com/g/p/-/merge_requests/7/diffs",
        ])
        .json();
    assert_eq!(v["forge"], "gitlab");
    assert_eq!(v["ref"], "!7");
    let calls = e.api_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["tool"], "glab");
    assert_eq!(calls[0]["host"], "gitlab.example.com");
    assert_eq!(calls[0]["method"], "GET");
    assert_eq!(calls[0]["path"], "projects/g%2Fp/merge_requests/7");
}

#[test]
fn gitlab_url_with_subgroup_encodes_project() {
    let mut e = Env::outside().cred(&["gitlab.example.com"]);
    e.route(
        "GET projects/g%2Fsub%2Fp/merge_requests/2",
        json!({"iid": 2, "state": "opened", "title": "t", "description": "",
               "source_branch": "s", "target_branch": "main", "sha": "abc",
               "web_url": "u"}),
    );
    e.forge(&[
        "pr",
        "view",
        "https://gitlab.example.com/g/sub/p/-/merge_requests/2",
    ])
    .json();
    assert_eq!(e.api_calls().len(), 1);
}

#[test]
fn github_url_works_outside_a_repo() {
    let mut e = Env::outside().cred(&["github.com"]);
    e.route(
        "GET repos/o/r/pulls/3",
        json!({"number": 3, "state": "open", "merged_at": null, "draft": false,
               "title": "t", "body": "b", "base": {"ref": "main"},
               "head": {"ref": "s", "sha": "abc"}, "merge_commit_sha": null,
               "html_url": "https://github.com/o/r/pull/3"}),
    );
    let v = e
        .forge(&["pr", "view", "https://github.com/o/r/pull/3"])
        .json();
    assert_eq!(v["forge"], "github");
    assert_eq!(v["ref"], "#3");
    let calls = e.api_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["tool"], "gh");
    assert_eq!(calls[0]["host"], "github.com");
    assert_eq!(calls[0]["path"], "repos/o/r/pulls/3");
}

#[test]
fn github_url_with_suffix() {
    let mut e = Env::outside().cred(&["github.com"]);
    e.route(
        "GET repos/o/r/pulls/3",
        json!({"number": 3, "state": "open", "merged_at": null, "draft": false,
               "title": "t", "body": "b", "base": {"ref": "main"},
               "head": {"ref": "s", "sha": "abc"}, "merge_commit_sha": null,
               "html_url": "u"}),
    );
    e.forge(&["pr", "view", "https://github.com/o/r/pull/3/files"])
        .json();
}

#[test]
fn non_pr_url_fails() {
    let e = Env::outside().cred(&["example.com"]);
    e.forge(&["pr", "view", "https://example.com/x"]).fails();
    assert!(e.api_calls().is_empty());
}

#[test]
fn github_url_with_bad_owner_or_repo_fails_before_any_api_call() {
    for url in [
        "https://github.com//r/pull/3",
        "https://github.com/../../pull/1",
        "https://github.com/x://pull/1",
    ] {
        let e = Env::outside().cred(&["github.com"]);
        e.forge(&["pr", "view", url]).fails();
        assert!(e.api_calls().is_empty(), "{url}: {:?}", e.api_calls());
    }
}

#[test]
fn url_ignores_remote_flag_and_repo_config() {
    let mut e = Env::in_repo(
        &[("origin", "git@other.example.com:a/b.git")],
        "[forge]\nhost = \"gitlab.work\"\nproject = \"x/y\"\nkind = \"github\"\n",
    )
    .cred(&["gitlab.example.com"]);
    e.route(
        "GET projects/g%2Fp/merge_requests/7",
        json!({"iid": 7, "state": "opened", "title": "t", "description": "",
               "source_branch": "s", "target_branch": "main", "sha": "abc",
               "web_url": "u"}),
    );
    e.forge(&[
        "--remote",
        "missing",
        "pr",
        "view",
        "https://gitlab.example.com/g/p/-/merge_requests/7",
    ])
    .json();
    let calls = e.api_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["tool"], "glab");
    assert_eq!(calls[0]["host"], "gitlab.example.com");
}

// Credentials

#[test]
fn url_without_github_credential_fails_before_any_api_call() {
    let e = Env::outside().cred(&["github.com"]);
    let out = e.forge(&["pr", "view", "https://evil.example.com/o/r/pull/1"]);
    out.fails();
    assert!(
        out.stderr.contains("no gh credential for evil.example.com"),
        "{}",
        out.stderr
    );
    assert!(e.api_calls().is_empty());
}

#[test]
fn url_without_gitlab_credential_fails_before_any_api_call() {
    let e = Env::outside().cred(&["gitlab.example.com"]);
    let out = e.forge(&[
        "pr",
        "view",
        "https://evil.example.com/g/p/-/merge_requests/1",
    ]);
    out.fails();
    assert!(
        out.stderr
            .contains("no glab credential for evil.example.com"),
        "{}",
        out.stderr
    );
    assert!(e.api_calls().is_empty());
}

#[test]
fn remote_target_without_credential_fails_before_any_api_call() {
    let e = Env::in_repo(&[("origin", "git@gitlab.example.com:g/p.git")], "").cred(&[]);
    let out = e.forge(&["pr", "view", "7"]);
    out.fails();
    assert!(
        out.stderr
            .contains("no glab credential for gitlab.example.com"),
        "{}",
        out.stderr
    );
    assert!(e.api_calls().is_empty());
    let e = Env::in_repo(&[("origin", "https://github.com/o/r")], "").cred(&[]);
    e.forge(&["pr", "list"]).fails();
    assert!(e.api_calls().is_empty());
}

#[test]
fn host_override_without_credential_fails_before_any_api_call() {
    let e = Env::in_repo(
        &[("origin", "git@alias:g/p.git")],
        "[forge]\nhost = \"gitlab.work\"\n",
    )
    .cred(&["alias"]);
    let out = e.forge(&["pr", "view", "7"]);
    out.fails();
    assert!(
        out.stderr.contains("no glab credential for gitlab.work"),
        "{}",
        out.stderr
    );
    assert!(e.api_calls().is_empty());
}

#[test]
fn github_credential_is_checked_with_gh_auth_token() {
    let mut e = Env::outside().cred(&["github.com"]);
    e.route(
        "GET repos/o/r/pulls/3",
        json!({"number": 3, "state": "open", "merged_at": null, "draft": false,
               "title": "t", "body": "b", "base": {"ref": "main"},
               "head": {"ref": "s", "sha": "abc"}, "merge_commit_sha": null,
               "html_url": "u"}),
    );
    e.forge(&["pr", "view", "https://github.com/o/r/pull/3"])
        .json();
    let auth = e.auth_calls();
    assert!(!auth.is_empty());
    assert!(auth
        .iter()
        .all(|a| a["tool"] == "gh" && a["sub"] == "token" && a["host"] == "github.com"));
}

#[test]
fn gitlab_credential_is_checked_with_glab_auth_status() {
    let mut e = Env::outside().cred(&["gitlab.example.com"]);
    e.route(
        "GET projects/g%2Fp/merge_requests/7",
        json!({"iid": 7, "state": "opened", "title": "t", "description": "",
               "source_branch": "s", "target_branch": "main", "sha": "abc",
               "web_url": "u"}),
    );
    e.forge(&[
        "pr",
        "view",
        "https://gitlab.example.com/g/p/-/merge_requests/7",
    ])
    .json();
    let auth = e.auth_calls();
    assert!(!auth.is_empty());
    assert!(auth
        .iter()
        .all(|a| a["tool"] == "glab" && a["sub"] == "status" && a["host"] == "gitlab.example.com"));
}

#[test]
fn empty_gh_token_counts_as_no_credential() {
    let e = Env::outside().cred(&["github.com"]).empty_token();
    e.forge(&["pr", "view", "https://github.com/o/r/pull/3"])
        .fails();
    assert!(e.api_calls().is_empty());
}

// Digit arguments

#[test]
fn digit_pr_outside_a_repo_fails() {
    let e = Env::outside().cred(&["gitlab.example.com", "github.com"]);
    e.forge(&["pr", "view", "7"]).fails();
    assert!(e.api_calls().is_empty());
}

#[test]
fn pr_list_outside_a_repo_fails() {
    let e = Env::outside().cred(&["gitlab.example.com"]);
    e.forge(&["pr", "list"]).fails();
    assert!(e.api_calls().is_empty());
}

#[test]
fn digit_pr_uses_the_repo_remote() {
    let mut e = Env::in_repo(&[("origin", "git@gitlab.example.com:g/p.git")], "")
        .cred(&["gitlab.example.com"]);
    e.route(
        "GET projects/g%2Fp/merge_requests/7",
        json!({"iid": 7, "state": "opened", "title": "t", "description": "",
               "source_branch": "s", "target_branch": "main", "sha": "abc",
               "web_url": "u"}),
    );
    let v = e.forge(&["pr", "view", "7"]).json();
    assert_eq!(v["number"], 7);
    let calls = e.api_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["host"], "gitlab.example.com");
    assert_eq!(calls[0]["path"], "projects/g%2Fp/merge_requests/7");
}

#[test]
fn digit_pr_with_named_remote() {
    let mut e = Env::in_repo(
        &[
            ("origin", "git@gitlab.example.com:g/p.git"),
            ("up", "https://github.com/o/r.git"),
        ],
        "",
    )
    .cred(&["github.com"]);
    e.route(
        "GET repos/o/r/pulls/3",
        json!({"number": 3, "state": "open", "merged_at": null, "draft": false,
               "title": "t", "body": "b", "base": {"ref": "main"},
               "head": {"ref": "s", "sha": "abc"}, "merge_commit_sha": null,
               "html_url": "u"}),
    );
    let v = e.forge(&["--remote", "up", "pr", "view", "3"]).json();
    assert_eq!(v["forge"], "github");
    let calls = e.api_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["tool"], "gh");
}
