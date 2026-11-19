use std::io::ErrorKind;
use std::process::{Command, Stdio};

pub type Result<T> = std::result::Result<T, String>;

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

pub fn run(cmd: &[&str], check: bool) -> Result<Output> {
    let out = Command::new(cmd[0])
        .args(&cmd[1..])
        .stdin(Stdio::inherit())
        .output()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                format!("{} not found on PATH", cmd[0])
            } else {
                format!("cannot run {}: {e}", cmd[0])
            }
        })?;
    let output = Output {
        ok: out.status.success(),
        stdout: text(&out.stdout),
        stderr: text(&out.stderr),
    };
    if check && !output.ok {
        return Err(format!(
            "`{}` failed:\n{}",
            cmd.join(" "),
            output.stderr.trim()
        ));
    }
    Ok(output)
}
