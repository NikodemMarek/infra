mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Env, Out};
use serde_json::{json, Value};

const AGENT: &str = "agent-0123abcd";
const AGENT_WS: &str = "a-0123abcd";

fn create_env(e: &Env, cwd: &Path, envs: &[(&str, &str)], name: &str) -> Out {
    let input = json!({
        "session_id": "sess",
        "hook_event_name": "WorktreeCreate",
        "cwd": cwd,
        "name": name,
    });
    e.jw_stdin_env(cwd, envs, &input.to_string(), &["hook", "create"])
}

fn create(e: &Env, cwd: &Path, name: &str) -> Out {
    create_env(e, cwd, &[], name)
}

fn printed(out: &Out) -> PathBuf {
    out.ok();
    assert!(out.stdout.ends_with('\n'), "stdout: {:?}", out.stdout);
    let line = out.stdout.trim_end_matches('\n');
    assert!(!line.contains('\n'), "one line expected: {:?}", out.stdout);
    PathBuf::from(line)
}

fn sha16(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn registry_key(ws: &Path) -> String {
    let repo = ws.join(".jj/repo");
    let store = if repo.is_file() {
        let target = PathBuf::from(fs::read_to_string(&repo).unwrap().trim());
        ws.join(".jj").join(target)
    } else {
        repo
    };
    sha16(fs::canonicalize(store).unwrap().to_str().unwrap())
}

fn registry_dir(e: &Env, repo: &Path) -> PathBuf {
    e.dir.join("claude/jj-agents").join(registry_key(repo))
}

fn entry(e: &Env, repo: &Path, name: &str) -> Value {
    let p = registry_dir(e, repo).join(format!("{name}.json"));
    let text = fs::read_to_string(&p).unwrap_or_else(|err| panic!("read {}: {err}", p.display()));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("bad entry json ({err}): {text}"))
}

fn write_entry(e: &Env, repo: &Path, name: &str, v: &Value) {
    let p = registry_dir(e, repo).join(format!("{name}.json"));
    fs::write(p, serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn change_id(e: &Env, cwd: &Path, rev: &str) -> String {
    e.jj(cwd, &["log", "-r", rev, "--no-graph", "-T", "change_id"])
        .trim()
        .to_string()
}

fn workspaces(e: &Env, repo: &Path) -> Vec<String> {
    e.jj(repo, &["workspace", "list", "-T", "name ++ \"\\n\""])
        .lines()
        .map(str::to_string)
        .collect()
}

fn is_empty(e: &Env, cwd: &Path, rev: &str) -> bool {
    e.jj(cwd, &["log", "-r", rev, "--no-graph", "-T", "empty"])
        .trim()
        == "true"
}

fn git(e: &Env, cwd: &Path, args: &[&str]) -> String {
    let o = e
        .command("git", cwd)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn git_repo(e: &Env) -> PathBuf {
    let d = e.root.join("plain");
    fs::create_dir_all(&d).unwrap();
    git(e, &d, &["init", "-q", "-b", "trunk"]);
    fs::write(d.join("file"), "one\n").unwrap();
    git(e, &d, &["add", "file"]);
    git(e, &d, &["commit", "-q", "-m", "one"]);
    d
}

#[test]
fn c1_create_agent_in_fresh_repo() {
    let e = Env::new();
    let repo = e.repo();
    let caller = change_id(&e, &repo, "@");
    assert!(!e.root.join("repo.agents").exists());

    let out = create(&e, &repo, AGENT);
    let d = printed(&out);
    assert_eq!(d, e.root.join("repo.agents").join(AGENT_WS));
    assert!(d.join(".jj").is_dir());
    assert!(workspaces(&e, &repo).contains(&AGENT_WS.to_string()));

    assert_eq!(change_id(&e, &d, "@-"), caller);
    assert!(is_empty(&e, &d, "@"));

    let log = e.log("claude-mcp-defaults.log");
    assert!(
        log.lines().any(|l| l == d.to_str().unwrap()),
        "mcp-defaults log: {log}"
    );

    let key = registry_key(&repo);
    assert_eq!(key.len(), 16);
    assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(e
        .dir
        .join("claude/jj-agents")
        .join(&key)
        .join(format!("{AGENT}.json"))
        .is_file());

    let v = entry(&e, &repo, AGENT);
    assert_eq!(v["name"], AGENT);
    assert_eq!(v["root"], repo.to_str().unwrap());
    assert_eq!(v["dir"], d.to_str().unwrap());
    assert_eq!(v["agents"], e.root.join("repo.agents").to_str().unwrap());
    assert_eq!(v["base"], caller);
    assert_eq!(v["kind"], "agent");
    assert_eq!(v["ws"], AGENT_WS);
    assert!(v["touched"].as_f64().unwrap() > 1e9, "touched: {v}");
    assert!(v.get("warned").is_none(), "warned must be omitted: {v}");
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        ["agents", "base", "dir", "kind", "name", "root", "touched", "ws"]
    );
}

#[test]
fn c2_create_session_kind() {
    let e = Env::new();
    let repo = e.repo();
    let trunk = change_id(&e, &repo, "main");
    assert_ne!(trunk, change_id(&e, &repo, "@"));

    let d = printed(&create(&e, &repo, "s-0101-120000-7"));
    assert_eq!(d, e.root.join("repo.agents/s-0101-120000-7"));
    assert!(workspaces(&e, &repo).contains(&"s-0101-120000-7".to_string()));
    assert_eq!(change_id(&e, &d, "@-"), trunk);

    let v = entry(&e, &repo, "s-0101-120000-7");
    assert_eq!(v["kind"], "session");
    assert_eq!(v["base"], trunk);
    assert_eq!(v["name"], "s-0101-120000-7");
    assert!(v.get("ws").is_none(), "ws must be omitted: {v}");
    assert!(v.get("warned").is_none(), "warned must be omitted: {v}");
    assert!(e
        .log("claude-mcp-defaults.log")
        .lines()
        .any(|l| l == d.to_str().unwrap()));
}

#[test]
fn c3_workspace_base_for_session() {
    let e = Env::new();
    let repo = e.repo();
    e.jj(&repo, &["new", "main", "-m", "feature"]);
    let feature = change_id(&e, &repo, "@");
    e.jj(&repo, &["new", "main"]);
    let trunk = change_id(&e, &repo, "main");

    let d = printed(&create_env(
        &e,
        &repo,
        &[("CLAUDE_WORKSPACE_BASE", &feature)],
        "s-one",
    ));
    assert_eq!(change_id(&e, &d, "@-"), feature);
    assert_eq!(entry(&e, &repo, "s-one")["base"], feature);

    let d = printed(&create_env(
        &e,
        &repo,
        &[("CLAUDE_WORKSPACE_BASE", "qqqqqqqqqqqqqqqq")],
        "s-two",
    ));
    assert_eq!(change_id(&e, &d, "@-"), trunk);
    assert_eq!(entry(&e, &repo, "s-two")["base"], trunk);

    let d = printed(&create_env(
        &e,
        &repo,
        &[("CLAUDE_WORKSPACE_BASE", "main")],
        "s-three",
    ));
    assert_eq!(change_id(&e, &d, "@-"), trunk);
}

#[test]
fn c4_workspace_base_ignored_for_agent() {
    let e = Env::new();
    let repo = e.repo();
    e.jj(&repo, &["describe", "-m", "caller"]);
    let caller = change_id(&e, &repo, "@");
    e.jj(&repo, &["new", "main", "-m", "feature"]);
    let feature = change_id(&e, &repo, "@");
    e.jj(&repo, &["edit", &caller]);

    let d = printed(&create_env(
        &e,
        &repo,
        &[("CLAUDE_WORKSPACE_BASE", &feature)],
        AGENT,
    ));
    assert_eq!(change_id(&e, &d, "@-"), caller);
}

#[test]
fn c5_secondary_workspace_shares_registry() {
    let e = Env::new();
    let repo = e.repo();
    let first = printed(&create(&e, &repo, AGENT));
    let keys = || {
        fs::read_dir(e.dir.join("claude/jj-agents"))
            .unwrap()
            .map(|d| d.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(), [registry_key(&repo)]);

    let second = printed(&create(&e, &first, "s-from-secondary"));
    assert!(second.join(".jj").is_dir());
    assert_ne!(first, second);
    assert_eq!(keys(), [registry_key(&repo)]);
    assert_eq!(registry_key(&first), registry_key(&repo));

    let v = entry(&e, &repo, "s-from-secondary");
    assert_eq!(v["root"], first.to_str().unwrap());
    assert_eq!(v["dir"], second.to_str().unwrap());
    assert!(workspaces(&e, &repo).contains(&"s-from-secondary".to_string()));
}

#[test]
fn c7_repeat_returns_same_dir() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    let mut v = entry(&e, &repo, AGENT);
    v["touched"] = json!(1000.0);
    write_entry(&e, &repo, AGENT, &v);
    fs::write(d.join("work"), "edit\n").unwrap();

    let again = printed(&create(&e, &repo, AGENT));
    assert_eq!(again, d);
    assert!(d.join("work").exists());
    assert!(entry(&e, &repo, AGENT)["touched"].as_f64().unwrap() > 1e9);
    let n = workspaces(&e, &repo)
        .iter()
        .filter(|w| *w == AGENT_WS)
        .count();
    assert_eq!(n, 1);
}

#[test]
fn c8_registered_dir_deleted_is_recreated() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::remove_dir_all(&d).unwrap();

    let again = printed(&create(&e, &repo, AGENT));
    assert_eq!(again, d);
    assert!(d.join(".jj").is_dir());
    let n = workspaces(&e, &repo)
        .iter()
        .filter(|w| *w == AGENT_WS)
        .count();
    assert_eq!(n, 1);
    assert_eq!(entry(&e, &repo, AGENT)["dir"], d.to_str().unwrap());
}

#[test]
fn c9_collision_with_unmanaged_workspace() {
    let e = Env::new();
    let repo = e.repo();
    let other = e.workspace(AGENT_WS);

    create(&e, &repo, AGENT).fail(&format!(
        "a jj workspace named '{AGENT_WS}' already exists and isn't managed by this hook"
    ));
    assert!(other.join(".jj").is_dir());
}

#[test]
fn c10_existing_dir_is_an_error() {
    let e = Env::new();
    let repo = e.repo();
    let d = e.root.join("repo.agents/s-taken");
    fs::create_dir_all(&d).unwrap();

    let out = create(&e, &repo, "s-taken");
    out.fail(&format!("{} already exists", d.display()));
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    assert!(!workspaces(&e, &repo).contains(&"s-taken".to_string()));
}

#[test]
fn c11_existing_dangling_symlink_is_an_error() {
    let e = Env::new();
    let repo = e.repo();
    let agents = e.root.join("repo.agents");
    fs::create_dir_all(&agents).unwrap();
    let d = agents.join("s-link");
    std::os::unix::fs::symlink(e.dir.join("nowhere"), &d).unwrap();

    create(&e, &repo, "s-link").fail(&format!("{} already exists", d.display()));
}

#[test]
fn c12_invalid_names() {
    let e = Env::new();
    let repo = e.repo();
    let long = "a".repeat(101);
    for name in ["", "bad/name", "has space", "-lead", &long] {
        create(&e, &repo, name).fail(&format!("jw: invalid worktree name '{name}'"));
    }

    let input = json!({"cwd": repo}).to_string();
    e.jw_stdin(&repo, &input, &["hook", "create"])
        .fail("jw: invalid worktree name ''");
    let input = json!({"cwd": repo, "name": 5}).to_string();
    e.jw_stdin(&repo, &input, &["hook", "create"])
        .fail("jw: invalid worktree name ''");

    assert!(!e.root.join("repo.agents").exists());
}

#[test]
fn c13_name_of_100_chars_is_valid() {
    let e = Env::new();
    let repo = e.repo();
    let name = "b".repeat(100);
    let d = printed(&create(&e, &repo, &name));
    assert_eq!(d, e.root.join("repo.agents").join(&name));
}

#[test]
fn c14_unparsable_stdin() {
    let e = Env::new();
    let repo = e.repo();
    let out = e.jw_stdin(&repo, "not json", &["hook", "create"]);
    out.fail("jw: ");
    assert!(out.stdout.is_empty());
}

#[test]
fn c15_cwd_defaults_to_process_cwd() {
    let e = Env::new();
    let repo = e.repo();
    let input = json!({"name": "s-nocwd"}).to_string();
    let out = e.jw_stdin(&repo, &input, &["hook", "create"]);
    assert_eq!(printed(&out), e.root.join("repo.agents/s-nocwd"));
}

#[test]
fn c16_agent_session_tracks_new_worktree() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create_env(&e, &repo, &[("AGENT_SESSION", "s1")], AGENT));
    assert_eq!(e.record("s1")["worktrees"], json!([d.to_str().unwrap()]));
}

#[test]
fn c17_no_tracking_without_agent_session() {
    let e = Env::new();
    let repo = e.repo();
    printed(&create(&e, &repo, AGENT));
    assert!(!e.state.join("agent-sessions").exists());
    printed(&create_env(&e, &repo, &[("AGENT_SESSION", "")], "s-x"));
    assert!(!e.state.join("agent-sessions").exists());
}

#[test]
fn c18_tracking_error_still_succeeds() {
    let e = Env::new();
    let repo = e.repo();
    let out = create_env(&e, &repo, &[("AGENT_SESSION", "bad/name")], AGENT);
    let d = printed(&out);
    assert_eq!(out.stdout, format!("{}\n", d.display()));
    assert!(
        out.stderr
            .contains(&format!("jw: session worktree {}: ", d.display())),
        "stderr: {}",
        out.stderr
    );
    assert!(d.join(".jj").is_dir());
    assert_eq!(entry(&e, &repo, AGENT)["dir"], d.to_str().unwrap());
}

#[test]
fn g1_git_fallback_new_branch() {
    let e = Env::new();
    let top = git_repo(&e);
    let sub = top.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let head = git(&e, &top, &["rev-parse", "HEAD"]);

    let out = create(&e, &sub, "feat");
    let wt = printed(&out);
    assert_eq!(wt, top.join(".claude/worktrees/feat"));
    assert!(wt.join("file").is_file());
    assert_eq!(
        git(&e, &wt, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "worktree-feat"
    );
    assert_eq!(git(&e, &wt, &["rev-parse", "HEAD"]), head);
    assert!(e
        .log("claude-mcp-defaults.log")
        .lines()
        .any(|l| l == wt.to_str().unwrap()));
    assert!(!e.dir.join("claude/jj-agents").exists());
}

#[test]
fn g2_git_fallback_existing_branch_reused() {
    let e = Env::new();
    let top = git_repo(&e);
    let old = git(&e, &top, &["rev-parse", "HEAD"]);
    fs::write(top.join("file"), "two\n").unwrap();
    git(&e, &top, &["commit", "-q", "-am", "two"]);
    git(&e, &top, &["branch", "worktree-feat", &old]);

    let wt = printed(&create(&e, &top, "feat"));
    assert_eq!(wt, top.join(".claude/worktrees/feat"));
    assert_eq!(
        git(&e, &wt, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "worktree-feat"
    );
    assert_eq!(git(&e, &wt, &["rev-parse", "HEAD"]), old);
}

#[test]
fn g3_git_fallback_existing_worktree_is_printed() {
    let e = Env::new();
    let top = git_repo(&e);
    let wt = printed(&create(&e, &top, "feat"));
    let again = printed(&create(&e, &top, "feat"));
    assert_eq!(again, wt);
}

#[test]
fn g4_git_fallback_outside_any_repo_fails() {
    let e = Env::new();
    let plain = e.dir.join("nothing");
    fs::create_dir_all(&plain).unwrap();
    let out = create(&e, &plain, "feat");
    assert_eq!(out.code, 1, "stderr: {}", out.stderr);
    assert!(out.stdout.is_empty());
}

#[test]
fn g5_git_fallback_invalid_name() {
    let e = Env::new();
    let top = git_repo(&e);
    create(&e, &top, "bad/name").fail("jw: invalid worktree name 'bad/name'");
    assert!(!top.join(".claude").exists());
}

const HEX: &str = "0123abcd";
const CONTEXT_TAIL: &str =
    " Reply with this sentence verbatim, followed by your complete previous final report unchanged.";

fn stop_input(agent_id: &str, cwd: &Path, active: Option<bool>) -> String {
    let mut v = json!({
        "session_id": "sess",
        "hook_event_name": "SubagentStop",
        "agent_id": agent_id,
        "cwd": cwd,
    });
    if let Some(a) = active {
        v["stop_hook_active"] = json!(a);
    }
    v.to_string()
}

fn stop_with(e: &Env, repo: &Path, agent_id: &str, cwd: &Path, active: Option<bool>) -> Out {
    e.jw_stdin(repo, &stop_input(agent_id, cwd, active), &["hook", "stop"])
}

fn stop(e: &Env, repo: &Path, d: &Path) -> Out {
    stop_with(e, repo, HEX, d, None)
}

fn b8(base: &str) -> String {
    base[..8].to_string()
}

fn no_output(out: &Out) {
    out.ok();
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
}

fn file_in(e: &Env, repo: &Path, rev: &str, name: &str) -> Option<String> {
    let o = e
        .command("jj", repo)
        .args(["--no-pager", "file", "show", "-r", rev, name])
        .output()
        .unwrap();
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn problem_output(out: &Out, active: bool) -> String {
    out.ok();
    let v = out.json();
    let msg = v["systemMessage"].as_str().unwrap().to_string();
    if active {
        assert!(v.get("hookSpecificOutput").is_none(), "output: {v}");
        assert_eq!(v.as_object().unwrap().len(), 1, "output: {v}");
    } else {
        let h = &v["hookSpecificOutput"];
        assert_eq!(h["hookEventName"], "SubagentStop");
        assert_eq!(h["additionalContext"], format!("{msg}{CONTEXT_TAIL}"));
        assert_eq!(v.as_object().unwrap().len(), 2, "output: {v}");
        assert_eq!(h.as_object().unwrap().len(), 2, "output: {v}");
    }
    msg
}

fn assert_conflict(out: &Out, active: bool) -> String {
    let msg = problem_output(out, active);
    assert!(
        msg.contains("which left CONFLICTS in"),
        "systemMessage: {msg}"
    );
    msg
}

fn assert_problem(out: &Out, dir: &Path, expect: &str, active: bool) -> String {
    let msg = problem_output(out, active);
    let tail = format!(". The agent's work is kept in {}.", dir.display());
    assert!(
        msg.starts_with("jj integration problem: "),
        "systemMessage: {msg}"
    );
    assert!(msg.ends_with(&tail), "systemMessage: {msg}");
    assert!(msg.contains(expect), "systemMessage: {msg}");
    msg
}

#[test]
fn s1_stop_squashes_agent_work_into_base() {
    let e = Env::new();
    let repo = e.repo();
    let base = change_id(&e, &repo, "@");
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "agent work\n").unwrap();
    fs::write(d.join("file"), "edited\n").unwrap();
    let mut v = entry(&e, &repo, AGENT);
    v["touched"] = json!(1000.0);
    write_entry(&e, &repo, AGENT, &v);

    let out = stop(&e, &repo, &d);
    out.ok();
    let j = out.json();
    assert_eq!(
        j["systemMessage"],
        format!(
            "jj: squashed 2 file(s) from agent workspace {AGENT_WS} into {} (no description)",
            b8(&base)
        )
    );
    assert!(j.get("hookSpecificOutput").is_none(), "output: {j}");
    assert_eq!(j.as_object().unwrap().len(), 1, "output: {j}");

    assert_eq!(
        file_in(&e, &repo, &base, "work").as_deref(),
        Some("agent work\n")
    );
    assert_eq!(
        file_in(&e, &repo, &base, "file").as_deref(),
        Some("edited\n")
    );
    assert!(is_empty(&e, &d, "@"));
    assert_eq!(change_id(&e, &d, "@-"), base);
    assert!(entry(&e, &repo, AGENT)["touched"].as_f64().unwrap() > 1e9);
}

#[test]
fn s2_stop_message_shows_first_description_line() {
    let e = Env::new();
    let repo = e.repo();
    e.jj(&repo, &["describe", "-m", "feature x\n\nmore text"]);
    let base = change_id(&e, &repo, "@");
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "w\n").unwrap();

    let out = stop(&e, &repo, &d);
    out.ok();
    assert_eq!(
        out.json()["systemMessage"],
        format!(
            "jj: squashed 1 file(s) from agent workspace {AGENT_WS} into {} feature x",
            b8(&base)
        )
    );
    assert_eq!(file_in(&e, &repo, &base, "work").as_deref(), Some("w\n"));
}

#[test]
fn s3_stop_accepts_cwd_inside_workspace() {
    let e = Env::new();
    let repo = e.repo();
    let base = change_id(&e, &repo, "@");
    let d = printed(&create(&e, &repo, AGENT));
    fs::create_dir_all(d.join("sub/deeper")).unwrap();
    fs::write(d.join("work"), "w\n").unwrap();

    let out = stop_with(&e, &repo, HEX, &d.join("sub/deeper"), Some(false));
    out.ok();
    assert!(out.json()["systemMessage"]
        .as_str()
        .unwrap()
        .starts_with("jj: squashed 1 file(s)"));
    assert!(file_in(&e, &repo, &base, "work").is_some());
}

#[test]
fn s4_stop_without_work_is_silent() {
    let e = Env::new();
    let repo = e.repo();
    let base = change_id(&e, &repo, "@");
    let d = printed(&create(&e, &repo, AGENT));
    let before = e.jj(
        &repo,
        &[
            "log",
            "-r",
            "all()",
            "--no-graph",
            "-T",
            "commit_id ++ \"\\n\"",
        ],
    );

    no_output(&stop(&e, &repo, &d));

    let after = e.jj(
        &repo,
        &[
            "log",
            "-r",
            "all()",
            "--no-graph",
            "-T",
            "commit_id ++ \"\\n\"",
        ],
    );
    assert_eq!(before, after);
    assert!(is_empty(&e, &d, "@"));
    assert_eq!(change_id(&e, &d, "@-"), base);
    assert!(d.join(".jj").is_dir());
}

#[test]
fn s5_stop_unknown_agent_id_is_silent() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "w\n").unwrap();
    let base = change_id(&e, &repo, "@");

    no_output(&stop_with(&e, &repo, "deadbeef", &d, None));
    assert!(file_in(&e, &repo, &base, "work").is_none());
    assert!(d.join("work").exists());
}

#[test]
fn s6_stop_without_registry_is_silent() {
    let e = Env::new();
    let repo = e.repo();
    no_output(&stop_with(&e, &repo, HEX, &repo, None));
    assert!(!e.dir.join("claude/jj-agents").exists());
}

#[test]
fn s7_stop_agent_id_must_be_lowercase_hex() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "w\n").unwrap();
    let base = change_id(&e, &repo, "@");
    for id in [
        "",
        "0123ABCD",
        "../agent-0123abcd",
        "0123abcd/..",
        "0123abc*",
        "xyz",
    ] {
        no_output(&stop_with(&e, &repo, id, &d, None));
    }
    assert!(file_in(&e, &repo, &base, "work").is_none());
}

#[test]
fn s8_stop_cwd_outside_workspace_is_silent() {
    let e = Env::new();
    let repo = e.repo();
    let base = change_id(&e, &repo, "@");
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "w\n").unwrap();
    let sibling = e.root.join("repo.agents/a-0123abcd-other");
    fs::create_dir_all(&sibling).unwrap();

    for cwd in [
        repo.clone(),
        sibling,
        e.root.clone(),
        PathBuf::from("/nonexistent"),
    ] {
        no_output(&stop_with(&e, &repo, HEX, &cwd, None));
    }
    assert!(file_in(&e, &repo, &base, "work").is_none());
    assert!(d.join("work").exists());
}

#[test]
fn s9_stop_missing_cwd_is_silent() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "w\n").unwrap();
    let base = change_id(&e, &repo, "@");

    let input = json!({"agent_id": HEX}).to_string();
    no_output(&e.jw_stdin(&d, &input, &["hook", "stop"]));
    assert!(file_in(&e, &repo, &base, "work").is_none());
}

fn conflict_setup(e: &Env) -> (PathBuf, PathBuf, String) {
    let repo = e.repo();
    let base = change_id(e, &repo, "@");
    let d = printed(&create(e, &repo, AGENT));
    e.jj(&repo, &["new", "-m", "child"]);
    fs::write(repo.join("file"), "child\n").unwrap();
    e.jj(&repo, &["status"]);
    fs::write(d.join("file"), "agent\n").unwrap();
    (repo, d, base)
}

#[test]
fn s10_stop_conflict_reports_with_additional_context() {
    let e = Env::new();
    let (repo, d, base) = conflict_setup(&e);

    let out = stop(&e, &repo, &d);
    let msg = assert_conflict(&out, false);
    assert!(
        msg.starts_with(&format!(
            "jj: squashed 1 file(s) from agent workspace {AGENT_WS} into {} (no description), which left CONFLICTS in ",
            b8(&base)
        )),
        "systemMessage: {msg}"
    );
    assert!(
        msg.contains("(jj resolve --list -r "),
        "systemMessage: {msg}"
    );
    assert!(msg.ends_with(")."), "systemMessage: {msg}");

    assert_eq!(
        file_in(&e, &repo, &base, "file").as_deref(),
        Some("agent\n")
    );
    let conflicted = e.jj(
        &repo,
        &[
            "log",
            "-r",
            "conflicts() ~ empty()",
            "--no-graph",
            "-T",
            "change_id.short(8) ++ \"\\n\"",
        ],
    );
    assert!(!conflicted.trim().is_empty());
    for id in conflicted.lines() {
        assert!(msg.contains(id), "{id} missing from {msg}");
    }
}

#[test]
fn s11_stop_conflict_with_stop_hook_active_has_no_additional_context() {
    let e = Env::new();
    let (repo, d, base) = conflict_setup(&e);

    let out = stop_with(&e, &repo, HEX, &d, Some(true));
    assert_conflict(&out, true);
    assert_eq!(
        file_in(&e, &repo, &base, "file").as_deref(),
        Some("agent\n")
    );
}

#[test]
fn s12_stop_hook_active_false_still_adds_context() {
    let e = Env::new();
    let (repo, d, _) = conflict_setup(&e);
    let out = stop_with(&e, &repo, HEX, &d, Some(false));
    assert_conflict(&out, false);
}

#[test]
fn s13_stop_hook_active_does_not_silence_success_message() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "w\n").unwrap();

    let out = stop_with(&e, &repo, HEX, &d, Some(true));
    out.ok();
    let j = out.json();
    assert!(j["systemMessage"]
        .as_str()
        .unwrap()
        .starts_with("jj: squashed 1 file(s)"));
    assert!(j.get("hookSpecificOutput").is_none());
}

fn immutable_setup(e: &Env) -> (PathBuf, PathBuf, String) {
    let repo = e.repo();
    let base = change_id(e, &repo, "@");
    let d = printed(&create(e, &repo, AGENT));
    e.jj(&repo, &["new"]);
    e.jj(&repo, &["tag", "set", "frozen", "-r", &base]);
    assert_eq!(
        e.jj(
            &repo,
            &[
                "log",
                "-r",
                &format!("immutable() & {base}"),
                "--no-graph",
                "-T",
                "change_id"
            ]
        )
        .trim(),
        base
    );
    fs::write(d.join("work"), "w\n").unwrap();
    (repo, d, base)
}

#[test]
fn s14_stop_immutable_base_is_an_integration_problem() {
    let e = Env::new();
    let (repo, d, base) = immutable_setup(&e);

    let out = stop(&e, &repo, &d);
    let msg = assert_problem(
        &out,
        &d,
        &format!("base change {} is immutable now", b8(&base)),
        false,
    );
    assert!(
        msg.starts_with("jj integration problem: "),
        "systemMessage: {msg}"
    );
    assert!(
        msg.ends_with(&format!(". The agent's work is kept in {}.", d.display())),
        "systemMessage: {msg}"
    );
    assert!(d.join("work").exists());
    assert!(file_in(&e, &repo, &base, "work").is_none());
}

#[test]
fn s15_stop_immutable_base_with_stop_hook_active() {
    let e = Env::new();
    let (repo, d, base) = immutable_setup(&e);
    let out = stop_with(&e, &repo, HEX, &d, Some(true));
    assert_problem(
        &out,
        &d,
        &format!("base change {} is immutable now", b8(&base)),
        true,
    );
}

#[test]
fn s16_stop_workspace_dir_gone() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::remove_dir_all(&d).unwrap();

    let out = stop(&e, &repo, &d);
    assert_problem(
        &out,
        &d,
        &format!("workspace {} no longer exists", d.display()),
        false,
    );
    assert_eq!(entry(&e, &repo, AGENT)["dir"], d.to_str().unwrap());
}

#[test]
fn s17_stop_dir_gone_with_stop_hook_active() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::remove_dir_all(&d).unwrap();
    let out = stop_with(&e, &repo, HEX, &d, Some(true));
    assert_problem(
        &out,
        &d,
        &format!("workspace {} no longer exists", d.display()),
        true,
    );
}

#[test]
fn s18_stop_agent_base_gone_is_an_integration_problem() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "w\n").unwrap();
    let mut v = entry(&e, &repo, AGENT);
    v["base"] = json!("k".repeat(32));
    write_entry(&e, &repo, AGENT, &v);

    let out = stop(&e, &repo, &d);
    assert_problem(&out, &d, "base change kkkkkkkk is gone", false);
    assert!(d.join("work").exists());
}

#[test]
fn s19_stop_agent_not_on_top_of_base_is_a_problem() {
    let e = Env::new();
    let repo = e.repo();
    let base = change_id(&e, &repo, "@");
    let d = printed(&create(&e, &repo, AGENT));
    e.jj(&d, &["rebase", "-r", "@", "-d", "main"]);
    fs::write(d.join("work"), "w\n").unwrap();

    let out = stop(&e, &repo, &d);
    assert_problem(
        &out,
        &d,
        &format!("the agent's @ is no longer on top of base {}", b8(&base)),
        false,
    );
    assert!(file_in(&e, &repo, &base, "work").is_none());
}

#[test]
fn s20_stop_folds_stale_twin_edits() {
    let e = Env::new();
    let repo = e.repo();
    let base = change_id(&e, &repo, "@");
    let d = printed(&create(&e, &repo, AGENT));
    let agent_change = change_id(&e, &d, "@");
    fs::write(d.join("late"), "unsaved\n").unwrap();
    e.jj(
        &repo,
        &["describe", "-r", &agent_change, "-m", "renamed elsewhere"],
    );

    let out = stop(&e, &repo, &d);
    out.ok();
    assert!(
        out.stderr.contains("jw: folded unsaved edits ("),
        "stderr: {}",
        out.stderr
    );
    assert!(
        out.stderr
            .contains(&format!("back into @ of {}", d.display())),
        "stderr: {}",
        out.stderr
    );
    assert_eq!(
        out.json()["systemMessage"],
        format!(
            "jj: squashed 1 file(s) from agent workspace {AGENT_WS} into {} (no description)",
            b8(&base)
        )
    );
    assert_eq!(
        file_in(&e, &repo, &base, "late").as_deref(),
        Some("unsaved\n")
    );
    let twins = e.jj(
        &d,
        &[
            "log",
            "-r",
            &format!("change_id({agent_change})"),
            "--no-graph",
            "-T",
            "commit_id ++ \"\\n\"",
        ],
    );
    assert_eq!(twins.lines().count(), 0, "twins: {twins}");
}

#[test]
fn s21_stop_bad_stdin_is_silent_exit_zero() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "w\n").unwrap();
    let base = change_id(&e, &repo, "@");

    for input in ["not json", "", "{", "[]"] {
        let out = e.jw_stdin(&repo, input, &["hook", "stop"]);
        no_output(&out);
        assert!(!out.stderr.trim().is_empty(), "stderr empty for {input:?}");
    }
    assert!(file_in(&e, &repo, &base, "work").is_none());
}

#[test]
fn s22_stop_missing_agent_id_is_silent() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "w\n").unwrap();
    let base = change_id(&e, &repo, "@");

    let input = json!({"cwd": d}).to_string();
    no_output(&e.jw_stdin(&repo, &input, &["hook", "stop"]));
    assert!(file_in(&e, &repo, &base, "work").is_none());
}

#[test]
fn s23_stop_is_idempotent() {
    let e = Env::new();
    let repo = e.repo();
    let d = printed(&create(&e, &repo, AGENT));
    fs::write(d.join("work"), "w\n").unwrap();

    assert!(stop(&e, &repo, &d).ok().json()["systemMessage"].is_string());
    no_output(&stop(&e, &repo, &d));
}

#[test]
fn s24_stop_leaves_other_agents_alone() {
    let e = Env::new();
    let repo = e.repo();
    let base = change_id(&e, &repo, "@");
    let a = printed(&create(&e, &repo, AGENT));
    let other = printed(&create(&e, &repo, "agent-feed0001"));
    fs::write(a.join("work"), "a\n").unwrap();
    fs::write(other.join("theirs"), "o\n").unwrap();

    stop(&e, &repo, &a).ok();
    assert!(file_in(&e, &repo, &base, "work").is_some());
    assert!(file_in(&e, &repo, &base, "theirs").is_none());
    assert!(other.join("theirs").exists());
}

#[test]
fn s25_stop_refreshes_caller_root_before_agent_dir() {
    let e = Env::new();
    let repo = e.repo();
    let base = change_id(&e, &repo, "@");
    let d = printed(&create(&e, &repo, AGENT));
    let agent_change = change_id(&e, &d, "@");
    fs::write(repo.join("callers"), "unsaved in root\n").unwrap();
    fs::write(d.join("agents"), "agent work\n").unwrap();

    let out = stop(&e, &repo, &d);
    out.ok();
    assert_eq!(
        out.json()["systemMessage"],
        format!(
            "jj: squashed 1 file(s) from agent workspace {AGENT_WS} into {} (no description)",
            b8(&base)
        )
    );
    assert_eq!(
        file_in(&e, &repo, &base, "callers").as_deref(),
        Some("unsaved in root\n")
    );
    assert_eq!(
        file_in(&e, &repo, &base, "agents").as_deref(),
        Some("agent work\n")
    );
    assert!(is_empty(&e, &d, "@"));
    assert_eq!(change_id(&e, &d, "@-"), base);
    let twins = e.jj(
        &d,
        &[
            "log",
            "-r",
            &format!("change_id({agent_change})"),
            "--no-graph",
            "-T",
            "commit_id ++ \"\\n\"",
        ],
    );
    assert_eq!(twins.lines().count(), 0, "twins: {twins}");
    e.jj(&repo, &["status"]);
    assert_eq!(
        file_in(&e, &repo, "@", "callers").as_deref(),
        Some("unsaved in root\n")
    );
}
