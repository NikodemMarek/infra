use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::cmd::output_with_input;
use crate::jj::Jj;
use crate::procs::home;
use crate::record::valid_name;
use crate::{CdArgs, CloneArgs, NewArgs, RmArgs};

static ROOT: OnceLock<String> = OnceLock::new();

pub fn init_root() -> Result<(), String> {
    let root = env::var("JW_PROJECTS_ROOT")
        .ok()
        .filter(|root| !root.is_empty())
        .ok_or("JW_PROJECTS_ROOT is not set")?;
    let root = match root.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home()),
        None => root,
    };
    let _ = ROOT.set(root);
    Ok(())
}

pub fn root() -> &'static str {
    ROOT.get().map_or("", String::as_str)
}

pub fn basename(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
}

pub fn dirname(path: &str) -> &str {
    Path::new(path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
}

/// The canonical path; for a path that does not exist, its canonical parent with the name appended, else path itself.
pub fn realpath(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if let Ok(real) = fs::canonicalize(path) {
        return real;
    }
    match (path.parent().map(fs::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

pub fn is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

pub fn has_agents_suffix(path: &str) -> bool {
    path.len() > ".agents".len() && path.ends_with(".agents")
}

/// The workspaces gitlab-context creates: it reuses and cleans them.
pub fn is_gitlab(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("gl-") else {
        return false;
    };
    let letters = |s: &&str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase());
    let digits = |s: &&str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match rest.split('-').collect::<Vec<_>>().as_slice() {
        [kind, id] => letters(kind) && digits(id),
        [kind, id, n] => letters(kind) && digits(id) && digits(n),
        _ => false,
    }
}

/// (home, name, path) of the agent workspace a record points to, if there is one.
pub fn agent_workspace(ws: Option<&str>) -> Option<(String, String, String)> {
    let ws = ws.filter(|w| !w.contains('\0') && Path::new(w).is_absolute())?;
    let real = realpath(ws);
    let parent = real.parent()?;
    let name = real.file_name()?.to_str()?;
    let agents = parent.file_name()?.to_str()?;
    let repo = agents.strip_suffix(".agents")?;
    if parent.parent()? != realpath(root())
        || !has_agents_suffix(agents)
        || !is_dir(&format!("{}/{repo}/.jj", root()))
        || !valid_name(name)
        || !is_dir(&format!("{}/{agents}/{name}/.jj", root()))
    {
        return None;
    }
    Some((
        format!("{}/{repo}", root()),
        name.to_string(),
        format!("{}/{agents}/{name}", root()),
    ))
}

/// (repo, workspace name, home = root of the main workspace) of the workspace at root.
pub fn layout(root: &str) -> (String, String, String) {
    let parent = dirname(root);
    if has_agents_suffix(parent) {
        let home = parent.strip_suffix(".agents").unwrap_or(parent);
        return (
            basename(home).to_string(),
            basename(root).to_string(),
            home.to_string(),
        );
    }
    (
        basename(root).to_string(),
        "default".to_string(),
        root.to_string(),
    )
}

/// (root of the current workspace, home = root of the main workspace).
pub fn locate() -> Result<(String, String), String> {
    let root = Jj::here().workspace_root()?;
    let home = layout(&root).2;
    Ok((root, home))
}

/// Sorted names of the jj repos directly in the projects root.
pub fn repos() -> Vec<String> {
    let Ok(entries) = fs::read_dir(root()) else {
        return vec![];
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            !n.starts_with('.') && !n.ends_with(".agents") && is_dir(&format!("{}/{n}/.jj", root()))
        })
        .collect();
    names.sort();
    names
}

/// Labels (repo/name) of the workspaces in the <repo>.agents dirs, those of the repo first before the others.
pub fn workspaces(first: Option<&str>) -> Vec<String> {
    let mut items = vec![];
    for repo in repos() {
        let agents = format!("{}/{repo}.agents", root());
        let Ok(entries) = fs::read_dir(&agents) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| is_dir(&format!("{agents}/{n}/.jj")))
            .collect();
        names.sort();
        items.extend(names.into_iter().map(|n| format!("{repo}/{n}")));
    }
    items.sort_by_key(|i| i.split('/').next() != first);
    items
}

extern "C" fn ignore_sigint(_: libc::c_int) {}

/// Lets the user choose with fzf (it draws on /dev/tty, so the fish function can capture stdout).
pub fn pick(items: &[String], query: &str, empty: Option<&str>) -> Result<String, String> {
    if items.is_empty() {
        return Err(empty.map_or_else(|| format!("no jj repos under {}", root()), str::to_string));
    }
    // fzf gets the Ctrl-C too and exits 130; jw must live to report it
    unsafe {
        libc::signal(
            libc::SIGINT,
            ignore_sigint as *const () as libc::sighandler_t,
        )
    };
    let out = output_with_input(
        Command::new("fzf")
            .args(["--select-1", "--exit-0", "--query", query])
            .stderr(Stdio::inherit()),
        &items.join("\n"),
    );
    unsafe { libc::signal(libc::SIGINT, libc::SIG_DFL) };
    let out = out.map_err(|e| format!("fzf: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        Some(1) => Err("no match".to_string()),
        Some(130) => Err("cancelled".to_string()),
        code => Err(format!(
            "fzf failed (exit {})",
            code.unwrap_or_else(|| -out.status.signal().unwrap_or(0))
        )),
    }
}

/// The repo to act on: the named one, else a picked one (the current repo listed first).
pub fn repo_home(arg: Option<&str>) -> Result<String, String> {
    if let Some(arg) = arg.filter(|a| !a.is_empty()) {
        if is_dir(&format!("{}/{arg}/.jj", root())) {
            return Ok(format!("{}/{arg}", root()));
        }
    }
    let mut items = repos();
    if let Ok((_, home)) = locate() {
        let current = Path::new(&home)
            .strip_prefix(root())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(i) = items.iter().position(|r| *r == current) {
            let repo = items.remove(i);
            items.insert(0, repo);
        }
    }
    Ok(format!(
        "{}/{}",
        root(),
        pick(&items, arg.unwrap_or(""), None)?
    ))
}

pub fn fetch(home: &str) {
    let jj = Jj::in_dir(home);
    // a repo without a remote (the mate's) has nothing to fetch
    if jj.has_remotes() == Ok(false) {
        return;
    }
    if jj.fetch().is_err() {
        eprintln!("jw: fetch failed, continuing");
    }
}

/// Creates the workspace <home>.agents/<name> on revision and returns its directory. resolve_in is the workspace
/// directory to resolve revision in (its edits are snapshotted first), None for home without snapshotting.
pub fn add_workspace(
    home: &str,
    revision: &str,
    name: &str,
    resolve_in: Option<&str>,
) -> Result<String, String> {
    // resolve first: `workspace add` leaves its directory behind on a bad revision
    let commits = match resolve_in {
        Some(dir) => Jj::in_dir(dir).commits(revision, true)?,
        None => Jj::in_dir(home).commits(revision, false)?,
    };
    let [commit] = commits.as_slice() else {
        return Err(format!(
            "'{revision}' must be exactly one commit, not {}",
            commits.len()
        ));
    };
    if commit == "root" {
        return Err(format!("{revision} resolves to the root commit; pass -r"));
    }
    let dir = format!("{home}.agents/{name}");
    fs::create_dir_all(dirname(&dir)).map_err(|e| e.to_string())?;
    Jj::in_dir(home).workspace_add(name, commit, &dir)?;
    Ok(dir)
}

/// The current workspace's root if it belongs to the repo at home (a revision resolves there), else None.
pub fn resolve_dir(home: &str) -> Option<String> {
    let (root, current_home) = locate().ok()?;
    (realpath(&current_home) == realpath(home)).then_some(root)
}

pub fn cmd_new(a: NewArgs) -> Result<(), String> {
    let home = repo_home(a.repo.as_deref())?;
    fetch(&home);
    let revision = a.revision.filter(|r| !r.is_empty());
    // the default trunk() is resolved in home without a snapshot
    let resolve_in = revision.as_ref().and_then(|_| resolve_dir(&home));
    let name = chrono::Local::now().format("w-%m%d-%H%M%S").to_string();
    let dir = add_workspace(
        &home,
        revision.as_deref().unwrap_or("trunk()"),
        &name,
        resolve_in.as_deref(),
    )?;
    println!("{dir}");
    Ok(())
}

pub fn cmd_cd(a: CdArgs) -> Result<(), String> {
    let name = a.name.filter(|n| !n.is_empty());
    let mut current = None;
    if let Ok((root, home)) = locate() {
        if let Some(name) = &name {
            let dir = format!("{home}.agents/{name}");
            if is_dir(&dir) {
                println!("{dir}");
                return Ok(());
            }
        }
        current = Some(layout(&root).0);
    }
    let picked = pick(
        &workspaces(current.as_deref()),
        name.as_deref().unwrap_or(""),
        Some("no workspaces"),
    )?;
    let (repo, name) = picked.split_once('/').ok_or("bad workspace label")?;
    println!("{}/{repo}.agents/{name}", root());
    Ok(())
}

pub fn cmd_clone(a: CloneArgs) -> Result<(), String> {
    let name = match a.name.filter(|n| !n.is_empty()) {
        Some(name) => name,
        None => {
            let last = a
                .url
                .trim_end_matches('/')
                .rsplit(['/', ':'])
                .next()
                .unwrap_or("");
            last.strip_suffix(".git").unwrap_or(last).to_string()
        }
    };
    if !valid_name(&name) || name.ends_with(".agents") {
        return Err(format!("invalid name '{name}'"));
    }
    let dir = format!("{}/{name}", root());
    if fs::symlink_metadata(&dir).is_ok() {
        return Err(format!("{dir} already exists"));
    }
    fs::create_dir_all(root()).map_err(|e| e.to_string())?;
    Jj::here().git_clone(&a.url, &dir)?;
    println!("{dir}");
    Ok(())
}

fn walk(base: &Path, dir: &Path, found: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // jj may put a .git file in the workspaces of colocated repos
        if dir == base && name == ".git" {
            continue;
        }
        if file_type.is_dir() {
            if name != ".jj" {
                walk(base, &path, found);
            }
        } else if let Ok(rel) = path.strip_prefix(base) {
            // jj tracks a symlink as one file, also to a directory
            found.insert(rel.to_string_lossy().into_owned());
        }
    }
}

/// Files in d that jj does not track (ignored or too large): they die with the directory.
fn untracked(d: &str) -> Result<Vec<String>, String> {
    let mut on_disk = BTreeSet::new();
    walk(Path::new(d), Path::new(d), &mut on_disk);
    let tracked: HashSet<String> = Jj::in_dir(d).file_list()?.into_iter().collect();
    Ok(on_disk
        .into_iter()
        .filter(|f| !tracked.contains(f))
        .collect())
}

/// Asks on the terminal; an error unless the answer is y. With yes, answers y without asking.
pub fn confirm(question: &str, yes: bool) -> Result<(), String> {
    if yes {
        eprintln!("{question}y (--yes)");
        return Ok(());
    }
    eprint!("{question}");
    let _ = io::stderr().flush();
    // stdout is captured by the fish function
    let tty = fs::File::open("/dev/tty")
        .map_err(|_| "nothing removed (no terminal to confirm)".to_string())?;
    let mut answer = String::new();
    let _ = BufReader::new(tty).read_line(&mut answer);
    if answer.trim().to_lowercase() != "y" {
        return Err("nothing removed".to_string());
    }
    Ok(())
}

/// Removes <repo>.agents if it is empty now.
fn rmdir_agents(home: &str) {
    let _ = fs::remove_dir(format!("{home}.agents"));
}

/// paths, with each top-level directory that holds more than ten of them as one `dir/ (N files)`.
fn collapse(paths: &[String]) -> Vec<String> {
    fn top(path: &str) -> &str {
        path.split('/').next().unwrap_or(path)
    }
    let mut count: HashMap<&str, usize> = HashMap::new();
    for p in paths {
        *count.entry(top(p)).or_default() += 1;
    }
    let mut shown: Vec<String> = vec![];
    for p in paths {
        let t = top(p);
        let item = if count[t] > 10 && t != p {
            format!("{t}/ ({} files)", count[t])
        } else {
            p.clone()
        };
        if !shown.contains(&item) {
            shown.push(item);
        }
    }
    shown
}

/// Snapshots the workspace at d (unless snapshot is false) and, if files jj does not track would be lost with it
/// and are not all in asked, asks whether to delete. Returns the files that would be lost.
pub fn ask_lost(
    d: &str,
    yes: bool,
    snapshot: bool,
    asked: &[String],
) -> Result<Vec<String>, String> {
    if snapshot {
        // its work stays in its change after the workspace is gone
        Jj::in_dir(d).status()?;
    }
    let lost = untracked(d)?;
    if lost.iter().any(|f| !asked.contains(f)) {
        let listed = if yes {
            collapse(&lost).join(", ")
        } else {
            let more = if lost.len() > 5 {
                format!(", ... (+{} more)", lost.len() - 5)
            } else {
                String::new()
            };
            format!("{}{more}", lost[..lost.len().min(5)].join(", "))
        };
        confirm(
            &format!(
                "jw: {} file(s) in {d} are not tracked by jj (ignored or too large) and will be lost: {listed}. Delete? [y/N] ",
                lost.len()
            ),
            yes,
        )?;
    }
    Ok(lost)
}

pub fn remove(home: &str, name: &str) -> Result<(), String> {
    let dir = format!("{home}.agents/{name}");
    // its own workspace: the jj name may differ from name
    Jj::in_dir(&dir).workspace_forget(None)?;
    fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    rmdir_agents(home);
    Ok(())
}

/// Deletes a workspace jj cannot read (the caller has asked: jj cannot tell what is in there).
pub fn remove_broken(home: &str, name: &str, known: &[String]) -> Result<(), String> {
    if known.iter().any(|k| k == name) {
        // its directory goes anyway
        let _ = Jj::in_dir(home).workspace_forget(Some(name));
    } else {
        // a renamed workspace: the jj name is unknown without reading the directory
        eprintln!("jw: the jj workspace for {name} may remain; see 'jj workspace list' / 'jj workspace forget <name>'");
    }
    fs::remove_dir_all(format!("{home}.agents/{name}")).map_err(|e| e.to_string())?;
    rmdir_agents(home);
    Ok(())
}

/// Whether d was modified within the last hour (a running agent may not have edited anything yet).
pub fn recent(d: &str) -> bool {
    [d.to_string(), format!("{d}/.jj/working_copy")]
        .iter()
        .filter_map(|p| fs::metadata(p).ok()?.modified().ok())
        .max()
        .is_some_and(|t| {
            SystemTime::now()
                .duration_since(t)
                .map_or(true, |age| age < Duration::from_secs(3600))
        })
}

pub fn cmd_rm(a: RmArgs) -> Result<(), String> {
    let (root, home) = locate()?;
    let name = match a.name.filter(|n| !n.is_empty()) {
        Some(name) => name,
        None if root != home => basename(&root).to_string(),
        None => return Err("give a workspace name (this is the main one)".to_string()),
    };
    let dir = format!("{home}.agents/{name}");
    if name == "default" || !valid_name(&name) || !is_dir(&format!("{dir}/.jj")) {
        return Err(format!("no workspace at {dir}"));
    }
    let inside = env::current_dir()
        .map(|cwd| realpath(cwd).starts_with(realpath(&dir)))
        .unwrap_or(false);
    ask_lost(&dir, false, true, &[])?;
    remove(&home, &name)?;
    if inside {
        println!("{home}");
    }
    Ok(())
}

#[derive(Serialize)]
struct Stack {
    index: usize,
    size: usize,
}

#[derive(Serialize)]
struct Info {
    repo: String,
    workspace: String,
    root: String,
    subdir: String,
    change: Option<String>,
    stack: Option<Stack>,
}

/// Prints the workspace and stack of the current directory as JSON (the prompt runs this).
pub fn cmd_info() -> Result<(), String> {
    let cwd = env::current_dir()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    let mut root = cwd.as_str();
    while !is_dir(&format!("{root}/.jj")) {
        if root == "/" || root.is_empty() {
            return Err(String::new());
        }
        root = dirname(root);
    }
    let (repo, workspace, _) = layout(root);
    let mut info = Info {
        repo,
        workspace,
        root: root.to_string(),
        subdir: cwd[root.len()..].trim_start_matches('/').to_string(),
        change: None,
        stack: None,
    };
    if let Some(out) = Jj::in_dir(root).stack_rows() {
        let rows: Vec<Vec<&str>> = out.lines().map(|l| l.split('\t').collect()).collect();
        info.change = rows.iter().find(|r| r[0] != "-").map(|r| r[0].to_string());
        info.stack = Some(Stack {
            index: rows.iter().filter(|r| r.get(1) == Some(&"y")).count(),
            size: rows.len(),
        });
        let mut mine: Vec<(&str, &str)> = rows
            .iter()
            .filter(|r| r[0] != "-")
            .flat_map(|r| r.iter().skip(2).copied())
            .filter(|w| !w.is_empty())
            .map(|w| w.split_once('=').unwrap_or((w, "")))
            .collect();
        if mine.len() > 1 {
            // several workspaces share @: the one whose root is this directory
            let here = realpath(root);
            mine.retain(|(_, r)| !r.is_empty() && realpath(r) == here);
        }
        if let [(name, _)] = mine.as_slice() {
            if !name.is_empty() {
                info.workspace = name.to_string();
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string(&info).map_err(|e| e.to_string())?
    );
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/workspace.rs"]
mod unit_tests;
