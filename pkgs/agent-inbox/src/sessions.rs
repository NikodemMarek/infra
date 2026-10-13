//! Live Claude Code sessions, read from the session registry (`<config dir>/sessions/<pid>.json`,
//! written by Claude Code) and /proc. Read-only: registry files are never written or deleted,
//! stale ones are only skipped.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// One running Claude session. Times are seconds since the epoch (the registry has milliseconds).
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub pid: u32,
    pub session_id: String,
    pub cwd: String,
    pub name: String,
    /// `busy`, `idle`, `waiting`, ...; "" if the registry has none.
    pub status: String,
    /// What a `waiting` session waits for (`permission prompt`), "" otherwise.
    pub waiting_for: String,
    pub kind: String,
    pub started_at: f64,
    /// When the status last changed.
    pub status_at: f64,
    /// Start time of the process in clock ticks since boot (`procStart`); None if unknown.
    pub start: Option<u64>,
    /// The shpool session hosting the process (`shpool attach <name>`), "" if there is none.
    pub shpool: String,
}

impl Session {
    /// Where the session runs: `repo:workspace` for a cwd in `<repo>.agents/<workspace>`, else
    /// the last component of the cwd.
    pub fn place(&self) -> String {
        let parts: Vec<&str> = self.cwd.split('/').collect();
        let workspace = parts.iter().enumerate().find_map(|(i, c)| {
            let repo = c.strip_suffix(".agents").filter(|r| !r.is_empty())?;
            let workspace = parts.get(i + 1).filter(|w| !w.is_empty())?;
            Some(format!("{repo}:{workspace}"))
        });
        workspace
            .or_else(|| {
                parts
                    .iter()
                    .rev()
                    .find(|c| !c.is_empty())
                    .map(|c| c.to_string())
            })
            .unwrap_or_default()
    }
}

/// `$CLAUDE_CONFIG_DIR`, else `~/.local/share/claude`; the registry is in its `sessions` directory.
pub fn config_dir() -> PathBuf {
    env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(
            || PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/share/claude"),
            PathBuf::from,
        )
}

/// The live sessions of the registry in `sessions_dir`, newest first. Files that do not parse
/// and sessions whose process is gone (or whose pid was reused) are skipped; a missing
/// directory means no sessions.
pub fn load(sessions_dir: &Path, proc: &Path) -> Vec<Session> {
    let Ok(entries) = fs::read_dir(sessions_dir) else {
        return Vec::new();
    };
    let mut sessions: Vec<Session> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| parse(&fs::read_to_string(p).ok()?))
        .filter(|s| alive(proc, s.pid, s.start))
        .map(|mut s| {
            s.shpool = shpool_name(proc, s.pid);
            s
        })
        .collect();
    sessions.sort_by(|a, b| b.started_at.total_cmp(&a.started_at));
    sessions
}

/// One registry file; None without a pid. Missing text fields are empty.
fn parse(text: &str) -> Option<Session> {
    let v: Value = serde_json::from_str(text).ok()?;
    let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let secs = |key: &str| v.get(key).and_then(Value::as_f64).map(|ms| ms / 1000.0);
    let started_at = secs("startedAt").unwrap_or(0.0);
    Some(Session {
        pid: u32::try_from(v.get("pid")?.as_u64()?).ok()?,
        session_id: text("sessionId"),
        cwd: text("cwd"),
        name: text("name"),
        status: text("status"),
        waiting_for: text("waitingFor"),
        kind: text("kind"),
        started_at,
        status_at: secs("statusUpdatedAt")
            .or_else(|| secs("updatedAt"))
            .unwrap_or(started_at),
        // a procStart that cannot be read is a start time that does not match
        start: match v.get("procStart") {
            None => None,
            Some(p) => Some(p.as_str()?.parse().ok()?),
        },
        shpool: String::new(),
    })
}

/// Whether `pid` is a live process that started at `start` (field 22 of `/proc/<pid>/stat`);
/// a different start time means the pid was reused. Without a start time the process only has
/// to exist.
pub fn alive(proc: &Path, pid: u32, start: Option<u64>) -> bool {
    let Ok(stat) = fs::read(proc.join(pid.to_string()).join("stat")) else {
        return false;
    };
    let Some(start) = start else {
        return true;
    };
    let stat = String::from_utf8_lossy(&stat);
    // the command name (field 2) may contain spaces and parentheses: count from the last `)`;
    // what follows starts with field 3, so field 22 is token 19
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return false;
    };
    rest.split_whitespace()
        .nth(19)
        .and_then(|t| t.parse::<u64>().ok())
        == Some(start)
}

/// `SHPOOL_SESSION_NAME` of the process, "" if it has none or its environment is unreadable.
fn shpool_name(proc: &Path, pid: u32) -> String {
    const VAR: &[u8] = b"SHPOOL_SESSION_NAME=";
    fs::read(proc.join(pid.to_string()).join("environ"))
        .ok()
        .and_then(|env| {
            env.split(|&b| b == 0)
                .find_map(|kv| kv.strip_prefix(VAR))
                .map(|v| String::from_utf8_lossy(v).into_owned())
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh `(sessions dir, proc dir)` under the temp dir.
    fn fixture(name: &str) -> (PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("agent-inbox-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (sessions, proc) = (root.join("sessions"), root.join("proc"));
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(&proc).unwrap();
        (sessions, proc)
    }

    fn cleanup(sessions: &Path) {
        fs::remove_dir_all(sessions.parent().unwrap()).unwrap();
    }

    /// `/proc/<pid>/stat` of a process called `comm` that started at `start`.
    fn stat(pid: u32, comm: &str, start: u64) -> String {
        // fields 3..21: state and 18 numbers, then the start time
        format!("{pid} ({comm}) S {} {start} 0 0\n", ["0"; 18].join(" "))
    }

    /// `<proc>/<pid>/stat` and, if given, `environ`.
    fn process(proc: &Path, pid: u32, stat: &str, environ: Option<&[u8]>) {
        let dir = proc.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("stat"), stat).unwrap();
        if let Some(environ) = environ {
            fs::write(dir.join("environ"), environ).unwrap();
        }
    }

    /// `<sessions>/<pid>.json`; `proc_start` is the `procStart` string, None leaves it out.
    fn register(sessions: &Path, pid: u32, started_ms: u64, proc_start: Option<&str>) {
        let proc_start = proc_start.map_or(String::new(), |s| format!(r#""procStart":"{s}","#));
        fs::write(
            sessions.join(format!("{pid}.json")),
            format!(
                r#"{{"pid":{pid},"sessionId":"sid-{pid}","cwd":"/home/u/projects/r.agents/w1","startedAt":{started_ms},{proc_start}"kind":"interactive","name":"n{pid}","status":"waiting","updatedAt":5000,"statusUpdatedAt":7000,"waitingFor":"permission prompt"}}"#
            ),
        )
        .unwrap();
    }

    fn session_at(cwd: &str) -> Session {
        Session {
            pid: 1,
            session_id: String::new(),
            cwd: cwd.to_string(),
            name: String::new(),
            status: String::new(),
            waiting_for: String::new(),
            kind: String::new(),
            started_at: 0.0,
            status_at: 0.0,
            start: None,
            shpool: String::new(),
        }
    }

    #[test]
    fn live_session_with_shpool_name() {
        let (sessions, proc) = fixture("shpool");
        register(&sessions, 10, 2000, Some("100"));
        process(
            &proc,
            10,
            &stat(10, "claude", 100),
            Some(b"A=b\0SHPOOL_SESSION_NAME=jw-1\0C=d\0"),
        );
        assert_eq!(
            load(&sessions, &proc),
            vec![Session {
                pid: 10,
                session_id: "sid-10".into(),
                cwd: "/home/u/projects/r.agents/w1".into(),
                name: "n10".into(),
                status: "waiting".into(),
                waiting_for: "permission prompt".into(),
                kind: "interactive".into(),
                started_at: 2.0,
                status_at: 7.0,
                start: Some(100),
                shpool: "jw-1".into(),
            }]
        );
        cleanup(&sessions);
    }

    #[test]
    fn live_session_without_shpool_name() {
        let (sessions, proc) = fixture("noshpool");
        register(&sessions, 10, 2000, Some("100"));
        process(&proc, 10, &stat(10, "claude", 100), Some(b"A=b\0C=d\0"));
        register(&sessions, 11, 1000, Some("100"));
        process(&proc, 11, &stat(11, "claude", 100), None);
        let found = load(&sessions, &proc);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|s| s.shpool.is_empty()));
        cleanup(&sessions);
    }

    #[test]
    fn status_time_falls_back_to_updated_at() {
        let s = parse(r#"{"pid":1,"startedAt":1000,"updatedAt":3000}"#).unwrap();
        assert_eq!((s.started_at, s.status_at), (1.0, 3.0));
        let s = parse(r#"{"pid":1,"startedAt":1000}"#).unwrap();
        assert_eq!(s.status_at, 1.0);
        assert_eq!(s.status, "");
        assert_eq!(s.waiting_for, "");
    }

    #[test]
    fn dead_process_is_skipped() {
        let (sessions, proc) = fixture("dead");
        register(&sessions, 10, 2000, Some("100"));
        assert!(load(&sessions, &proc).is_empty());
        assert!(load(&sessions, &proc.join("missing")).is_empty());
        cleanup(&sessions);
    }

    #[test]
    fn reused_pid_is_skipped() {
        let (sessions, proc) = fixture("reused");
        register(&sessions, 10, 2000, Some("100"));
        process(&proc, 10, &stat(10, "bash", 999), None);
        assert!(load(&sessions, &proc).is_empty());
        cleanup(&sessions);
    }

    #[test]
    fn missing_proc_start_only_needs_the_process() {
        let (sessions, proc) = fixture("nostart");
        register(&sessions, 10, 2000, None);
        register(&sessions, 11, 1000, Some("not a number"));
        process(&proc, 10, &stat(10, "claude", 100), None);
        process(&proc, 11, &stat(11, "claude", 100), None);
        let found = load(&sessions, &proc);
        // the file with an unreadable procStart is skipped
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].pid, found[0].start), (10, None));
        cleanup(&sessions);
    }

    #[test]
    fn unparsable_files_are_skipped() {
        let (sessions, proc) = fixture("malformed");
        register(&sessions, 10, 2000, Some("100"));
        process(&proc, 10, &stat(10, "claude", 100), None);
        fs::write(sessions.join("11.json"), "{not json").unwrap();
        fs::write(sessions.join("12.json"), r#"{"sessionId":"no pid"}"#).unwrap();
        fs::write(sessions.join("13.json"), "[]").unwrap();
        fs::write(sessions.join("notes.txt"), r#"{"pid":14}"#).unwrap();
        process(&proc, 14, &stat(14, "claude", 100), None);
        let found = load(&sessions, &proc);
        assert_eq!(found.iter().map(|s| s.pid).collect::<Vec<_>>(), vec![10]);
        cleanup(&sessions);
    }

    #[test]
    fn missing_sessions_dir_is_empty() {
        let (sessions, proc) = fixture("nodir");
        assert!(load(&sessions.join("gone"), &proc).is_empty());
        cleanup(&sessions);
    }

    #[test]
    fn comm_may_contain_parentheses_and_spaces() {
        let (sessions, proc) = fixture("comm");
        process(&proc, 10, &stat(10, "a) (b c", 100), None);
        assert!(alive(&proc, 10, Some(100)));
        assert!(!alive(&proc, 10, Some(101)));
        assert!(alive(&proc, 10, None));
        // garbage and truncated stat files never match a start time
        process(&proc, 11, "no parenthesis", None);
        process(&proc, 12, "12 (x) S 1 2", None);
        assert!(!alive(&proc, 11, Some(100)));
        assert!(!alive(&proc, 12, Some(100)));
        assert!(!alive(&proc, 13, None));
        cleanup(&sessions);
    }

    #[test]
    fn newest_session_comes_first() {
        let (sessions, proc) = fixture("order");
        for (pid, started) in [(10, 2000), (11, 9000), (12, 5000)] {
            register(&sessions, pid, started, Some("100"));
            process(&proc, pid, &stat(pid, "claude", 100), None);
        }
        let found = load(&sessions, &proc);
        assert_eq!(
            found.iter().map(|s| s.pid).collect::<Vec<_>>(),
            vec![11, 12, 10]
        );
        cleanup(&sessions);
    }

    #[test]
    fn place_is_repo_and_workspace_else_last_component() {
        let place = |cwd: &str| session_at(cwd).place();
        assert_eq!(
            place("/home/u/projects/cpd-versions.agents/gl-review-2213"),
            "cpd-versions:gl-review-2213"
        );
        assert_eq!(place("/home/u/projects/r.agents/w1/pkgs/x"), "r:w1");
        assert_eq!(place("/home/u/projects/real-dotfiles"), "real-dotfiles");
        assert_eq!(place("/home/u/projects/real-dotfiles/"), "real-dotfiles");
        // a bare `.agents` or a missing workspace names no repo
        assert_eq!(place("/x/.agents/w1"), "w1");
        assert_eq!(place("/x/r.agents"), "r.agents");
        assert_eq!(place("/"), "");
        assert_eq!(place(""), "");
    }
}
