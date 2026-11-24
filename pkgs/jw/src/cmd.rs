use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs cmd with input on stdin (null if none) and its stdout captured; None if it did not finish within limit.
/// stderr is captured only if the caller made it a pipe.
pub fn output_timeout(
    cmd: &mut Command,
    input: Option<String>,
    limit: Duration,
) -> io::Result<Option<Output>> {
    let stdin = if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    };
    let mut child = cmd.stdin(stdin).stdout(Stdio::piped()).spawn()?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    };
    Ok(Some(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    }))
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

/// Runs cmd with input on stdin and its stdout captured.
pub fn output_with_input(cmd: &mut Command, input: &str) -> io::Result<Output> {
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    child.wait_with_output()
}

/// Publishes an event of kind with payload (a JSON object) through `event-pub`.
pub fn event_pub(kind: &str, payload: &str) -> Result<(), String> {
    let mut cmd = Command::new("event-pub");
    cmd.arg(kind).stderr(Stdio::piped());
    let out = match output_timeout(&mut cmd, Some(payload.to_string()), PUBLISH_TIMEOUT) {
        Ok(Some(out)) => out,
        Ok(None) => return Err("event-pub timed out".to_string()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err("event-pub not found".to_string())
        }
        Err(e) => return Err(e.to_string()),
    };
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let last = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty());
    Err(match (last, out.status.code()) {
        (Some(line), _) => line.to_string(),
        (None, Some(code)) => format!("event-pub exited with {code}"),
        (None, None) => "event-pub killed by a signal".to_string(),
    })
}

pub fn stdin_is_tty() -> bool {
    // SAFETY: isatty only inspects the file descriptor
    unsafe { libc::isatty(0) == 1 }
}

pub fn which(tool: &str) -> bool {
    env::var_os("PATH").is_some_and(|path| {
        env::split_paths(&path).any(|dir| {
            fs::metadata(dir.join(tool))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}
