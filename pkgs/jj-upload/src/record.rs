use std::env;

use crate::cmd::{run, Result};

fn env_nonempty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
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
    let out = run(&["jw", "session", "set", "--mr", url], false)?;
    if out.ok {
        return Ok(());
    }
    let stderr = out.stderr.trim();
    Err(if stderr.is_empty() {
        "jw failed".to_string()
    } else {
        stderr.to_string()
    })
}
