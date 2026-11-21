use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{env, fs};

use serde_json::Value;

use crate::cmd::Result;
use crate::jj::Jj;

fn env_nonempty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

fn quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn repo_name(root: &str) -> String {
    let path = Path::new(root);
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let parent = path.parent().map(name).unwrap_or_default();
    match parent.strip_suffix(".agents") {
        Some(repo) => repo.to_string(),
        None => name(path),
    }
}

fn write_record(branch: &str, session_id: &str) -> Result<()> {
    let root = fs::canonicalize(Jj::root()?).map_err(|e| e.to_string())?;
    let root = root.to_string_lossy();
    let state_home = match env_nonempty("XDG_STATE_HOME") {
        Some(dir) => dir,
        None => format!(
            "{}/.local/state",
            env_nonempty("HOME").ok_or("HOME is not set")?
        ),
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs_f64();
    let path = format!(
        "{state_home}/jj-upload/{}/{}.json",
        repo_name(&root),
        quote(branch)
    );
    let tmp = format!("{path}.tmp");
    let record = format!(
        r#"{{"workspace": {}, "session_id": {}, "uploaded_at": {now:?}}}"#,
        Value::from(&*root),
        Value::from(session_id)
    );
    if let Some(dir) = Path::new(&path).parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&tmp, record).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub fn record_session(branch: &str) {
    let Some(session_id) = env_nonempty("CLAUDE_CODE_SESSION_ID") else {
        return;
    };
    if let Err(e) = write_record(branch, &session_id) {
        eprintln!("jj upload: cannot record the session for {branch}: {e}");
    }
}

pub fn record_mr(url: &str) {
    let Some(name) = env_nonempty("AGENT_SESSION") else {
        return;
    };
    if let Err(e) = set_session_mr(url) {
        eprintln!("jj upload: cannot add {url} to session {name}: {e}");
    }
}

fn set_session_mr(url: &str) -> Result<()> {
    let mut child = Command::new("jw")
        .args(["session", "set", "--mr", url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut pipe = child.stderr.take().ok_or("no stderr pipe")?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if Instant::now() < deadline => sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out after 10 seconds".to_string());
            }
        }
    };
    if status.success() {
        return Ok(());
    }
    let stderr = rx
        .recv_timeout(Duration::from_millis(500))
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .unwrap_or_default();
    Err(if stderr.is_empty() {
        format!("exit {}", status.code().unwrap_or(-1))
    } else {
        stderr
    })
}
