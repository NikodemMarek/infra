use std::env;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::jj::Jj;
use crate::procs::claude_dir;
use crate::record::{now, valid_name};
use crate::session::track_worktree;
use crate::workspace::{dirname, is_dir, layout, realpath};

const LOCK_WAIT: Duration = Duration::from_secs(120);
const NAME_MAX: usize = 100;

/// A workspace the hook created; the file format is shared with the Python hook.
#[derive(Serialize, Deserialize)]
struct Entry {
    name: String,
    root: String,
    dir: String,
    #[serde(default)]
    agents: String,
    base: String,
    kind: String,
    #[serde(default)]
    touched: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ws: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    warned: bool,
}

/// The entries of one repo, claude_dir()/jj-agents/<first 16 hex of the SHA-256 of the store path>/<name>.json: the same
/// from every workspace of the repo.
struct Registry {
    dir: PathBuf,
}

impl Registry {
    fn new(root: &str) -> Registry {
        let store = Path::new(root).join(".jj/repo");
        // in a secondary workspace .jj/repo is a file pointing to the store
        let store = match fs::read_to_string(&store) {
            Ok(target) => store.with_file_name(target.trim()),
            Err(_) => store,
        };
        let hash = format!(
            "{:x}",
            Sha256::digest(realpath(store).as_os_str().as_bytes())
        );
        Registry {
            dir: Path::new(&claude_dir()).join("jj-agents").join(&hash[..16]),
        }
    }

    /// The registry and entry of an isolated agent, looked up in every repo's registry.
    fn find_agent(agent_id: &str) -> Option<(Registry, Entry)> {
        if !is_hex(agent_id) {
            return None;
        }
        let name = format!("agent-{agent_id}");
        fs::read_dir(Path::new(&claude_dir()).join("jj-agents"))
            .ok()?
            .flatten()
            .find_map(|d| {
                let reg = Registry { dir: d.path() };
                let entry = reg.load(&name)?;
                Some((reg, entry))
            })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    /// The entry of name, None if it is missing or incomplete.
    fn load(&self, name: &str) -> Option<Entry> {
        let mut entry: Entry = serde_json::from_slice(&fs::read(self.path(name)).ok()?).ok()?;
        if entry.agents.is_empty() {
            entry.agents = dirname(&entry.dir).to_string();
        }
        Some(entry)
    }

    fn save(&self, entry: &Entry) -> io::Result<()> {
        let tmp = self.dir.join(format!("{}.json.tmp", entry.name));
        fs::write(&tmp, serde_json::to_string_pretty(entry)?)?;
        fs::rename(tmp, self.path(&entry.name))
    }

    fn remove(&self, name: &str) {
        let _ = fs::remove_file(self.path(name));
    }

    /// Waits for the exclusive lock on the registry; it is held until the file is dropped.
    fn lock(&self) -> Result<File, String> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join(".lock"))
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err("timed out waiting for the registry lock".to_string());
                    }
                    thread::sleep(Duration::from_millis(200));
                }
                Err(TryLockError::Error(e)) => return Err(e.to_string()),
            }
        }
    }
}

/// Starts the project MCP servers of a fresh worktree disabled: avoids the blocking approval dialog.
fn mcp_defaults(dir: &str) {
    if let Err(e) = Command::new("claude-mcp-defaults")
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
    {
        eprintln!("jw: claude-mcp-defaults: {e}");
    }
}

/// Runs git in cwd; its stdout goes to stderr, so that stdout carries only the directory.
fn git(cwd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(["-C", cwd])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`git {}`: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    eprint!("{stdout}");
    Ok(stdout)
}

fn create_git(cwd: &str, name: &str) -> Result<String, String> {
    let top = git(cwd, &["rev-parse", "--show-toplevel"])?
        .trim()
        .to_string();
    let wt = format!("{top}/.claude/worktrees/{name}");
    if is_dir(&wt) {
        return Ok(wt);
    }
    let branch = format!("worktree-{name}");
    fs::create_dir_all(dirname(&wt)).map_err(|e| e.to_string())?;
    let branch_exists = Command::new("git")
        .args(["-C", top.as_str(), "show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if branch_exists {
        git(&top, &["worktree", "add", &wt, &branch])?;
    } else {
        git(&top, &["worktree", "add", "-b", &branch, &wt, "HEAD"])?;
    }
    mcp_defaults(&wt);
    Ok(wt)
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn forget(root: &str, ws: &str) {
    let _ = Jj::repo(root).query(&["--ignore-working-copy", "workspace", "forget", ws]);
}

/// The id of the change to start a workspace from. A session does not build on the caller's @, which may be another
/// session's work: it starts on $CLAUDE_WORKSPACE_BASE (the claude wrapper sets it in a w-* workspace) or trunk().
fn base_change(jj: &Jj, session: bool) -> Result<String, String> {
    if session {
        let want = env::var("CLAUDE_WORKSPACE_BASE").unwrap_or_default();
        let one_change = !want.is_empty()
            && want.bytes().all(|b| b.is_ascii_lowercase())
            && jj
                .query(&[
                    "--ignore-working-copy",
                    "log",
                    "--no-graph",
                    "-r",
                    &format!("change_id({want})"),
                    "-T",
                    r#"change_id ++ "\n""#,
                ])
                .is_ok_and(|ids| ids.split_whitespace().count() == 1);
        if one_change {
            return Ok(want);
        }
        let trunk = jj
            .query(&[
                "--ignore-working-copy",
                "log",
                "--no-graph",
                "-r",
                "trunk()",
                "-T",
                r#"if(root, "", change_id)"#,
            ])
            .unwrap_or_default();
        if !trunk.trim().is_empty() {
            return Ok(trunk.trim().to_string());
        }
    }
    // snapshots the caller
    let id = jj.query(&["log", "--no-graph", "-r", "@", "-T", "change_id"])?;
    Ok(id.trim().to_string())
}

/// Under the registry lock: the directory of the workspace for name, and whether it was created now.
fn create_locked(reg: &Registry, root: &str, name: &str) -> Result<(String, bool), String> {
    let jj = Jj::repo(root);
    let agent_hex = name.strip_prefix("agent-").filter(|h| is_hex(h));
    let (wsname, kind) = match agent_hex {
        Some(hex) => (format!("a-{hex}"), "agent"),
        None => (name.to_string(), "session"),
    };
    if let Some(mut entry) = reg.load(name) {
        if is_dir(&entry.dir) {
            entry.touched = now();
            reg.save(&entry).map_err(|e| e.to_string())?;
            return Ok((entry.dir, false));
        }
        forget(root, entry.ws.as_deref().unwrap_or(name));
        // jj keeps a git worktree registered for the forgotten workspace, which would make `workspace add` fail
        let _ = git(root, &["worktree", "prune"]);
        reg.remove(name);
    } else if jj
        .query(&[
            "--ignore-working-copy",
            "workspace",
            "list",
            "-T",
            r#"name ++ "\n""#,
        ])?
        .split_whitespace()
        .any(|n| n == wsname)
    {
        return Err(format!(
            "a jj workspace named '{wsname}' already exists and isn't managed by this hook"
        ));
    }
    let agents = format!("{}.agents", layout(root).2);
    let dir = format!("{agents}/{wsname}");
    if fs::symlink_metadata(&dir).is_ok() {
        return Err(format!("{dir} already exists"));
    }
    let base = base_change(&jj, kind == "session")?;
    fs::create_dir_all(&agents).map_err(|e| e.to_string())?;
    jj.query(&[
        "workspace",
        "add",
        &format!("--name={wsname}"),
        "-r",
        &format!("change_id({base})"),
        &dir,
    ])?;
    mcp_defaults(&dir);
    let entry = Entry {
        name: name.to_string(),
        root: root.to_string(),
        dir: dir.clone(),
        agents,
        base,
        kind: kind.to_string(),
        touched: now(),
        ws: (wsname != name).then_some(wsname.clone()),
        warned: false,
    };
    if let Err(e) = reg.save(&entry) {
        forget(root, &wsname);
        let _ = fs::remove_dir_all(&dir);
        return Err(e.to_string());
    }
    Ok((dir, true))
}

/// The directory of the isolated worktree for name, created if need be (WorktreeCreate hook). In a jj repo it is a
/// workspace whose @ is a new change on the base; elsewhere a git worktree.
pub fn create(name: &str, cwd: &str) -> Result<String, String> {
    if !valid_name(name) || name.len() > NAME_MAX {
        return Err(format!("invalid worktree name '{name}'"));
    }
    let Ok(root) = Jj::in_dir(cwd).workspace_root() else {
        return create_git(cwd, name);
    };
    let reg = Registry::new(&root);
    fs::create_dir_all(&reg.dir).map_err(|e| e.to_string())?;
    let (dir, created) = {
        let _lock = reg.lock()?;
        create_locked(&reg, &root, name)?
    };
    if created && env::var("AGENT_SESSION").is_ok_and(|s| !s.is_empty()) {
        if let Err(e) = track_worktree(&dir) {
            eprintln!("jw: session worktree {dir}: {e}");
        }
    }
    Ok(dir)
}

enum Fail {
    /// The integration left conflicts: the text is the whole report.
    Conflict(String),
    Other(String),
}

impl From<String> for Fail {
    fn from(e: String) -> Fail {
        Fail::Other(e)
    }
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Updates a stale workspace and snapshots it without losing unsaved edits. A workspace goes stale when a commit under
/// its @ is rewritten; update-stale then snapshots unsaved edits into a divergent twin of @ and checks out the rebased
/// clean @, so the twin is folded back into @.
fn refresh(ws: &str) -> Result<(), String> {
    let jj = Jj::repo(ws);
    if let Err(e) = jj.query(&["workspace", "update-stale"]) {
        if !e.contains("not stale") {
            return Err(e);
        }
    }
    let cid = jj.query(&["log", "--no-graph", "-r", "@", "-T", "change_id"])?;
    let twins = format!("change_id({}) ~ @", cid.trim());
    let ids = jj.query(&[
        "log",
        "--no-graph",
        "-r",
        &twins,
        "-T",
        r#"commit_id.short() ++ " ""#,
    ])?;
    let ids: Vec<&str> = ids.split_whitespace().collect();
    if !ids.is_empty() {
        jj.query(&["squash", "--from", &twins, "--into", "@", "-u"])?;
        eprintln!(
            "jw: folded unsaved edits ({}) back into @ of {ws}",
            ids.join(", ")
        );
    }
    jj.query(&["status"])?;
    Ok(())
}

fn check_base(jj: &Jj, base: &str) -> Result<(), String> {
    let states = jj
        .query(&[
            "log",
            "--no-graph",
            "-r",
            &format!("change_id({base})"),
            "-T",
            r#"if(immutable, "immutable", "ok") ++ "\n""#,
        ])
        .unwrap_or_default();
    let states: Vec<&str> = states.split_whitespace().collect();
    let b8 = short(base);
    match states[..] {
        [] => return Err(format!("base change {b8} is gone")),
        ["immutable"] => {
            return Err(format!("base change {b8} is immutable now"));
        }
        [_] => {}
        _ => return Err(format!("base change {b8} is divergent")),
    }
    let stray = jj.query(&[
        "log",
        "--no-graph",
        "-r",
        &format!("(change_id({base})..@) ~ change_id({base})::"),
        "-T",
        r#"change_id.short(8) ++ " ""#,
    ])?;
    let stray: Vec<&str> = stray.split_whitespace().collect();
    if !stray.is_empty() {
        return Err(format!(
            "the agent's @ is no longer on top of base {b8} (also contains {})",
            stray.join(", ")
        ));
    }
    Ok(())
}

/// The number of files that differ between the base and the @ of the workspace.
fn changed_files(jj: &Jj, base: &str) -> Result<usize, String> {
    let out = jj.query(&[
        "diff",
        "--from",
        &format!("change_id({base})"),
        "--to",
        "@",
        "--name-only",
    ])?;
    Ok(out.lines().filter(|l| !l.is_empty()).count())
}

/// Squashes everything the agent did (base..@) into its base. Returns a summary, None if there was nothing to do.
fn integrate(entry: &Entry) -> Result<Option<String>, Fail> {
    let base = &entry.base;
    refresh(&entry.root)?;
    refresh(&entry.dir)?;
    let agent = Jj::repo(&entry.dir);
    check_base(&agent, base)?;
    let files = changed_files(&agent, base)?;
    if files == 0 {
        return Ok(None);
    }
    agent.query(&[
        "squash",
        "--from",
        &format!("change_id({base})..@"),
        "--into",
        &format!("change_id({base})"),
        "-u",
    ])?;
    // the base may be the caller's @
    refresh(&entry.root)?;
    let caller = Jj::repo(&entry.root);
    let info = caller.query(&[
        "log",
        "--no-graph",
        "-r",
        &format!("change_id({base})"),
        "-T",
        r#"change_id.short(8) ++ "\x1f" ++ description.first_line() ++ "\n""#,
    ])?;
    let [info] = info.lines().collect::<Vec<_>>()[..] else {
        return Err(format!(
            "base change {} became divergent while squashing (a concurrent jj command?); check `jj log -r 'change_id({base})'`",
            short(base)
        )
        .into());
    };
    let (cid, title) = info.split_once('\x1f').unwrap_or((info, ""));
    let title = if title.is_empty() {
        "(no description)"
    } else {
        title
    };
    let msg = format!(
        "jj: squashed {files} file(s) from agent workspace {} into {cid} {title}",
        entry.ws.as_deref().unwrap_or(&entry.name)
    );
    let conflicts = caller.query(&[
        "log",
        "--no-graph",
        "-r",
        &format!("change_id({base}):: & conflicts() ~ empty()"),
        "-T",
        r#"change_id.short(8) ++ " ""#,
    ])?;
    let conflicts: Vec<&str> = conflicts.split_whitespace().collect();
    if !conflicts.is_empty() {
        return Err(Fail::Conflict(format!(
            "{msg}, which left CONFLICTS in {} (jj resolve --list -r <change>)",
            conflicts.join(", ")
        )));
    }
    Ok(Some(msg))
}

fn stop_locked(reg: &Registry, name: &str) -> Result<Option<String>, Fail> {
    let _lock = reg.lock()?;
    let Some(mut entry) = reg.load(name) else {
        return Ok(None);
    };
    if !is_dir(&entry.dir) {
        return Err(format!("workspace {} no longer exists", entry.dir).into());
    }
    let msg = integrate(&entry)?;
    entry.touched = now();
    reg.save(&entry).map_err(|e| e.to_string())?;
    Ok(msg)
}

/// Squashes the work of the stopping agent into its base (SubagentStop hook); the workspace stays, so the agent can be
/// resumed. The notes to report; Err is the problem to report, the workspace is kept.
pub fn stop(agent_id: &str, cwd: &str) -> Result<Vec<String>, String> {
    let Some((reg, entry)) = Registry::find_agent(agent_id) else {
        return Ok(vec![]);
    };
    if !realpath(cwd).starts_with(realpath(&entry.dir)) {
        return Ok(vec![]);
    }
    match stop_locked(&reg, &entry.name) {
        Ok(msg) => Ok(msg.into_iter().collect()),
        Err(Fail::Conflict(e)) => Err(format!("{e}.")),
        Err(Fail::Other(e)) => Err(format!(
            "jj integration problem: {e}. The agent's work is kept in {}.",
            entry.dir
        )),
    }
}
