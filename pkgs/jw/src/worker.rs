use std::collections::VecDeque;
use std::env;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::agent::{latest_transcript, session_repo};
use crate::cmd::event_pub;
use crate::record::{now, Record, Records, ResultKind};
use crate::systemd::Systemd;

const DELIVER_WAIT: Duration = Duration::from_secs(10);
const DELIVER_POLL: Duration = Duration::from_millis(200);
const STDERR_TAIL: usize = 20;
const INPUT_CLIP: usize = 200;
/// Below this the context is mostly the system prompt: a summary would save little.
const COMPACT_MIN_TOKENS: u64 = 50_000;
/// Leaves room for an immediate follow-up message, which cancels the compaction.
const COMPACT_DELAY_SECS: u64 = 120;
const COMPACT_INSTRUCTIONS: &str = "/compact Keep the task and its decisions, the uploaded MRs/PRs with their state and open review threads, who this session reports to, and what is left to do.";

/// The transient user unit that runs the turns of headless worker name.
pub fn unit(name: &str) -> String {
    format!("jw-worker-{name}")
}

/// Writes the result of a turn of worker name (see `Records::write_result`) and, if it was written, publishes
/// `session.result` for it.
pub fn finish(
    name: &str,
    kind: ResultKind,
    text: &str,
    guard: impl FnOnce(&mut Record) -> bool,
) -> Result<Option<u64>, String> {
    let seq = Records::new().write_result(name, kind, text, guard)?;
    if let Some(seq) = seq {
        let payload = serde_json::json!({"type": "session.result", "session": name, "seq": seq});
        if let Err(e) = event_pub("session.result", &payload.to_string()) {
            let _ = writeln!(io::stderr(), "jw: {name}: {e}");
        }
    }
    Ok(seq)
}

pub enum Delivery {
    /// An open turn of the worker picks the message up.
    Queued,
    /// A new turn was started for it.
    Started,
}

/// The `jw` that runs the turns: the wrapper beside the executable (wrapProgram renames the binary), else the executable.
fn jw_exe() -> Result<String, String> {
    let exe = env::current_exe().map_err(|e| format!("cannot find the jw executable: {e}"))?;
    let wrapper = exe.with_file_name("jw");
    let jw = if wrapper.is_file() { wrapper } else { exe };
    Ok(jw.to_string_lossy().into_owned())
}

fn enqueue(queue: &Path, name: &str, text: &str) -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = queue.join(format!("{nanos}-{}.txt", process::id()));
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(queue)
        .and_then(|()| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?
                .write_all(text.as_bytes())
                .inspect_err(|_| {
                    let _ = fs::remove_file(&path);
                })
        })
        .map_err(|e| format!("cannot queue the message for {name}: {e}"))?;
    Ok(path)
}

/// Waits until a turn of worker name runs or is started; Err is the last reason it could not.
fn start_turn(records: &Records, name: &str, record: Record) -> Result<Delivery, String> {
    let workspace = record.workspace.ok_or("the record has no workspace")?;
    let unit_name = unit(name);
    let argv = vec![
        jw_exe()?,
        "worker".to_string(),
        "turn".to_string(),
        name.to_string(),
    ];
    let mut open = record.turn_base == Some(record.result_seq);
    let deadline = Instant::now() + DELIVER_WAIT;
    loop {
        let last: String;
        match Systemd.is_active(&unit_name) {
            None => return Err(format!("cannot tell whether {name}'s turn runs")),
            Some(true) if open => return Ok(Delivery::Queued),
            // the runner is starting or exiting
            Some(true) => last = format!("the turn of {name} is ending"),
            Some(false) => {
                match Systemd.run(&unit_name, &workspace, &["RuntimeMaxSec=4h"], &argv) {
                    Ok(()) => return Ok(Delivery::Started),
                    Err(e) => last = e,
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(last);
        }
        thread::sleep(DELIVER_POLL);
        let current = records.update(name, |_| false)?;
        open = current.turn_base == Some(current.result_seq);
    }
}

/// Queues text for worker name and makes sure a turn will run it.
pub fn deliver(name: &str, text: &str) -> Result<Delivery, String> {
    let records = Records::new();
    let queue = records.queue_dir(name);
    let mut entry = Err(String::new());
    // the turn runner reads the queue under the same lock: an entry is either in its turn or seen as open here
    let record = records.update(name, |_| {
        entry = enqueue(&queue, name, text);
        false
    })?;
    let entry = entry?;
    let e = match start_turn(&records, name, record) {
        Ok(delivery) => return Ok(delivery),
        Err(e) => e,
    };
    // under the lock the runner cannot open a turn between the check and the removal; a turn open now has the entry
    // or takes it next, so it stays
    let mut queued = false;
    let cleaned = records.update(name, |r| {
        queued = r.turn_base == Some(r.result_seq) && Systemd.is_active(&unit(name)) == Some(true);
        if !queued {
            let _ = fs::remove_file(&entry);
        }
        false
    });
    if cleaned.is_err() {
        let _ = fs::remove_file(&entry);
    }
    if queued {
        return Ok(Delivery::Queued);
    }
    Err(format!("cannot start a turn of {name}: {e}"))
}

fn read_queue(dir: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            Ok((path, text))
        })
        .collect()
}

struct Turn {
    /// The result_seq the turn starts at.
    base: u64,
    /// The queued messages by age.
    entries: Vec<(PathBuf, String)>,
}

/// Opens a turn on the queued messages of worker name; None, and no open turn, if the queue is empty.
fn open_turn(records: &Records, name: &str) -> Result<Option<Turn>, String> {
    let queue = records.queue_dir(name);
    let mut queued = Ok(vec![]);
    let mut base = 0;
    records.update(name, |r| {
        queued = read_queue(&queue);
        match &queued {
            Ok(entries) if !entries.is_empty() => {
                base = r.result_seq;
                r.turn_base = Some(base);
                if r.status.as_deref() != Some("done") {
                    r.status = Some("busy".to_string());
                    r.status_at = Some(now());
                }
                true
            }
            Ok(_) => r.turn_base.take().is_some(),
            Err(_) => false,
        }
    })?;
    let entries = queued?;
    Ok((!entries.is_empty()).then_some(Turn { base, entries }))
}

fn clip(value: Option<&Value>) -> String {
    value
        .map_or_else(|| "null".to_string(), Value::to_string)
        .chars()
        .take(INPUT_CLIP)
        .collect()
}

fn summary(event: &Value) -> String {
    let field = |key: &str| match event.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "?".to_string(),
    };
    format!(
        "turn: cost_usd={} duration_ms={} num_turns={} is_error={}",
        field("total_cost_usd"),
        field("duration_ms"),
        field("num_turns"),
        field("is_error")
    )
}

fn result_text(event: &Value) -> String {
    let result = event.get("result").and_then(Value::as_str).unwrap_or("");
    let failed = event.get("is_error").and_then(Value::as_bool) == Some(true);
    let mut text = if result.is_empty() && failed {
        let subtype = event.get("subtype").and_then(Value::as_str).unwrap_or("?");
        format!("turn failed ({subtype})")
    } else {
        result.to_string()
    };
    let denials = event
        .get("permission_denials")
        .and_then(Value::as_array)
        .filter(|d| !d.is_empty());
    if let Some(denials) = denials {
        text.push_str("\n\nDenied tool calls (not run):");
        for denial in denials {
            let tool = denial
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let input = clip(denial.get("tool_input"));
            text.push_str(&format!("\n- {tool}: {input}"));
        }
    }
    text
}

fn died_text(name: &str, ended: &str, stderr_tail: &[String]) -> String {
    format!(
        "claude ended without a result ({ended}).\nLast stderr lines:\n{}\nLog: journalctl --user -u {}",
        stderr_tail.join("\n"),
        unit(name)
    )
}

/// The tokens of the context an assistant event was generated from.
fn context_tokens(event: &Value) -> u64 {
    [
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ]
    .iter()
    .map(|key| {
        event
            .pointer(&format!("/message/usage/{key}"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    })
    .sum()
}

/// Runs claude on input as a turn of worker name in workspace and returns its result event and the size of its last
/// context in tokens; Err is the text of a result that says it ended without one.
fn run_claude(
    name: &str,
    workspace: &str,
    resume: Option<&str>,
    input: String,
) -> Result<(Value, u64), String> {
    let mut cmd = Command::new("direnv");
    cmd.args(["exec", ".", "claude", "-p"]);
    if let Some(id) = resume {
        cmd.args(["--resume", id]);
    }
    cmd.args([
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-prompts",
        "none",
        "--disallowedTools",
        "AskUserQuestion",
    ])
    .current_dir(workspace)
    .env("AGENT_SESSION", name)
    .env("AGENT_HEADLESS", "1")
    .env("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS", "0")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    // an ignored signal stays ignored across exec: claude must die on the unit's SIGTERM
    unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| died_text(name, &format!("cannot start: {e}"), &[]))?;
    // written and read concurrently: a large input or output cannot deadlock
    let stdin = child.stdin.take();
    thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(input.as_bytes());
        }
    });
    let stderr = child.stderr.take().map(|pipe| {
        thread::spawn(move || {
            let mut tail = VecDeque::new();
            for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line).into_owned();
                let _ = writeln!(io::stderr(), "{line}");
                if tail.len() == STDERR_TAIL {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            tail
        })
    });
    let mut result = None;
    let mut context = 0;
    let stdout = child.stdout.take();
    let lines = stdout
        .into_iter()
        .flat_map(|pipe| BufReader::new(pipe).split(b'\n'))
        .map_while(Result::ok);
    for line in lines {
        let Ok(event) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                context = context_tokens(&event);
                let blocks = event
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten();
                let tool_uses =
                    blocks.filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"));
                for block in tool_uses {
                    let tool = block.get("name").and_then(Value::as_str).unwrap_or("?");
                    let _ = writeln!(io::stdout(), "tool {tool} {}", clip(block.get("input")));
                }
            }
            Some("system")
                if event.get("subtype").and_then(Value::as_str) == Some("compact_boundary") =>
            {
                let tokens = |key: &str| {
                    event
                        .pointer(&format!("/compact_metadata/{key}"))
                        .map_or_else(|| "?".to_string(), Value::to_string)
                };
                let _ = writeln!(
                    io::stdout(),
                    "compact: pre_tokens={} post_tokens={}",
                    tokens("pre_tokens"),
                    tokens("post_tokens")
                );
            }
            Some("result") => {
                let _ = writeln!(io::stdout(), "{}", summary(&event));
                result = Some(event);
            }
            _ => {}
        }
    }
    let status = child.wait();
    let tail: Vec<String> = stderr
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
        .into();
    result.map(|event| (event, context)).ok_or_else(|| {
        let ended = match status {
            Ok(s) => match (s.code(), s.signal()) {
                (Some(code), _) => format!("exit status {code}"),
                (None, Some(signal)) => format!("signal {signal}"),
                (None, None) => "unknown status".to_string(),
            },
            Err(e) => e.to_string(),
        };
        died_text(name, &ended, &tail)
    })
}

/// The workspace of worker name and the transcript its claude resumes.
fn prepare(name: &str) -> Result<(String, Option<String>), String> {
    let (home, workspace) = session_repo(name)?;
    let resume = latest_transcript(&home, &workspace)?;
    Ok((workspace, resume))
}

/// Whether the worker with this record and queue is idle at result seq: not done, no open turn, nothing queued.
fn idle_at(record: &Record, queue: &Path, seq: u64) -> bool {
    record.result_seq == seq
        && record.turn_base != Some(record.result_seq)
        && record.status.as_deref() != Some("done")
        && read_queue(queue).is_ok_and(|entries| entries.is_empty())
}

/// Compacts the session of worker name if it is still idle at result seq, as an open turn that writes no result. The
/// status stays what it was: claude's status hooks run during the compaction too.
fn compact_session(records: &Records, name: &str, seq: u64) -> Result<(), String> {
    let queue = records.queue_dir(name);
    let mut idle = false;
    let mut status = None;
    let mut status_at = None;
    records.update(name, |r| {
        idle = idle_at(r, &queue, seq);
        if idle {
            r.turn_base = Some(seq);
            status = r.status.clone();
            status_at = r.status_at;
        }
        idle
    })?;
    if !idle {
        return Ok(());
    }
    let compacted = prepare(name).and_then(|(workspace, resume)| {
        run_claude(
            name,
            &workspace,
            resume.as_deref(),
            COMPACT_INSTRUCTIONS.to_string(),
        )
    });
    records.update(name, |r| {
        let ours =
            r.turn_base == Some(seq) && r.result_seq == seq && r.status.as_deref() != Some("done");
        if ours {
            r.status = status;
            r.status_at = status_at;
        }
        ours
    })?;
    compacted.map(|_| ())
}

/// Runs the queued messages of headless worker name as turns of claude, one result per turn, until the queue is empty.
/// With compact, first compacts the session if it is still idle at that result.
pub fn cmd_worker_turn(name: &str, compact: Option<u64>) -> Result<(), String> {
    let records = Records::new();
    if !records.read(name).is_some_and(|r| r.headless) {
        return Err(format!("{name} is not a headless worker"));
    }
    // a unit stop signals the whole cgroup: only claude dies, so the result is still written
    unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
    if let Some(seq) = compact {
        if let Err(e) = compact_session(&records, name, seq) {
            let _ = writeln!(io::stderr(), "jw: {name}: compaction failed: {e}");
        }
    }
    let mut context = None;
    while let Some(Turn { base, entries }) = open_turn(&records, name)? {
        context = None;
        let input = entries
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let (kind, text, tokens) = match prepare(name) {
            Ok((workspace, resume)) => {
                match run_claude(name, &workspace, resume.as_deref(), input) {
                    Ok((event, tokens)) => (ResultKind::Result, result_text(&event), tokens),
                    Err(text) => (ResultKind::Died, text, 0),
                }
            }
            Err(e) => (
                ResultKind::Died,
                format!("{e}\nLog: journalctl --user -u {}", unit(name)),
                0,
            ),
        };
        let written = finish(name, kind, &text, |r| {
            if r.turn_base != Some(base) {
                return false;
            }
            // after a death the messages stay: the next turn runs them again
            if kind == ResultKind::Result {
                for (path, _) in &entries {
                    let _ = fs::remove_file(path);
                }
            }
            if r.status.as_deref() != Some("done") {
                r.status = Some("idle".to_string());
            }
            r.status_at = Some(now());
            true
        })?;
        match written {
            None => {
                let _ = writeln!(
                    io::stderr(),
                    "jw: {name}: turn closed elsewhere, result dropped"
                );
            }
            Some(_) if kind == ResultKind::Died => break,
            Some(_) => context = Some(tokens),
        }
    }
    if context.is_some_and(|tokens| tokens >= COMPACT_MIN_TOKENS) {
        if let Err(e) = schedule_compact(&records, name) {
            let _ = writeln!(io::stderr(), "jw: {name}: cannot schedule compaction: {e}");
        }
    }
    Ok(())
}

/// Starts the compaction of worker name's session shortly, if it only waits for events on its open MRs.
fn schedule_compact(records: &Records, name: &str) -> Result<(), String> {
    let Some(record) = records.read(name).filter(|r| {
        r.status.as_deref() != Some("done") && r.mrs.iter().any(|mr| !r.merged_mrs.contains(mr))
    }) else {
        return Ok(());
    };
    let seq = record.result_seq;
    let argv = vec![
        jw_exe()?,
        "worker".to_string(),
        "compact".to_string(),
        name.to_string(),
        seq.to_string(),
    ];
    Systemd.run_later(
        &format!("jw-compact-{name}-{seq}"),
        COMPACT_DELAY_SECS,
        &argv,
    )
}

/// Starts the turn that compacts the session of headless worker name, unless it has had a result, a message, a turn
/// or an end since result seq.
pub fn cmd_worker_compact(name: &str, seq: u64) -> Result<(), String> {
    let records = Records::new();
    let Some(record) = records.read(name) else {
        return Ok(());
    };
    let Some(workspace) = record.workspace.as_deref() else {
        return Ok(());
    };
    if !record.headless
        || record.reaped.is_some()
        || !idle_at(&record, &records.queue_dir(name), seq)
        || Systemd.is_active(&unit(name)) != Some(false)
    {
        return Ok(());
    }
    let argv = vec![
        jw_exe()?,
        "worker".to_string(),
        "turn".to_string(),
        name.to_string(),
        "--compact".to_string(),
        seq.to_string(),
    ];
    // a message that started a turn meanwhile holds the unit: nothing to compact
    if let Err(e) = Systemd.run(&unit(name), workspace, &["RuntimeMaxSec=4h"], &argv) {
        let _ = writeln!(io::stderr(), "jw: {name}: compaction not started: {e}");
    }
    Ok(())
}
