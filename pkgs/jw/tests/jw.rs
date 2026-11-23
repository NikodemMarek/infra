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
        &[("CLAUDECODE", "1"), ("AGENT_ORIGIN", "other")],
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
    assert!(!env.iter().any(|l| l.starts_with("AGENT_ORIGIN=")));

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
fn e2_knowledge_origin_alone_is_not_event_session() {
    let e = Env::new();
    let repo = e.repo();
    e.jw_env(
        &repo,
        &[("KNOWLEDGE_ORIGIN", "event:x")],
        &["agent", "-d", "repo", "hi"],
    )
    .ok();
    assert!(e.logs.join("shpool.log").exists());
}

#[test]
fn a4_agent_origin_not_passed_to_claude() {
    let e = Env::new();
    e.repo();
    e.jw_env(
        &e.root,
        &[("AGENT_ORIGIN", "other")],
        &["agent", "-d", "repo", "hi"],
    )
    .ok();
    let log = e.log("claude.log");
    assert!(!claude_env(&log)
        .iter()
        .any(|l| l.starts_with("AGENT_ORIGIN=")));
}

#[test]
fn a5_agent_repo_from_first_word() {
    let e = Env::new();
    e.repo();

    let out = e.jw(&e.root, &["agent", "-d", "--", "repo hello"]);
    out.ok();
    let name = started(&out);
    assert!(e.root.join("repo.agents").join(&name).join(".jj").is_dir());
    let log = e.log("claude.log");
    assert_eq!(claude_args(&log), ["--", "repo hello"]);
    assert!(!e.logs.join("knowledge.log").exists());
}

#[test]
fn a6_agent_no_repo_fails() {
    let e = Env::new();
    e.repo();
    let msg = "jw: no repo: pass one or start the task with its name";

    e.jw(&e.root, &["agent", "-d", "--", "hello"]).fail(msg);
    e.jw(&e.root, &["agent", "-d"]).fail(msg);
    assert!(!e.logs.join("shpool.log").exists());
    assert!(!e.root.join("repo.agents").exists());
    assert!(!e.logs.join("knowledge.log").exists());
}

#[test]
fn e1_event_session_refusals() {
    let e = Env::new();
    let repo = e.repo();
    let ev = [
        ("AGENT_ORIGIN", "event:gitlab-note"),
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
fn s1_mr_marks_round_trip() {
    let e = Env::new();
    e.write_record(
        "mk",
        r#"{"title": "T", "mrs": ["https://example.com/mr/1"], "mr_marks": {"https://example.com/mr/1": {"failed_sha": "abc123", "seen": ["comment:1", "inline:2"]}, "https://example.com/mr/2": {"seen": []}}}"#,
    );
    let want = json!({
        "https://example.com/mr/1": {"failed_sha": "abc123", "seen": ["comment:1", "inline:2"]},
        "https://example.com/mr/2": {"seen": []}
    });
    let got = e.jw(&e.home, &["session", "get", "-s", "mk"]).ok().json();
    assert_eq!(got["mr_marks"], want);

    e.jw(
        &e.home,
        &["session", "set", "-s", "mk", "--description", "d"],
    )
    .ok();
    let got = e.record("mk");
    assert_eq!(got["mr_marks"], want);
    assert_eq!(got["description"], "d");
}

#[test]
fn s1_mr_marks_omitted_when_empty() {
    let e = Env::new();
    e.write_record("nm", r#"{"title": "T"}"#);
    let got = e.jw(&e.home, &["session", "get", "-s", "nm"]).ok().json();
    assert!(got.get("mr_marks").is_none(), "{got}");
}

#[test]
fn s1_mr_marks_tolerant_parsing() {
    let e = Env::new();
    e.write_record(
        "t1",
        r#"{"title": "Keep", "mr_marks": ["not", "an", "object"]}"#,
    );
    let got = e.jw(&e.home, &["session", "get", "-s", "t1"]).ok().json();
    assert_eq!(got["title"], "Keep");
    assert!(got.get("mr_marks").is_none(), "{got}");

    e.write_record(
        "t2",
        r#"{"title": "Keep", "mr_marks": {
            "https://e/1": "garbage",
            "https://e/2": {"failed_sha": 5, "seen": ["a", 3, null, "b"]},
            "https://e/3": {"failed_sha": "s"},
            "https://e/4": {"seen": "nope"},
            "https://e/5": {"failed_sha": "x", "seen": {"a": 1}}
        }}"#,
    );
    let got = e.jw(&e.home, &["session", "get", "-s", "t2"]).ok().json();
    assert_eq!(got["title"], "Keep");
    assert_eq!(
        got["mr_marks"],
        json!({
            "https://e/2": {"seen": ["a", "b"]},
            "https://e/3": {"failed_sha": "s", "seen": []},
            "https://e/4": {"seen": []},
            "https://e/5": {"failed_sha": "x", "seen": []}
        })
    );
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

fn s1_env() -> [(&'static str, &'static str); 1] {
    [("AGENT_SESSION", "s1")]
}

fn dir_str(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn w1_worktree_refusals() {
    let e = Env::new();
    let plain = e.dir.join("plain");
    fs::create_dir_all(&plain).unwrap();
    fs::write(e.dir.join("afile"), "x").unwrap();

    e.jw(&e.home, &["session", "worktree", dir_str(&plain)])
        .fail("jw: not in an agent session");
    e.jw_env(
        &e.home,
        &[("AGENT_SESSION", "")],
        &["session", "worktree", dir_str(&plain)],
    )
    .fail("jw: not in an agent session");

    e.jw_env(&e.home, &s1_env(), &["session", "worktree", "rel/dir"])
        .fail("jw: not an absolute directory: rel/dir");
    fs::create_dir_all(e.home.join("rel/dir")).unwrap();
    e.jw_env(&e.home, &s1_env(), &["session", "worktree", "rel/dir"])
        .fail("jw: not an absolute directory: rel/dir");

    let missing = e.dir.join("missing");
    e.jw_env(
        &e.home,
        &s1_env(),
        &["session", "worktree", dir_str(&missing)],
    )
    .fail(&format!(
        "jw: not an absolute directory: {}",
        missing.display()
    ));
    let file = e.dir.join("afile");
    e.jw_env(&e.home, &s1_env(), &["session", "worktree", dir_str(&file)])
        .fail(&format!(
            "jw: not an absolute directory: {}",
            file.display()
        ));

    assert!(!e.record_path("s1").exists());
}

#[test]
fn w2_worktree_in_event_session() {
    let e = Env::new();
    let d = e.dir.join("wt");
    fs::create_dir_all(&d).unwrap();
    let out = e.jw_env(
        &e.home,
        &[
            ("AGENT_ORIGIN", "event:gitlab-note"),
            ("AGENT_SESSION", "s1"),
        ],
        &["session", "worktree", dir_str(&d)],
    );
    out.ok();
    assert_eq!(out.stdout, "");
    assert_eq!(e.record("s1")["worktrees"], json!([dir_str(&d)]));
}

#[test]
fn w3_worktree_dedupe_order_and_other_fields() {
    let e = Env::new();
    e.write_record(
        "s1",
        r#"{"kind": "task", "title": "Keep", "status": "busy", "workspace": "/ws"}"#,
    );
    let a = e.dir.join("wt/a");
    let b = e.dir.join("wt/b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();

    for d in [&b, &a, &b, &a] {
        let out = e.jw_env(&e.home, &s1_env(), &["session", "worktree", dir_str(d)]);
        out.ok();
        assert_eq!(out.stdout, "");
    }
    let r = e.record("s1");
    assert_eq!(r["worktrees"], json!([dir_str(&b), dir_str(&a)]));
    assert_eq!(r["title"], "Keep");
    assert_eq!(r["status"], "busy");
    assert_eq!(r["workspace"], "/ws");

    let got = e
        .jw_env(&e.home, &s1_env(), &["session", "get"])
        .ok()
        .json();
    assert_eq!(got["worktrees"], json!([dir_str(&b), dir_str(&a)]));
}

#[test]
fn w4_worktree_stores_canonical_path() {
    let e = Env::new();
    let real = e.dir.join("real");
    fs::create_dir_all(&real).unwrap();
    let link = e.dir.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    e.jw_env(&e.home, &s1_env(), &["session", "worktree", dir_str(&link)])
        .ok();
    e.jw_env(&e.home, &s1_env(), &["session", "worktree", dir_str(&real)])
        .ok();
    assert_eq!(e.record("s1")["worktrees"], json!([dir_str(&real)]));
}

#[test]
fn w5_wrong_typed_worktrees_ignored_record_kept() {
    let e = Env::new();
    e.write_record(
        "odd",
        r#"{"title": "Keep", "workspace": "/ws", "worktrees": 5}"#,
    );
    let got = e.jw(&e.home, &["session", "get", "-s", "odd"]).ok().json();
    assert_eq!(got["title"], "Keep");
    assert!(
        got.get("worktrees").is_none_or(|w| w == &json!([])),
        "{got}"
    );

    e.jw(
        &e.home,
        &["session", "set", "-s", "odd", "--description", "d"],
    )
    .ok();
    let r = e.record("odd");
    assert_eq!(r["title"], "Keep");
    assert_eq!(r["workspace"], "/ws");
    assert_eq!(r["description"], "d");

    let d = e.dir.join("wt");
    fs::create_dir_all(&d).unwrap();
    e.jw_env(
        &e.home,
        &[("AGENT_SESSION", "odd")],
        &["session", "worktree", dir_str(&d)],
    )
    .ok();
    let r = e.record("odd");
    assert_eq!(r["title"], "Keep");
    assert_eq!(r["worktrees"], json!([dir_str(&d)]));
}

#[test]
fn w6_done_message_mentions_worktrees() {
    let e = Env::new();
    let out = e.jw_env(&e.home, &s1_env(), &["done"]);
    out.ok();
    assert!(
        out.stderr.contains(
            "jw: s1 is done; it is stopped once it is idle and detached, and the worktrees it created are removed"
        ),
        "{}",
        out.stderr
    );
}

fn hook_ws(e: &Env, hex: &str) -> std::path::PathBuf {
    let repo = e.root.join("repo");
    let input = json!({"cwd": repo, "name": format!("agent-{hex}")}).to_string();
    let out = e.jw_stdin(&repo, &input, &["hook", "create"]);
    out.ok();
    std::path::PathBuf::from(out.stdout.trim_end())
}

fn stdout_has(out: &Out, line: &str) {
    assert!(
        out.stdout.lines().any(|l| l == line),
        "stdout should have line {line:?}, got: {}\nstderr: {}",
        out.stdout,
        out.stderr
    );
}

#[test]
fn r1_reap_done_record_drops_worktrees() {
    let e = Env::new();
    e.repo();
    let own = e.workspace("s-own");
    let other = e.workspace("s-other");
    e.write_record(
        "s-other",
        &json!({"workspace": dir_str(&other), "status": "idle"}).to_string(),
    );

    let a1 = hook_ws(&e, "000000a1");
    let a2 = hook_ws(&e, "000000a2");
    fs::write(a2.join("work"), "holds work\n").unwrap();
    let a4 = e.workspace("a-4");
    let a5 = hook_ws(&e, "000000a5");
    let missing = e.dir.join("wt/missing");
    let mut sleeper = e
        .command("sleep", &a5)
        .arg("60")
        .spawn()
        .expect("spawn sleep");

    let wts = [
        dir_str(&a1),
        dir_str(&a2),
        dir_str(&a4),
        dir_str(&missing),
        "relative/dir",
        dir_str(&other),
        dir_str(&a5),
        dir_str(&own),
    ];
    e.write_record(
        "s1",
        &json!({"workspace": dir_str(&own), "status": "done", "worktrees": wts}).to_string(),
    );

    let out = e.jw(&e.root, &["reap"]);
    let _ = sleeper.kill();
    let _ = sleeper.wait();
    out.ok();

    let lines: Vec<&str> = out
        .stdout
        .lines()
        .filter(|l| l.starts_with("kept ") || l.starts_with("removed "))
        .collect();
    let want = vec![
        format!("removed {}", a1.display()),
        format!("kept {}: holds work", a2.display()),
        format!("kept {}: not a hook-created workspace", a4.display()),
        format!("kept {}: workspace of session s-other", other.display()),
        format!("kept {}: in use by other processes", a5.display()),
        format!("kept {}: workspace of session s1", own.display()),
        "removed repo.agents/s-own".to_string(),
    ];
    assert_eq!(lines, want, "stderr: {}", out.stderr);
    assert!(!a1.exists());
    assert!(a2.join("work").exists());
    assert!(a4.join(".jj").is_dir());
    assert!(other.join(".jj").is_dir());
    assert!(a5.exists());
    assert!(
        !out.stdout.contains("relative/dir") && !out.stdout.contains("wt/missing"),
        "{}",
        out.stdout
    );

    let listed = e.jj(
        &e.root.join("repo"),
        &["workspace", "list", "-T", "name ++ \"\\n\""],
    );
    assert!(!listed.lines().any(|l| l == "a-000000a1"), "{listed}");
    assert!(listed.lines().any(|l| l == "a-000000a2"), "{listed}");

    assert!(!own.exists(), "own workspace should be removed");
    assert!(e.record("s1")["reaped"].is_number());
}

#[test]
fn r2_reap_leaves_worktrees_of_not_done_record() {
    let e = Env::new();
    e.repo();
    let own = e.workspace("s-own");
    let a1 = hook_ws(&e, "000000a1");
    e.write_record(
        "s1",
        &json!({"workspace": dir_str(&own), "status": "idle", "worktrees": [dir_str(&a1)]})
            .to_string(),
    );
    let out = e.jw(&e.root, &["reap"]);
    out.ok();
    assert!(a1.exists());
    assert!(own.join(".jj").is_dir());
    assert!(e.record("s1").get("reaped").is_none());
    assert_eq!(e.record("s1")["worktrees"], json!([dir_str(&a1)]));
}

#[test]
fn r3_reap_merged_branch_leaves_worktrees() {
    let e = Env::new();
    e.repo();
    let m = e.workspace("s-m");
    let a9 = hook_ws(&e, "000000a9");
    e.write_record(
        "s-m",
        &json!({
            "workspace": dir_str(&m),
            "mrs": ["https://x/1"],
            "notified": 1,
            "worktrees": [dir_str(&a9)]
        })
        .to_string(),
    );
    let out = e.jw(&e.root, &["reap"]);
    out.ok();
    assert!(!m.exists(), "workspace should be removed: {}", out.stdout);
    assert!(a9.exists());
}

#[test]
fn r4_reap_keeps_hook_workspace_with_work() {
    let e = Env::new();
    e.repo();
    let own = e.workspace("s-own");
    let a1 = hook_ws(&e, "000000a1");
    fs::write(a1.join("work"), "w\n").unwrap();
    e.write_record(
        "s1",
        &json!({"workspace": dir_str(&own), "status": "done", "worktrees": [dir_str(&a1)]})
            .to_string(),
    );
    let out = e.jw(&e.root, &["reap"]);
    out.ok();
    stdout_has(&out, &format!("kept {}: holds work", a1.display()));
    assert_eq!(fs::read_to_string(a1.join("work")).unwrap(), "w\n");
}

#[test]
fn r5_reap_keeps_non_hook_workspace() {
    let e = Env::new();
    e.repo();
    let own = e.workspace("s-own");
    let a1 = e.workspace("a-1");
    e.write_record(
        "s1",
        &json!({"workspace": dir_str(&own), "status": "done", "worktrees": [dir_str(&a1)]})
            .to_string(),
    );
    let out = e.jw(&e.root, &["reap"]);
    out.ok();
    stdout_has(
        &out,
        &format!("kept {}: not a hook-created workspace", a1.display()),
    );
    assert!(a1.join(".jj").is_dir());
}

#[test]
fn r9_reap_keeps_hook_workspace_with_committed_work() {
    let e = Env::new();
    e.repo();
    let own = e.workspace("s-own");
    let a1 = hook_ws(&e, "000000a1");
    fs::write(a1.join("work"), "w\n").unwrap();
    e.jj(&a1, &["commit", "-m", "agent work"]);
    e.write_record(
        "s1",
        &json!({"workspace": dir_str(&own), "status": "done", "worktrees": [dir_str(&a1)]})
            .to_string(),
    );
    let out = e.jw(&e.root, &["reap"]);
    out.ok();
    stdout_has(&out, &format!("kept {}: holds work", a1.display()));
    assert_eq!(fs::read_to_string(a1.join("work")).unwrap(), "w\n");
}

#[test]
fn r10_reap_reports_deletion_failure() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    e.repo();
    let own = e.workspace("s-own");
    let a1 = hook_ws(&e, "000000a1");
    let ro = a1.join("ro");
    fs::create_dir(&ro).unwrap();
    fs::write(ro.join(".gitignore"), "*\n").unwrap();
    fs::write(ro.join("f"), "x\n").unwrap();
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
    e.write_record(
        "s1",
        &json!({"workspace": dir_str(&own), "status": "done", "worktrees": [dir_str(&a1)]})
            .to_string(),
    );
    let out = e.jw(&e.root, &["reap"]);
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).unwrap();
    out.ok();
    let prefix = format!("kept {}: ", a1.display());
    let line = out
        .stdout
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| {
            panic!(
                "no kept line\nstdout: {}\nstderr: {}",
                out.stdout, out.stderr
            )
        });
    assert!(!line.contains("holds work"), "{line}");
    assert!(line.len() > prefix.len(), "{line}");
    assert!(a1.exists());
}

fn which(tool: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(tool))
        .find(|p| p.is_file())
}

#[test]
fn r8_reap_missing_tools_on_path() {
    let e = Env::new();
    e.repo();
    let tools = e.dir.join("tools");
    fs::create_dir_all(&tools).unwrap();
    let git = which("git").expect("git on PATH");
    std::os::unix::fs::symlink(git, tools.join("git")).unwrap();
    let path = format!("{}:{}", e.dir.join("bin").display(), tools.display());

    let out = e.jw_env(&e.root, &[("PATH", &path)], &["reap"]);
    out.ok();
    let warn = out
        .stderr
        .lines()
        .find(|l| l.starts_with("jw: missing on PATH:"))
        .unwrap_or_else(|| panic!("no missing-on-PATH warning: {}", out.stderr));
    assert_eq!(warn, "jw: missing on PATH: jj");

    let jj = which("jj").expect("jj on PATH");
    std::os::unix::fs::symlink(jj, tools.join("jj")).unwrap();
    let out = e.jw_env(&e.root, &[("PATH", &path)], &["reap"]);
    out.ok();
    assert!(!out.stderr.contains("missing on PATH"), "{}", out.stderr);
}

#[test]
fn r6_reap_busy_done_record_keeps_everything() {
    let e = Env::new();
    e.repo();
    let own = e.workspace("s-own");
    let a1 = hook_ws(&e, "000000a1");
    e.write_record(
        "s1",
        &json!({"workspace": dir_str(&own), "status": "done", "worktrees": [dir_str(&a1)]})
            .to_string(),
    );
    let mut sleeper = e
        .command("sleep", &own)
        .arg("60")
        .spawn()
        .expect("spawn sleep");
    let out = e.jw(&e.root, &["reap"]);
    let _ = sleeper.kill();
    let _ = sleeper.wait();
    out.ok();
    assert!(a1.exists());
    assert!(own.join(".jj").is_dir());
    assert!(e.record("s1").get("reaped").is_none());
}

#[test]
fn r7_reap_already_reaped_record_not_dropped_again() {
    let e = Env::new();
    e.repo();
    let own = e.workspace("s-own");
    let a1 = hook_ws(&e, "000000a1");
    e.write_record(
        "s1",
        &json!({
            "workspace": dir_str(&own),
            "status": "done",
            "reaped": 1,
            "worktrees": [dir_str(&a1)]
        })
        .to_string(),
    );
    e.jw(&e.root, &["reap"]).ok();
    assert!(a1.exists());
}

#[test]
fn w7_worktree_prunes_vanished_paths_on_append() {
    let e = Env::new();
    let a = e.dir.join("wt/a");
    let b = e.dir.join("wt/b");
    let c = e.dir.join("wt/c");
    for d in [&a, &b, &c] {
        fs::create_dir_all(d).unwrap();
    }
    for d in [&a, &b] {
        e.jw_env(&e.home, &s1_env(), &["session", "worktree", dir_str(d)])
            .ok();
    }
    fs::remove_dir_all(&a).unwrap();
    e.jw_env(&e.home, &s1_env(), &["session", "worktree", dir_str(&c)])
        .ok();
    assert_eq!(
        e.record("s1")["worktrees"],
        json!([dir_str(&b), dir_str(&c)])
    );
}

const MR: &str = "https://github.com/o/r/pull/1";

fn poll_env(e: &Env, mark: Option<Value>) {
    e.repo();
    let ws = e.workspace("s-poll");
    let mut rec = json!({"workspace": dir_str(&ws), "status": "idle", "mrs": [MR]});
    if let Some(m) = mark {
        rec["mr_marks"] = json!({ MR: m });
    }
    e.write_record("p1", &rec.to_string());
    fixture(e, "view", json!({"forge": "github", "state": "open"}));
}

fn fixture(e: &Env, name: &str, v: Value) {
    fs::write(e.logs.join(format!("forge-{name}.json")), v.to_string()).unwrap();
}

fn pub_log(e: &Env) -> Vec<(String, Value)> {
    let log = fs::read_to_string(e.logs.join("event-pub.log")).unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    lines
        .chunks(2)
        .map(|c| {
            let ty = c[0].strip_prefix("type=").unwrap().to_string();
            let body = c[1].strip_prefix("stdin=").unwrap();
            (ty, serde_json::from_str(body).unwrap())
        })
        .collect()
}

fn gh_comment(id: i64) -> Value {
    json!({"id": id, "kind": "comment", "author": "alice", "association": "NONE",
           "created_at": "2026-11-22T10:00:00Z", "body": "please fix",
           "url": "https://github.com/o/r/pull/1#c", "path": null, "line": null,
           "review_state": null})
}

#[test]
fn p1_poll_publishes_failed_pipeline_then_comment() {
    let e = Env::new();
    poll_env(&e, Some(json!({"seen": []})));
    fixture(
        &e,
        "checks",
        json!({"sha": "S0123456789", "state": "failed", "url": "https://ci/1",
               "failed": [{"name": "build", "url": null}, {"name": "test", "url": null}]}),
    );
    fixture(&e, "comments", json!([gh_comment(77)]));

    let out = e.jw(&e.root, &["poll"]);
    out.ok();
    stdout_has(&out, "notified p1");

    let calls = pub_log(&e);
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0].0, "forge.pipeline_failed");
    assert_eq!(
        calls[0].1,
        json!({"type": "forge.pipeline_failed", "session": "p1", "forge": "github", "mr": MR,
               "sha": "S0123456789", "url": "https://ci/1", "jobs": ["build", "test"]})
    );
    assert_eq!(calls[1].0, "forge.comment");
    assert_eq!(
        calls[1].1,
        json!({"type": "forge.comment", "session": "p1", "forge": "github", "mr": MR, "id": 77,
               "kind": "comment", "author": "alice", "association": "NONE", "body": "please fix",
               "url": "https://github.com/o/r/pull/1#c", "path": null, "line": null,
               "review_state": null})
    );
    assert_eq!(
        e.record("p1")["mr_marks"][MR],
        json!({"failed_sha": "S0123456789", "seen": ["comment:77"]})
    );
}

#[test]
fn p2_poll_publish_failure_leaves_record_unchanged() {
    let e = Env::new();
    poll_env(&e, Some(json!({"seen": []})));
    fixture(
        &e,
        "checks",
        json!({"sha": "S0123456789", "state": "failed", "url": null, "failed": []}),
    );
    fixture(&e, "comments", json!([gh_comment(77)]));
    let msg = "event-pub: cannot publish events.forge.pipeline_failed: x";
    fs::write(e.logs.join("event-pub.fail"), format!("{msg}\n")).unwrap();
    let before = e.record("p1");

    let out = e.jw(&e.root, &["poll"]);
    out.ok();
    assert!(
        out.stderr.lines().any(|l| l == format!("jw: p1: {msg}")),
        "stderr: {}",
        out.stderr
    );
    assert!(!out.stdout.contains("notified"), "{}", out.stdout);
    assert_eq!(e.record("p1"), before);
    assert_eq!(pub_log(&e).len(), 1);
}

#[test]
fn p3_poll_without_activity_baselines_and_publishes_nothing() {
    let e = Env::new();
    poll_env(&e, None);
    fixture(
        &e,
        "checks",
        json!({"sha": "abc", "state": "success", "url": null, "failed": []}),
    );
    fixture(&e, "comments", json!([gh_comment(5)]));

    let out = e.jw(&e.root, &["poll"]);
    out.ok();
    assert!(!out.stdout.contains("notified"), "{}", out.stdout);
    assert!(!e.logs.join("event-pub.log").exists());
    assert_eq!(
        e.record("p1")["mr_marks"][MR],
        json!({"seen": ["comment:5"]})
    );
}

#[test]
fn p4_forge_notice_cli() {
    let e = Env::new();
    let ev = |assoc: &str| {
        json!({"type": "forge.comment", "session": "s1", "forge": "github", "mr": MR, "id": 1,
               "kind": "comment", "author": "alice", "association": assoc, "body": "hi\nthere",
               "url": null, "path": null, "line": null, "review_state": null})
        .to_string()
    };

    let out = e.jw_stdin(&e.root, &ev("OWNER"), &["forge-notice"]);
    out.ok();
    assert_eq!(out.stdout.lines().count(), 1, "{}", out.stdout);
    assert!(out.stdout.ends_with('\n'));
    let v = out.json();
    assert_eq!(v["to"], "s1");
    let text = v["text"].as_str().unwrap();
    assert!(text.contains(MR) && text.contains("- comment by @alice\n  > hi\n  > there"));

    let out = e.jw_stdin(&e.root, &ev("NONE"), &["forge-notice"]);
    out.ok();
    assert_eq!(out.stdout, "");

    let out = e.jw_stdin(&e.root, "not json", &["forge-notice"]);
    out.fail("jw: invalid forge event");
    assert_eq!(out.stdout, "");
}
