mod common;

use std::fs;
use std::path::Path;

use common::{age, Env, Out};
use serde_json::{json, Value};

fn is_stamp(name: &str, prefix: &str) -> bool {
    let rest = name.strip_prefix(prefix).unwrap_or("");
    rest.len() == 11
        && rest
            .chars()
            .enumerate()
            .all(|(i, c)| if i == 4 { c == '-' } else { c.is_ascii_digit() })
}

fn started(out: &Out) -> String {
    let lines = out.stderr_lines();
    let n = lines.len();
    assert!(n >= 2, "stderr: {}", out.stderr);
    let name = lines[n - 2]
        .strip_prefix("jw: started ")
        .and_then(|s| s.strip_suffix(" in repo"))
        .unwrap_or_else(|| panic!("no started line in: {}", out.stderr))
        .to_string();
    assert_eq!(lines[n - 1], format!("jw attach {name}"));
    name
}

fn claude_env(log: &str) -> Vec<&str> {
    log.lines().filter_map(|l| l.strip_prefix("env=")).collect()
}

fn claude_args(log: &str) -> Vec<&str> {
    log.lines().filter_map(|l| l.strip_prefix("arg=")).collect()
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

#[test]
fn n1_new_and_cd() {
    let e = Env::new();
    let repo = e.repo();

    let out = e.jw(&e.root, &["new", "repo"]);
    out.ok();
    let line = out.stdout.trim_end_matches('\n');
    assert!(!line.contains('\n'), "one line expected: {:?}", out.stdout);
    let ws = Path::new(line);
    assert_eq!(ws.parent().unwrap(), e.root.join("repo.agents"));
    let name = ws.file_name().unwrap().to_str().unwrap();
    assert!(is_stamp(name, "w-"), "workspace name: {name}");
    assert!(ws.join(".jj").is_dir());
    let listed = e.jj(&repo, &["workspace", "list", "-T", "name ++ \"\\n\""]);
    assert!(listed.lines().any(|l| l == name), "workspaces: {listed}");

    let out = e.jw(&repo, &["cd", name]);
    out.ok();
    assert_eq!(out.stdout, format!("{}\n", ws.display()));
}

#[test]
fn a1_agent_detached_start() {
    let e = Env::new();
    e.repo();

    let out = e.jw_env(
        &e.root,
        &[("CLAUDECODE", "1"), ("KNOWLEDGE_ORIGIN", "other")],
        &["agent", "-d", "repo", "hello", "world"],
    );
    out.ok();
    let name = started(&out);
    assert!(is_stamp(&name, "s-"), "session name: {name}");
    let ws = e.root.join("repo.agents").join(&name);
    assert!(ws.join(".jj").is_dir());

    let shpool = e.log("shpool.log");
    let attach: Vec<&str> = shpool
        .lines()
        .find(|l| l.contains("attach"))
        .unwrap_or_else(|| panic!("no attach in shpool log: {shpool}"))
        .split_whitespace()
        .collect();
    assert!(attach.contains(&"-b"), "{attach:?}");
    assert!(attach.contains(&ws.to_str().unwrap()), "{attach:?}");
    assert_eq!(attach.last(), Some(&name.as_str()));

    let log = e.log("claude.log");
    assert_eq!(claude_args(&log), ["--", "hello world"]);
    assert_eq!(
        log.lines().find_map(|l| l.strip_prefix("pwd=")),
        Some(ws.to_str().unwrap())
    );
    let env = claude_env(&log);
    assert!(
        env.contains(&format!("AGENT_SESSION={name}").as_str()),
        "{env:?}"
    );
    assert!(env.contains(&format!("SHPOOL_SESSION_NAME={name}").as_str()));
    assert!(!env.iter().any(|l| l.starts_with("CLAUDECODE=")));
    assert!(!env.iter().any(|l| l.starts_with("KNOWLEDGE_ORIGIN=")));

    let r = e.record(&name);
    assert_eq!(r["kind"], "task");
    assert_eq!(r["workspace"], ws.to_str().unwrap());
    assert_eq!(r["title"], "hello world");
    assert_eq!(r["description"], "hello world");
    assert!(r.get("status").is_none());

    let left: Vec<_> = fs::read_dir(&e.runtime)
        .unwrap()
        .map(|f| f.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("jw-agent-") && n != "jw-agent.lock")
        .collect();
    assert!(left.is_empty(), "job files left: {left:?}");
}

#[test]
fn a2_agent_dashdash_after_non_repo() {
    let e = Env::new();
    e.repo();

    e.jw(&e.root, &["agent", "-d", "nope", "--", "x"])
        .fail("jw: nope: not a repo");
    assert!(!e.logs.join("shpool.log").exists());
    assert!(!e.root.join("repo.agents").exists());
}

#[test]
fn a3_agent_leading_dashdash_is_all_prompt() {
    let e = Env::new();
    let repo = e.repo();

    let out = e.jw(&repo, &["agent", "-d", "--", "repo", "hello"]);
    out.ok();
    let name = started(&out);
    let log = e.log("claude.log");
    assert_eq!(claude_args(&log), ["--", "repo hello"]);
    assert_eq!(e.record(&name)["title"], "repo hello");
}

#[test]
fn e1_event_session_refusals() {
    let e = Env::new();
    let repo = e.repo();
    let ev = [
        ("KNOWLEDGE_ORIGIN", "event:gitlab-note"),
        ("AGENT_SESSION", "s1"),
    ];

    e.jw_env(&repo, &ev, &["agent", "-d", "repo", "hi"])
        .fail("jw: event sessions cannot start agents");
    assert!(!e.logs.join("shpool.log").exists());
    assert!(!e.root.join("repo.agents").exists());

    e.jw_env(&repo, &ev, &["session", "set", "--title", "x"])
        .fail("jw: event sessions cannot change session records");
    assert!(!e.record_path("s1").exists());

    e.jw_env(&repo, &ev, &["session", "status", "busy"]).ok();
    assert_eq!(e.record("s1")["status"], "busy");
    e.jw_env(&repo, &ev, &["session", "get"]).ok();
    e.jw_env(&repo, &ev, &["done"]).ok();
    assert_eq!(e.record("s1")["status"], "done");
}

#[test]
fn s1_session_set_get() {
    let e = Env::new();

    e.jw(&e.home, &["session", "get", "-s", "s1"])
        .fail("jw: no session record s1");
    e.jw(&e.home, &["session", "set", "--title", "x"])
        .fail("jw: no session: pass -s NAME or run in an agent session");

    e.jw(
        &e.home,
        &[
            "session",
            "set",
            "-s",
            "s1",
            "--kind",
            "task",
            "--title",
            "T",
            "--description",
            "D",
            "--url",
            "https://example.com/i/1",
            "--workspace",
            "/w",
            "--mr",
            "https://example.com/mr/1",
            "--mr",
            "https://example.com/mr/2",
            "--mr",
            "https://example.com/mr/1",
        ],
    )
    .ok();
    let got = e.jw(&e.home, &["session", "get", "-s", "s1"]).ok().json();
    assert_eq!(got["kind"], "task");
    assert_eq!(got["title"], "T");
    assert_eq!(got["description"], "D");
    assert_eq!(got["url"], "https://example.com/i/1");
    assert_eq!(got["workspace"], "/w");
    assert_eq!(
        got["mrs"],
        json!(["https://example.com/mr/1", "https://example.com/mr/2"])
    );

    e.jw(
        &e.home,
        &[
            "session",
            "set",
            "-s",
            "s1",
            "--mr",
            "https://example.com/mr/2",
            "--mr",
            "https://example.com/mr/3",
        ],
    )
    .ok();
    let got = e
        .jw_env(&e.home, &[("AGENT_SESSION", "s1")], &["session", "get"])
        .ok()
        .json();
    assert_eq!(got["title"], "T");
    assert_eq!(
        got["mrs"],
        json!([
            "https://example.com/mr/1",
            "https://example.com/mr/2",
            "https://example.com/mr/3"
        ])
    );

    e.jw(
        &e.home,
        &["session", "set", "-s", "s1", "--url", "http://example.com"],
    )
    .fail("jw: not an https url: http://example.com");
    assert_eq!(e.record("s1")["url"], "https://example.com/i/1");
}

#[test]
fn s1_session_set_keeps_record_written_by_python() {
    let e = Env::new();
    e.write_record(
        "py",
        r#"{"kind": "task", "title": "Old title", "workspace": "/old/ws", "status": "idle", "status_at": 1700000000.5, "notified": 1700000100.25, "cache_expires_at": 1700000200, "mrs": ["https://example.com/mr/1"], "unknown_key": {"x": [1, 2]}}"#,
    );

    let got = e.jw(&e.home, &["session", "get", "-s", "py"]).ok().json();
    assert_eq!(got["title"], "Old title");
    assert_eq!(got["workspace"], "/old/ws");

    e.jw(
        &e.home,
        &[
            "session",
            "set",
            "-s",
            "py",
            "--description",
            "new",
            "--mr",
            "https://example.com/mr/2",
        ],
    )
    .ok();
    let got = e.jw(&e.home, &["session", "get", "-s", "py"]).ok().json();
    assert_eq!(got["kind"], "task");
    assert_eq!(got["title"], "Old title");
    assert_eq!(got["workspace"], "/old/ws");
    assert_eq!(got["description"], "new");
    assert_eq!(got["status"], "idle");
    assert_eq!(num(&got["status_at"]), 1700000000.5);
    assert_eq!(num(&got["notified"]), 1700000100.25);
    assert_eq!(num(&got["cache_expires_at"]), 1700000200.0);
    assert_eq!(
        got["mrs"],
        json!(["https://example.com/mr/1", "https://example.com/mr/2"])
    );
}

#[test]
fn s1_session_set_ignores_wrong_typed_field() {
    let e = Env::new();
    e.write_record(
        "odd",
        r#"{"title": "Keep", "workspace": "/ws", "status_at": "garbage"}"#,
    );

    e.jw(
        &e.home,
        &["session", "set", "-s", "odd", "--description", "d"],
    )
    .ok();
    let got = e.record("odd");
    assert_eq!(got["title"], "Keep");
    assert_eq!(got["workspace"], "/ws");
    assert_eq!(got["description"], "d");
}

#[test]
fn s2_status_and_done() {
    let e = Env::new();
    let s1 = [("AGENT_SESSION", "s1")];

    e.jw(&e.home, &["session", "status", "busy"])
        .fail("jw: not in an agent session");
    e.jw(&e.home, &["done"]).fail("jw: not in an agent session");

    e.jw_env(&e.home, &s1, &["session", "status", "busy"]).ok();
    let r = e.record("s1");
    assert_eq!(r["status"], "busy");
    assert!(num(&r["status_at"]) > 1.6e9);
    assert!(r.get("waiting_for").is_none());
    assert!(r.get("cache_expires_at").is_none());

    e.jw_env(
        &e.home,
        &s1,
        &[
            "session",
            "status",
            "waiting",
            "--waiting-for",
            "need input",
            "--cache-expires-at",
            "1999999999",
        ],
    )
    .ok();
    let r = e.record("s1");
    assert_eq!(r["status"], "waiting");
    assert_eq!(r["waiting_for"], "need input");
    assert_eq!(num(&r["cache_expires_at"]), 1999999999.0);

    e.jw_env(
        &e.home,
        &s1,
        &["session", "status", "busy", "--waiting-for", "x"],
    )
    .fail("jw: --waiting-for needs waiting");

    e.jw_env(&e.home, &s1, &["session", "status", "idle"]).ok();
    let r = e.record("s1");
    assert_eq!(r["status"], "idle");
    assert!(r.get("waiting_for").is_none());
    assert!(r.get("cache_expires_at").is_none());

    let out = e.jw_env(&e.home, &s1, &["done"]);
    out.ok();
    assert!(
        out.stderr
            .contains("jw: s1 is done; it is stopped once it is idle and detached"),
        "{}",
        out.stderr
    );
    let r = e.record("s1");
    assert_eq!(r["status"], "done");
    assert!(num(&r["status_at"]) > 1.6e9);

    e.jw_env(&e.home, &s1, &["session", "status", "busy"]).ok();
    assert_eq!(e.record("s1")["status"], "done");
}

#[test]
fn i1_info() {
    let e = Env::new();
    let repo = e.repo();

    let out = e.jw(&repo, &["info"]);
    out.ok();
    let v = out.json();
    assert_eq!(v["repo"], "repo");
    assert_eq!(v["workspace"], "default");
    assert_eq!(v["root"], repo.to_str().unwrap());
    assert_eq!(v["subdir"], "");
    assert!(v["change"].as_str().is_some_and(|c| !c.is_empty()), "{v}");
    assert_eq!(v["stack"], json!({"index": 1, "size": 1}));

    fs::create_dir_all(repo.join("sub/dir")).unwrap();
    let v = e.jw(&repo.join("sub/dir"), &["info"]).ok().json();
    assert_eq!(v["root"], repo.to_str().unwrap());
    assert_eq!(v["subdir"], "sub/dir");

    let ws = e.workspace("w1");
    let v = e.jw(&ws, &["info"]).ok().json();
    assert_eq!(v["repo"], "repo");
    assert_eq!(v["workspace"], "w1");
    assert_eq!(v["root"], ws.to_str().unwrap());
    assert_eq!(v["subdir"], "");

    e.jj(&repo, &["new", "-m", "a"]);
    e.jj(&repo, &["new", "-m", "b"]);
    e.jj(&repo, &["edit", "@-"]);
    let v = e.jw(&repo, &["info"]).ok().json();
    assert_eq!(v["stack"], json!({"index": 2, "size": 3}));

    let out = e.jw(&e.home, &["info"]);
    assert_eq!(out.code, 1);
    assert_eq!(out.stdout, "");
}

#[test]
fn p1_prune_merged_and_unmerged() {
    let e = Env::new();
    e.repo();
    let merged = e.workspace("merged");
    let keep = e.workspace("keep");
    age(&merged);
    age(&keep);

    let out = e.jw(&e.root, &["prune", "--yes"]);
    out.ok();
    assert!(
        out.stderr.contains("jw: removed repo.agents/merged"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr
            .contains("jw: kept repo.agents/keep: unmerged work"),
        "{}",
        out.stderr
    );
    assert!(!merged.exists());
    assert!(keep.join(".jj").is_dir());
    let listed = e.jj(
        &e.root.join("repo"),
        &["workspace", "list", "-T", "name ++ \"\\n\""],
    );
    let names: Vec<&str> = listed.lines().collect();
    assert!(
        names.contains(&"keep") && !names.contains(&"merged"),
        "{names:?}"
    );
}
