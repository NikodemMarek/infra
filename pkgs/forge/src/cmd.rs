use std::io::ErrorKind;
use std::process::{Command, Stdio};

pub type Result<T> = std::result::Result<T, String>;

pub struct Output {
    pub ok: bool,
    pub stdout: String,
}

pub fn run(cmd: &[&str], check: bool) -> Result<Output> {
    let out = Command::new(cmd[0])
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                format!("{} not found on PATH", cmd[0])
            } else {
                format!("cannot run {}: {e}", cmd[0])
            }
        })?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if check && !out.status.success() {
        // gh and glab print the API error body on stdout and a summary on stderr
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail: Vec<&str> = [stderr.trim(), stdout.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        return Err(format!(
            "`{}` failed:\n{}",
            cmd.join(" "),
            detail.join("\n")
        ));
    }
    Ok(Output {
        ok: out.status.success(),
        stdout,
    })
}
