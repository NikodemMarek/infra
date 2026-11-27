use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{self, Command};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use serde_json::{Map, Value};

use crate::cmd::{output_with_input, stdin_is_tty};
use crate::jj::Jj;
use crate::procs::{claude_dir, live_claudes};
use crate::record::{now, valid_name, write_error, Record, Records, MATE_KIND};
use crate::session::session_name;
use crate::shpool::Shpool;
use crate::worker;
use crate::workspace::{
    add_workspace, agent_workspace, basename, dirname, fetch, is_dir, layout, locate, pick,
    realpath, repo_home, repos, resolve_dir, root,
};
use crate::{AgentArgs, AttachArgs, ResumeArgs};

/// The word quoted for a POSIX shell, as shlex.quote does.
pub fn sh_quote(word: &str) -> String {
    let safe = |b: u8| b.is_ascii_alphanumeric() || b"@%+=:,./-_".contains(&b);
    if !word.is_empty() && word.bytes().all(safe) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r#"'"'"'"#))
    }
}

/// The environment without the variables that tie a session to the caller's own (repo key, nested-claude
/// marker). CLAUDE_CODE_* from a calling Claude (session id, child marker, messaging socket) would make the new Claude
/// a child of the caller: it writes into the caller's transcript and never registers, so jw msg cannot reach it.
fn agent_env() -> BTreeMap<String, String> {
    const DROP: [&str; 7] = [
        "CLAUDECODE",
        "CLAUDE_PID",
        "CLAUDE_EFFORT",
        "AI_AGENT",
        "CLAUDE_WORKSPACE_BASE",
        "AGENT_SESSION",
        "AGENT_HEADLESS",
    ];
    env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .filter(|(k, _)| !DROP.contains(&k.as_str()) && !k.starts_with("CLAUDE_CODE_"))
        .collect()
}

pub fn runtime_dir() -> String {
    env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|dir| !dir.is_empty())
        // SAFETY: getuid has no preconditions
        .unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }))
}

fn task_record(prompt: &str, workspace: &str, reports_to: Option<&str>) -> Record {
    let mut record = Record {
        kind: Some("task".to_string()),
        workspace: Some(workspace.to_string()),
        reports_to: reports_to.map(str::to_string),
        ..Record::default()
    };
    if let Some(first) = prompt.lines().map(str::trim).find(|l| !l.is_empty()) {
        record.title = Some(first.chars().take(200).collect());
        record.description = Some(prompt.chars().take(8000).collect());
    }
    record
}

pub fn cmd_agent(a: AgentArgs) -> Result<(), String> {
    let interactive = a.interactive;
    let ask_permissions = a.ask_permissions;
    let reports_to = a
        .reports_to
        .map(|name| {
            let name = session_name(Some(&name))?;
            match Records::new().read(&name) {
                Some(_) => Ok(name),
                None => Err(format!("no session record {name}")),
            }
        })
        .transpose()?;
    // clap drops a leading `--`; after it everything is the prompt, even a first word naming a repo
    let argv: Vec<String> = env::args().collect();
    let literal = argv
        .iter()
        .position(|w| w == "--")
        .is_some_and(|i| argv.len() - i - 1 == a.args.len());
    let args = a.args.as_slice();
    let first = args.first().filter(|f| !literal && !f.is_empty());
    let (home, prompt) = match first {
        Some(first) if repos().contains(first) => {
            let mut prompt = &args[1..];
            match prompt.first().map(String::as_str) {
                Some("--") => prompt = &prompt[1..],
                Some(opt)
                    if matches!(
                        opt,
                        "-r" | "--revision"
                            | "-d"
                            | "--detach"
                            | "--reports-to"
                            | "--interactive"
                            | "--ask-permissions"
                    ) || opt.starts_with("--revision=")
                        || opt.starts_with("--reports-to=") =>
                {
                    return Err("options go before the repo".to_string());
                }
                _ => {}
            }
            (format!("{}/{first}", root()), prompt)
        }
        _ => {
            if let Some(first) = first {
                if args.get(1).is_some_and(|s| s == "--") {
                    return Err(format!("{first}: not a repo"));
                }
                if Path::new(&format!("{}/{first}", root())).exists() {
                    return Err(format!("{first}: not a jj repo"));
                }
            }
            let home = match locate() {
                Ok((_, home)) => home,
                // not in a repo: the task's first word if it names one, else pick
                Err(_) => pick_for(&args.join(" "))?,
            };
            (home, args)
        }
    };
    match reports_to.as_deref().filter(|_| !interactive) {
        Some(supervisor) => start_headless(&home, a.revision, prompt, supervisor, ask_permissions),
        None => start_agent(
            &home,
            a.detach,
            a.revision,
            prompt,
            reports_to.as_deref(),
            false,
            ask_permissions,
        ),
    }
}

/// Starts a headless worker on the prompt in a new workspace of the repo at home: no shpool session, its turns run as
/// a unit (see worker.rs). The prompt is its first message.
fn start_headless(
    home: &str,
    revision: Option<String>,
    prompt: &[String],
    reports_to: &str,
    ask_permissions: bool,
) -> Result<(), String> {
    let text = prompt.join(" ");
    if text.trim().is_empty() {
        return Err("a headless session needs a prompt".to_string());
    }
    let revision = revision.filter(|r| !r.is_empty());
    let resolve_in = revision.as_ref().and_then(|_| resolve_dir(home));
    fetch(home);
    let name = {
        let _lock = naming_lock(&runtime_dir())?;
        let records = Records::new();
        let name = fresh_name(home, |name| records.read(name).is_some());
        let revision = revision.as_deref().unwrap_or("trunk()");
        let dir = add_workspace(home, revision, &name, resolve_in.as_deref())?;
        records
            .update(&name, |r| {
                r.merge(task_record(&text, &dir, Some(reports_to)));
                r.clear_marks();
                r.headless = true;
                r.ask_permissions = ask_permissions;
                true
            })
            .map_err(|e| write_error(&e))?;
        name
    };
    if let Err(e) = worker::deliver(&name, &text) {
        // reap retires the workspace of a done record
        let marked = Records::new().update(&name, |r| {
            r.status = Some("done".to_string());
            r.status_at = Some(now());
            r.waiting_for = None;
            true
        });
        return Err(match marked {
            Ok(_) => format!("{e}; record {name} marked done"),
            Err(m) => format!("{e}; record {name} not marked done: {m}"),
        });
    }
    eprintln!("jw: started {name} in {}", basename(home));
    Ok(())
}

/// Starts Claude on the prompt in a new workspace of the repo at home (a first mate if mate).
fn start_agent(
    home: &str,
    detach: bool,
    revision: Option<String>,
    prompt: &[String],
    reports_to: Option<&str>,
    mate: bool,
    ask_permissions: bool,
) -> Result<(), String> {
    let revision = revision.filter(|r| !r.is_empty());
    // the default trunk() is resolved in home without a snapshot
    let resolve_in = revision.as_ref().and_then(|_| resolve_dir(home));
    fetch(home);
    let text = prompt.join(" ");
    // one argument after `--`: claude takes a single prompt, and neither a first word like `update` nor a leading `-` is parsed
    let args: Vec<&str> = if prompt.is_empty() {
        vec![]
    } else {
        vec!["--", &text]
    };
    launch(
        home,
        detach,
        &args,
        &text,
        reports_to,
        |name| {
            let revision = revision.as_deref().unwrap_or("trunk()");
            add_workspace(home, revision, name, resolve_in.as_deref())
        },
        Naming::New,
        mate,
        ask_permissions,
    )
}

/// The repo for a task outside a repo: the one the task starts with, else a picked one.
fn pick_for(task: &str) -> Result<String, String> {
    let named = task
        .split_whitespace()
        .next()
        .filter(|word| repos().iter().any(|r| r == word));
    if let Some(name) = named {
        return Ok(format!("{}/{name}", root()));
    }
    if !stdin_is_tty() {
        return Err("no repo: pass one or start the task with its name".to_string());
    }
    repo_home(None)
}

/// How launch names the session.
#[derive(Clone, Copy)]
enum Naming<'a> {
    /// s-<time>
    New,
    /// That name if no shpool session has it, else s-<time>; an existing record of the name only gets its workspace
    /// updated.
    Resume(&'a str),
}

/// The lock under which names are picked and taken (session started or record written); held until dropped.
fn naming_lock(rundir: &str) -> Result<fs::File, String> {
    let lock_path = format!("{rundir}/jw-agent.lock");
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&lock_path)
        .map_err(|e| format!("{lock_path}: {e}"))?;
    lock.lock().map_err(|e| format!("{lock_path}: {e}"))?;
    Ok(lock)
}

/// s-<time>, with a suffix while the name is in use or the repo at home has a workspace of it.
fn fresh_name(home: &str, in_use: impl Fn(&str) -> bool) -> String {
    let base = Local::now().format("s-%m%d-%H%M%S").to_string();
    let mut name = base.clone();
    let mut i = 2;
    while in_use(&name) || Path::new(&format!("{home}.agents/{name}")).exists() {
        name = format!("{base}-{i}");
        i += 1;
    }
    name
}

/// The words of a claude command line: the first mate also gets its role prompt; bypass mode unless ask_permissions.
fn claude_argv(mate: bool, ask_permissions: bool, args: &[&str]) -> Vec<String> {
    let mut argv = vec!["claude".to_string()];
    if mate {
        argv.extend([
            "--append-system-prompt-file".to_string(),
            format!("{}/mate.md", claude_dir()),
        ]);
    }
    if !ask_permissions {
        argv.extend([
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
        ]);
    }
    argv.extend(args.iter().map(|a| a.to_string()));
    argv
}

/// Runs claude with args in a new shpool session of the repo at home, with a record titled after text, and attaches to
/// it unless detach. make_dir(name) returns the directory to run in; it is called under the lock, once the name is
/// chosen. The name is the session's identity: AGENT_SESSION, the record name and the shpool session name are equal,
/// and jw finds a live Claude's shpool session by its AGENT_SESSION. reports_to is the session the new record names as
/// its supervisor. A first mate (mate) has its role prompt, the record kind MATE_KIND and no shpool ttl. Claude runs in
/// bypass mode unless ask_permissions, which the record keeps.
#[allow(clippy::too_many_arguments)]
fn launch(
    home: &str,
    detach: bool,
    args: &[&str],
    text: &str,
    reports_to: Option<&str>,
    make_dir: impl FnOnce(&str) -> Result<String, String>,
    naming: Naming,
    mate: bool,
    ask_permissions: bool,
) -> Result<(), String> {
    let rundir = runtime_dir();
    let name = {
        let _lock = naming_lock(&rundir)?;
        let taken = Shpool.list()?;
        let records = Records::new();
        let name = match naming {
            Naming::Resume(name) if valid_name(name) && !taken.contains_key(name) => {
                name.to_string()
            }
            _ => fresh_name(home, |name| {
                taken.contains_key(name) || records.read(name).is_some()
            }),
        };
        let dir = make_dir(&name)?;
        let mut fields = match naming {
            Naming::Resume(_) if records.read(&name).is_some() => Record {
                workspace: Some(dir.clone()),
                ..Record::default()
            },
            _ => task_record(text, &dir, reports_to),
        };
        if mate {
            fields.kind = Some(MATE_KIND.to_string());
        }
        // a failure to write the record must not stop the session
        let recorded = records.update(&name, |r| {
            r.merge(fields);
            // a (re)started session is not finished
            r.clear_marks();
            r.ask_permissions = ask_permissions;
            true
        });
        if let Err(e) = recorded {
            eprintln!("jw: {}", write_error(&e));
        }
        let job = write_job(
            &rundir,
            &name,
            &dir,
            &claude_argv(mate, ask_permissions, args),
        )?;
        let started = Shpool.start(&name, &dir, &job, (!mate).then_some("1d"));
        if !matches!(started, Ok(true)) {
            // the script removes the file when it starts; the empty workspace is left for `jw prune`
            let _ = fs::remove_file(&job);
            return Err(match started {
                Err(e) => e,
                Ok(_) => format!("shpool attach failed for session {name}"),
            });
        }
        name
    };
    if detach {
        eprintln!("jw: started {name} in {}", basename(home));
        eprintln!("jw attach {name}");
        return Ok(());
    }
    // if claude has exited, attach would silently create a plain shell session under this name; checking narrows the
    // race, it doesn't close it
    // the daemon can still count the exited `attach -b` client as attached for a moment, and refuses a second one then
    let deadline = Instant::now() + Duration::from_secs(5);
    let state = loop {
        let state = Shpool.list()?.get(&name).cloned();
        if state.as_deref() == Some("attached") && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        } else {
            break state;
        }
    };
    if state.is_none() {
        return Err(format!("session {name} ended before attaching"));
    }
    Err(Shpool.attach(&name))
}

/// Writes the 0600 script that sets up the session's environment and runs argv; the environment and the prompt go only
/// into this file: shpool does not pass on the client's environment, and its -c is split like shell words.
fn write_job(rundir: &str, name: &str, dir: &str, argv: &[String]) -> Result<String, String> {
    let mut env = agent_env();
    env.insert("AGENT_SESSION".to_string(), name.to_string());
    // started from a service (task.new) there is no terminal env, and claude falls back to no colors
    if env.get("TERM").map_or("dumb", String::as_str) == "dumb" {
        env.insert("TERM".to_string(), "xterm-256color".to_string());
    }
    env.entry("COLORTERM".to_string())
        .or_insert_with(|| "truecolor".to_string());
    env.insert("SHPOOL_SESSION_NAME".to_string(), name.to_string());
    let is_var = |k: &str| {
        k.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !["UID", "EUID", "PPID", "SHELLOPTS", "BASHOPTS"].contains(&k)
    };
    let mut lines: Vec<String> = env
        .iter()
        .filter(|(k, _)| is_var(k.as_str()))
        .map(|(k, v)| format!("export {k}={}", sh_quote(v)))
        .collect();
    // after the exports: PATH is set now
    lines.push(r#"rm -f -- "$0""#.to_string());
    lines.push(format!("cd -- {} || exit 1", sh_quote(dir)));
    let words: Vec<String> = argv.iter().map(|w| sh_quote(w)).collect();
    // also unloads an inherited direnv env
    lines.push(format!("exec direnv exec . {}", words.join(" ")));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = format!("{rundir}/jw-agent-{}-{nanos}", process::id());
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut f| f.write_all(format!("{}\n", lines.join("\n")).as_bytes()))
        .map_err(|e| format!("{path}: {e}"))?;
    Ok(path)
}

/// The directory name under projects/ that the claude wrapper (__claude_project) gives the repo at home.
fn project_key(home: &str) -> Result<String, String> {
    let key: String = home
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if key.len() <= 64 {
        return Ok(key);
    }
    let out =
        output_with_input(&mut Command::new("cksum"), home).map_err(|e| format!("cksum: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let crc = stdout.split_whitespace().next().ok_or("cksum failed")?;
    Ok(format!("{}-{crc}", &key[key.len() - 50..]))
}

/// The text of a transcript's user entry (whitespace collapsed), empty for tool results.
fn user_text(entry: &Map<String, Value>) -> String {
    let text = match entry.get("message").and_then(|m| m.get("content")) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text")?.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => return String::new(),
    };
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The root of the workspace of the repo at home that directory cwd is in, by string so that deleted directories work;
/// None outside the repo.
fn workspace_root(cwd: &str, home: &str) -> Option<String> {
    let agents = format!("{home}.agents/");
    if let Some(rest) = cwd.strip_prefix(&agents) {
        let name = rest.split('/').next().unwrap_or("");
        return (!name.is_empty()).then(|| format!("{agents}{name}"));
    }
    (cwd == home || cwd.starts_with(&format!("{home}/"))).then(|| home.to_string())
}

/// (root of the last workspace of the repo at home it worked in, title) of the transcript, None if it has no user
/// message or workspace.
fn transcript(path: &str, home: &str) -> Option<(String, String)> {
    let data = fs::read(path).ok()?;
    let (mut root, mut ai, mut custom) = (None, None, None);
    let mut first = String::new();
    for line in String::from_utf8_lossy(&data).lines() {
        let Ok(Value::Object(entry)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        if let Some(cwd) = entry.get("cwd").and_then(Value::as_str) {
            root = workspace_root(cwd, home).or(root);
        }
        let meta = entry.get("isMeta").and_then(Value::as_bool) == Some(true);
        match entry.get("type").and_then(Value::as_str) {
            Some("ai-title") => ai = text("aiTitle").or(ai),
            Some("custom-title") => custom = text("customTitle").or(custom),
            Some("user") if first.is_empty() && !meta => first = user_text(&entry),
            _ => {}
        }
    }
    if first.is_empty() {
        return None;
    }
    let title = custom.or(ai).unwrap_or(first);
    let title: String = title.chars().take(100).collect();
    Some((
        root?,
        title.split_whitespace().collect::<Vec<_>>().join(" "),
    ))
}

struct Row {
    label: String,
    id: String,
    root: String,
    title: String,
}

/// The repo's transcripts newest first, without the live ones if skip_live; only those whose last workspace is
/// workspace if given, only the newest one if newest_only.
fn resume_rows(
    home: &str,
    newest_only: bool,
    workspace: Option<&str>,
    skip_live: bool,
) -> Result<Vec<Row>, String> {
    let base = format!("{}/projects/{}", claude_dir(), project_key(home)?);
    let mut files: Vec<(SystemTime, String)> = fs::read_dir(&base)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e
                .file_name()
                .into_string()
                .ok()?
                .strip_suffix(".jsonl")?
                .to_string();
            let mtime = fs::metadata(e.path()).ok()?.modified().ok()?;
            Some((mtime, id))
        })
        .collect();
    files.sort_by(|a, b| b.cmp(a));
    let live: HashSet<String> = if skip_live {
        live_claudes()
            .into_iter()
            .filter_map(|c| c.session_id)
            .collect()
    } else {
        HashSet::new()
    };
    let mut rows = vec![];
    for (mtime, id) in files {
        if live.contains(&id) {
            continue;
        }
        let Some((root, title)) = transcript(&format!("{base}/{id}.jsonl"), home) else {
            continue;
        };
        if workspace.is_some_and(|w| w != root) {
            continue;
        }
        let when = DateTime::<Local>::from(mtime).format("%m-%d %H:%M");
        let short: String = id.chars().take(8).collect();
        rows.push(Row {
            label: format!("{when}\t{}\t{title}\t{short}", basename(&root)),
            id,
            root,
            title,
        });
        if newest_only {
            break;
        }
    }
    Ok(rows)
}

/// The id of the newest Claude transcript of the repo home whose last workspace is workspace.
pub(crate) fn latest_transcript(home: &str, workspace: &str) -> Result<Option<String>, String> {
    Ok(resume_rows(home, true, Some(workspace), false)?
        .into_iter()
        .next()
        .map(|row| row.id))
}

/// (home, workspace) of the record of session name; the repo comes from the workspace path if it is gone.
pub(crate) fn session_repo(name: &str) -> Result<(String, String), String> {
    let record = Records::new()
        .read(name)
        .ok_or_else(|| format!("no session record {name}"))?;
    let ws = record.workspace.as_deref();
    if let Some((home, _, path)) = agent_workspace(ws) {
        return Ok((home, path));
    }
    if let Some(ws) = ws.filter(|w| !w.contains('\0') && Path::new(w).is_absolute()) {
        let parent = dirname(ws);
        let agents = basename(parent);
        let repo = agents.strip_suffix(".agents").unwrap_or(agents);
        let wname = basename(ws);
        if realpath(dirname(parent)) == realpath(root())
            && agents.ends_with(".agents")
            && !repo.is_empty()
            && valid_name(wname)
            && is_dir(&format!("{}/{repo}/.jj", root()))
        {
            return Ok((
                format!("{}/{repo}", root()),
                format!("{}/{agents}/{wname}", root()),
            ));
        }
    }
    Err(format!("no repo for session {name}"))
}

fn not_headless(name: &str) -> Result<(), String> {
    if Records::new().read(name).is_some_and(|r| r.headless) {
        return Err(format!("{name} is a headless worker; use jw msg"));
    }
    Ok(())
}

pub fn cmd_resume(a: ResumeArgs) -> Result<(), String> {
    let session = a.session.filter(|s| !s.is_empty());
    let repo = a.repo.filter(|r| !r.is_empty());
    if session.is_some() && repo.is_some() {
        return Err("--session takes no repo".to_string());
    }
    let (home, row, name) = if let Some(session) = session {
        let name = session_name(Some(&session))?;
        not_headless(&name)?;
        let (home, workspace) = session_repo(&name)?;
        if Shpool.list()?.contains_key(&name) {
            return Err(format!("session {name} is running"));
        }
        let Some(row) = resume_rows(&home, true, Some(&workspace), false)?
            .into_iter()
            .next()
        else {
            return Err(format!("no sessions to resume for {name}"));
        };
        if live_claudes()
            .iter()
            .any(|c| c.session_id.as_deref() == Some(row.id.as_str()))
        {
            return Err(format!("session {name} is running"));
        }
        (home, row, name)
    } else {
        let home = match &repo {
            None => locate().ok().map(|(_, home)| home),
            Some(_) => None,
        };
        let home = match home {
            Some(home) => home,
            None => repo_home(repo.as_deref())?,
        };
        let rows = resume_rows(&home, false, None, true)?;
        if rows.is_empty() {
            return Err(format!("no sessions to resume in {}", basename(&home)));
        }
        let labels: Vec<String> = rows.iter().map(|r| r.label.clone()).collect();
        let label = pick(&labels, "", None)?;
        let row = rows
            .into_iter()
            .find(|r| r.label == label)
            .ok_or("no such session")?;
        let name = basename(&row.root).to_string();
        not_headless(&name)?;
        (home, row, name)
    };
    let Row {
        id, root, title, ..
    } = row;
    let make_dir = |name: &str| -> Result<String, String> {
        if is_dir(&root) {
            return Ok(root.clone());
        }
        fetch(&home);
        let dir = add_workspace(&home, "trunk()", name, None)?;
        eprintln!("jw: {root} is gone, recreated it as {dir} on trunk");
        Ok(dir)
    };
    let record = Records::new().read(&name);
    let mate = record
        .as_ref()
        .is_some_and(|r| r.kind.as_deref() == Some(MATE_KIND));
    let ask_permissions = record.is_some_and(|r| r.ask_permissions);
    launch(
        &home,
        a.detach,
        &["--resume", id.as_str()],
        &title,
        None,
        make_dir,
        Naming::Resume(&name),
        mate,
        ask_permissions,
    )
}

pub fn cmd_mate() -> Result<(), String> {
    let home = format!("{}/mate", root());
    let jj = Jj::in_dir(&home);
    if !is_dir(&format!("{home}/.jj")) {
        fs::create_dir_all(&home).map_err(|e| format!("{home}: {e}"))?;
        jj.git_init()?;
    }
    jj.ensure_main()?;
    start_agent(&home, false, None, &[], None, true, false)
}

/// The session called prefix, else the only one starting with it.
fn match_session(names: &[&String], prefix: &str) -> Result<String, String> {
    if names.iter().any(|n| *n == prefix) {
        return Ok(prefix.to_string());
    }
    let mut matches: Vec<&String> = names
        .iter()
        .copied()
        .filter(|n| n.starts_with(prefix))
        .collect();
    matches.sort();
    match matches.as_slice() {
        [] => Err(format!("no Claude shpool session {prefix}")),
        [only] => Ok(only.to_string()),
        _ => Err(format!(
            "{prefix} is ambiguous: {}",
            matches
                .iter()
                .map(|n| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// fzf lines name, workspace (repo/name, else the directory), shpool state of the sessions in found ({session:
/// directory of its Claude}) that shpool has (state); those of the repo `current` first.
fn session_rows(
    found: &BTreeMap<String, String>,
    state: &HashMap<String, String>,
    current: Option<&str>,
) -> Vec<String> {
    let mut rows = vec![];
    for (name, cwd) in found {
        let Some(shpool_state) = state.get(name) else {
            continue;
        };
        let mut d = cwd.as_str();
        while d != "/" && !d.is_empty() && !is_dir(&format!("{d}/.jj")) {
            d = dirname(d);
        }
        let (repo, where_) = if is_dir(&format!("{d}/.jj")) {
            let (repo, workspace, _) = layout(d);
            let where_ = format!("{repo}/{workspace}");
            (Some(repo), where_)
        } else {
            (None, cwd.clone())
        };
        let other = current.is_none() || repo.as_deref() != current;
        rows.push((other, format!("{name}\t{where_}\t{shpool_state}")));
    }
    rows.sort_by_key(|(other, _)| *other);
    rows.into_iter().map(|(_, line)| line).collect()
}

pub fn cmd_attach(a: AttachArgs) -> Result<(), String> {
    let found: BTreeMap<String, String> = live_claudes()
        .into_iter()
        .filter_map(|c| Some((c.agent?, c.cwd)))
        .collect();
    let state = Shpool.list()?;
    let name = a.name.filter(|n| !n.is_empty());
    if let Some(name) = &name {
        // also right after `jw agent -d`, before claude has registered its session
        if state.contains_key(name) {
            return Err(Shpool.attach(name));
        }
        let names: Vec<&String> = found.keys().filter(|n| state.contains_key(*n)).collect();
        return Err(Shpool.attach(&match_session(&names, name)?));
    }
    let current = locate().ok().map(|(root, _)| layout(&root).0);
    let rows = session_rows(&found, &state, current.as_deref());
    let picked = pick(&rows, "", Some("no Claude shpool sessions"))?;
    Err(Shpool.attach(picked.split('\t').next().unwrap_or("")))
}
