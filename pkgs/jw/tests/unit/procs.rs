use super::*;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

fn fixture(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("jw-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (sessions, proc) = (root.join("sessions"), root.join("proc"));
    fs::create_dir_all(&sessions).unwrap();
    fs::create_dir_all(&proc).unwrap();
    (sessions, proc)
}

fn cleanup(sessions: &Path) {
    fs::remove_dir_all(sessions.parent().unwrap()).unwrap();
}

fn stat(pid: u32, comm: &str, start: u64) -> String {
    format!("{pid} ({comm}) S {} {start} 0 0\n", ["0"; 18].join(" "))
}

fn process(proc: &Path, pid: u32, start: u64, cwd: &str, environ: &[u8]) {
    let dir = proc.join(pid.to_string());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("stat"), stat(pid, "claude", start)).unwrap();
    symlink(cwd, dir.join("cwd")).unwrap();
    fs::write(dir.join("environ"), environ).unwrap();
}

fn register(sessions: &Path, file: &str, pid_json: &str, proc_start: &str) {
    fs::write(
        sessions.join(file),
        format!(
            r#"{{"pid":{pid_json},"procStart":"{proc_start}","sessionId":"sid-{file}","name":"n-{file}","status":"idle","waitingFor":"x","statusUpdatedAt":7000.0,"messagingSocketPath":"/s/{file}"}}"#
        ),
    )
    .unwrap();
}

#[test]
fn load_claudes_matches_process() {
    let (sessions, proc) = fixture("match");
    register(&sessions, "10.json", "10", "100");
    process(
        &proc,
        10,
        100,
        "/home/u/w1",
        b"A=b\0AGENT_SESSION=jw-1\0C=d\0",
    );
    let found = load_claudes(&sessions, &proc);
    assert_eq!(found.len(), 1);
    let c = &found[0];
    assert_eq!(c.pid, 10);
    assert_eq!(c.cwd, "/home/u/w1");
    assert_eq!(c.agent.as_deref(), Some("jw-1"));
    assert_eq!(c.status.as_deref(), Some("idle"));
    assert_eq!(c.waiting_for.as_deref(), Some("x"));
    assert_eq!(c.status_updated_at, Some(7000.0));
    assert_eq!(c.session_id.as_deref(), Some("sid-10.json"));
    assert_eq!(c.name.as_deref(), Some("n-10.json"));
    assert_eq!(c.messaging_socket.as_deref(), Some("/s/10.json"));
    cleanup(&sessions);
}

#[test]
fn load_claudes_skips_start_mismatch() {
    let (sessions, proc) = fixture("mismatch");
    register(&sessions, "10.json", "10", "100");
    process(&proc, 10, 101, "/w", b"AGENT_SESSION=a\0");
    assert!(load_claudes(&sessions, &proc).is_empty());
    cleanup(&sessions);
}

#[test]
fn load_claudes_skips_missing_process() {
    let (sessions, proc) = fixture("noproc");
    register(&sessions, "10.json", "10", "100");
    assert!(load_claudes(&sessions, &proc).is_empty());
    cleanup(&sessions);
}

#[test]
fn load_claudes_missing_proc_dir_is_empty() {
    let (sessions, proc) = fixture("noprocdir");
    register(&sessions, "10.json", "10", "100");
    fs::remove_dir_all(&proc).unwrap();
    assert!(load_claudes(&sessions, &proc).is_empty());
    cleanup(&sessions);
}

#[test]
fn load_claudes_skips_invalid_json() {
    let (sessions, proc) = fixture("badjson");
    fs::write(sessions.join("10.json"), "{not json").unwrap();
    process(&proc, 10, 100, "/w", b"AGENT_SESSION=a\0");
    register(&sessions, "11.json", "11", "100");
    process(&proc, 11, 100, "/w", b"AGENT_SESSION=b\0");
    let found = load_claudes(&sessions, &proc);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].pid, 11);
    cleanup(&sessions);
}

#[test]
fn load_claudes_accepts_string_pid() {
    let (sessions, proc) = fixture("strpid");
    register(&sessions, "10.json", r#""10""#, "100");
    process(&proc, 10, 100, "/w", b"AGENT_SESSION=a\0");
    let found = load_claudes(&sessions, &proc);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].pid, 10);
    cleanup(&sessions);
}

#[test]
fn load_claudes_agent_absent_empty_or_invalid_is_none() {
    let (sessions, proc) = fixture("agentnone");
    let envs: [&[u8]; 5] = [
        b"A=b\0",
        b"AGENT_SESSION=\0",
        b"AGENT_SESSION=-bad\0",
        b"AGENT_SESSION=a b\0",
        b"AGENT_SESSION=a/b\0",
    ];
    for (i, env) in envs.iter().enumerate() {
        let pid = 10 + i as u32;
        register(&sessions, &format!("{pid}.json"), &pid.to_string(), "100");
        process(&proc, pid, 100, "/w", env);
    }
    let found = load_claudes(&sessions, &proc);
    assert_eq!(found.len(), 5);
    assert!(found.iter().all(|c| c.agent.is_none()));
    cleanup(&sessions);
}

#[test]
fn load_claudes_ignores_shpool_session_name() {
    let (sessions, proc) = fixture("shpoolonly");
    register(&sessions, "10.json", "10", "100");
    process(&proc, 10, 100, "/w", b"SHPOOL_SESSION_NAME=jw-1\0");
    let found = load_claudes(&sessions, &proc);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].agent, None);
    cleanup(&sessions);
}

#[test]
fn load_claudes_orders_by_file_name() {
    let (sessions, proc) = fixture("order");
    for (file, pid) in [("b.json", 5), ("c.json", 3), ("a.json", 9)] {
        register(&sessions, file, &pid.to_string(), "100");
        process(&proc, pid, 100, "/w", b"AGENT_SESSION=x\0");
    }
    let pids: Vec<u32> = load_claudes(&sessions, &proc)
        .iter()
        .map(|c| c.pid)
        .collect();
    assert_eq!(pids, vec![9, 5, 3]);
    cleanup(&sessions);
}

#[test]
fn load_claudes_parses_stat_with_paren_and_spaces_in_comm() {
    let (sessions, proc) = fixture("parencomm");
    register(&sessions, "10.json", "10", "100");
    process(&proc, 10, 100, "/w", b"AGENT_SESSION=a\0");
    fs::write(proc.join("10/stat"), stat(10, "a) b (c", 100)).unwrap();
    let found = load_claudes(&sessions, &proc);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].pid, 10);
    cleanup(&sessions);
}

#[test]
fn load_claudes_skips_process_without_environ() {
    let (sessions, proc) = fixture("noenviron");
    register(&sessions, "10.json", "10", "100");
    process(&proc, 10, 100, "/w", b"AGENT_SESSION=a\0");
    fs::remove_file(proc.join("10/environ")).unwrap();
    assert!(load_claudes(&sessions, &proc).is_empty());
    cleanup(&sessions);
}

#[test]
fn load_claudes_skips_process_without_cwd() {
    let (sessions, proc) = fixture("nocwd");
    register(&sessions, "10.json", "10", "100");
    process(&proc, 10, 100, "/w", b"AGENT_SESSION=a\0");
    fs::remove_file(proc.join("10/cwd")).unwrap();
    assert!(load_claudes(&sessions, &proc).is_empty());
    cleanup(&sessions);
}

fn stat_ppid(pid: u32, ppid: u32) -> String {
    format!("{pid} (x) S {ppid} {} 0 0\n", ["0"; 17].join(" "))
}

fn node(proc: &Path, pid: u32, ppid: u32, environ: Option<&[u8]>) {
    let dir = proc.join(pid.to_string());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("stat"), stat_ppid(pid, ppid)).unwrap();
    if let Some(env) = environ {
        fs::write(dir.join("environ"), env).unwrap();
    }
}

fn claude(pid: u32, agent: Option<&str>) -> Claude {
    Claude {
        pid,
        cwd: "/w".into(),
        agent: agent.map(String::from),
        status: None,
        waiting_for: None,
        status_updated_at: None,
        session_id: None,
        name: None,
        messaging_socket: None,
    }
}

fn set(pids: &[u32]) -> std::collections::HashSet<u32> {
    pids.iter().copied().collect()
}

const A: &[u8] = b"X=y\0AGENT_SESSION=a\0";
const B: &[u8] = b"AGENT_SESSION=b\0";

#[test]
fn session_procs_jw_tree() {
    let (sessions, proc) = fixture("sp-jw");
    node(&proc, 100, 1, Some(b"X=y\0"));
    node(&proc, 10, 100, Some(A));
    node(&proc, 11, 10, Some(A));
    assert_eq!(
        session_procs(&[claude(10, Some("a"))], &proc),
        set(&[10, 11])
    );
    cleanup(&sessions);
}

#[test]
fn session_procs_event_tree() {
    let (sessions, proc) = fixture("sp-event");
    node(&proc, 100, 1, Some(b"X=y\0"));
    node(&proc, 20, 100, Some(A));
    node(&proc, 21, 20, Some(A));
    node(&proc, 22, 21, Some(A));
    node(&proc, 23, 22, Some(A));
    node(&proc, 24, 20, Some(A));
    node(&proc, 25, 23, Some(B));
    node(&proc, 26, 25, None);
    assert_eq!(
        session_procs(&[claude(22, Some("a"))], &proc),
        set(&[20, 21, 22, 23, 25, 26])
    );
    cleanup(&sessions);
}

#[test]
fn session_procs_nested_claudes_descendants_not_blocked_by_ancestor_walk() {
    let (sessions, proc) = fixture("sp-nested");
    node(&proc, 100, 1, Some(b"X=y\0"));
    node(&proc, 81, 100, Some(A));
    node(&proc, 82, 81, Some(A));
    node(&proc, 83, 82, Some(A));
    node(&proc, 84, 83, Some(A));
    node(&proc, 85, 81, Some(b"X=y\0"));
    assert_eq!(
        session_procs(&[claude(83, Some("a")), claude(81, Some("a"))], &proc),
        set(&[81, 82, 83, 84, 85])
    );
    cleanup(&sessions);
}

#[test]
fn session_procs_other_marker_stops_walk() {
    let (sessions, proc) = fixture("sp-other");
    node(&proc, 30, 1, Some(B));
    node(&proc, 31, 30, Some(A));
    node(&proc, 32, 31, Some(A));
    assert_eq!(
        session_procs(&[claude(32, Some("a"))], &proc),
        set(&[31, 32])
    );
    cleanup(&sessions);
}

#[test]
fn session_procs_agent_none_skips_ancestors() {
    let (sessions, proc) = fixture("sp-none");
    node(&proc, 40, 1, Some(A));
    node(&proc, 41, 40, Some(A));
    assert_eq!(session_procs(&[claude(41, None)], &proc), set(&[41]));
    cleanup(&sessions);
}

#[test]
fn session_procs_empty_or_invalid_marker_stops_walk() {
    let envs: [&[u8]; 2] = [b"AGENT_SESSION=\0", b"AGENT_SESSION=-bad\0"];
    for (i, env) in envs.iter().enumerate() {
        let (sessions, proc) = fixture(&format!("sp-badmarker{i}"));
        node(&proc, 80, 1, Some(A));
        node(&proc, 81, 80, Some(env));
        node(&proc, 82, 81, Some(A));
        assert_eq!(session_procs(&[claude(82, Some("a"))], &proc), set(&[82]));
        cleanup(&sessions);
    }
}

#[test]
fn session_procs_unrelated_processes_excluded() {
    let (sessions, proc) = fixture("sp-unrelated");
    node(&proc, 100, 1, Some(b"X=y\0"));
    node(&proc, 10, 100, Some(A));
    node(&proc, 50, 100, Some(b"X=y\0"));
    node(&proc, 60, 100, Some(B));
    assert_eq!(session_procs(&[claude(10, Some("a"))], &proc), set(&[10]));
    cleanup(&sessions);
}

#[test]
fn session_procs_missing_ancestor_dir_stops_walk() {
    let (sessions, proc) = fixture("sp-missingdir");
    node(&proc, 91, 90, Some(A));
    node(&proc, 92, 91, Some(A));
    assert_eq!(
        session_procs(&[claude(92, Some("a"))], &proc),
        set(&[91, 92])
    );
    cleanup(&sessions);
}

#[test]
fn session_procs_ancestor_without_environ_stops_walk() {
    let (sessions, proc) = fixture("sp-noenviron");
    node(&proc, 89, 1, Some(A));
    node(&proc, 90, 89, None);
    node(&proc, 91, 90, Some(A));
    node(&proc, 92, 91, Some(A));
    assert_eq!(
        session_procs(&[claude(92, Some("a"))], &proc),
        set(&[91, 92])
    );
    cleanup(&sessions);
}

#[test]
fn session_procs_cycle_terminates() {
    let (sessions, proc) = fixture("sp-cycle");
    node(&proc, 70, 71, Some(A));
    node(&proc, 71, 70, Some(A));
    assert_eq!(
        session_procs(&[claude(70, Some("a"))], &proc),
        set(&[70, 71])
    );
    cleanup(&sessions);
}

#[test]
fn session_procs_two_claudes_union() {
    let (sessions, proc) = fixture("sp-union");
    node(&proc, 100, 1, Some(b"X=y\0"));
    node(&proc, 10, 100, Some(A));
    node(&proc, 11, 10, Some(A));
    node(&proc, 20, 100, Some(B));
    node(&proc, 21, 20, Some(B));
    node(&proc, 22, 21, Some(B));
    assert_eq!(
        session_procs(&[claude(11, Some("a")), claude(22, Some("b"))], &proc),
        set(&[10, 11, 20, 21, 22])
    );
    cleanup(&sessions);
}

#[test]
fn session_procs_absent_pid_and_missing_proc_dir() {
    let (sessions, proc) = fixture("sp-absent");
    assert_eq!(session_procs(&[claude(5, Some("a"))], &proc), set(&[5]));
    fs::remove_dir_all(&proc).unwrap();
    assert_eq!(
        session_procs(&[claude(5, Some("a")), claude(6, None)], &proc),
        set(&[5, 6])
    );
    cleanup(&sessions);
}
