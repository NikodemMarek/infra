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
