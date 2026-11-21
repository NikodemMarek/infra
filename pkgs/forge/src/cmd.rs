use std::io::{ErrorKind, Write};
use std::process::{Command, Stdio};

pub type Result<T> = std::result::Result<T, String>;

pub struct Output {
    pub ok: bool,
    pub stdout: String,
}

pub fn run(cmd: &[&str], check: bool) -> Result<Output> {
    run_with_input(cmd, None, check)
}

pub fn run_with_input(cmd: &[&str], input: Option<&str>, check: bool) -> Result<Output> {
    let fail = |e: std::io::Error| {
        if e.kind() == ErrorKind::NotFound {
            format!("{} not found on PATH", cmd[0])
        } else {
            format!("cannot run {}: {e}", cmd[0])
        }
    };
    let mut child = Command::new(cmd[0])
        .args(&cmd[1..])
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(fail)?;
    // written on its own thread so a large input cannot deadlock against unread output
    let out = std::thread::scope(|s| {
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            s.spawn(move || {
                // a command that fails early closes the pipe; its exit status and output explain why
                let _ = stdin.write_all(input.as_bytes());
            });
        }
        child.wait_with_output()
    })
    .map_err(fail)?;
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
