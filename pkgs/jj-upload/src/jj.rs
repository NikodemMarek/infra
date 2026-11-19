use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::os::unix::process::CommandExt;
use std::process::Command;

use regex::Regex;

use crate::cmd::{run, Output, Result};

pub const DEFAULT_REVSET: &str = r#"trunk()..(@::) ~ (empty() & description(exact:""))"#;
pub const REMOTE_TITLE: &str = "remote changes on ";
const FS: char = '\x1f';
const RS: char = '\x1e';

const QUIET: &[&str] = &["--no-pager", "--color=never"];

const COMMIT_TEMPLATE: &str = concat!(
    r#"change_id ++ "\x1f" ++ commit_id ++ "\x1f""#,
    r#" ++ parents.map(|p| p.commit_id()).join(",") ++ "\x1f""#,
    r#" ++ local_bookmarks.map(|b| b.name()).join(",") ++ "\x1f""#,
    r#" ++ if(conflict, "1", "0") ++ "\x1f""#,
    r#" ++ if(immutable, "1", "0") ++ "\x1f""#,
    r#" ++ description ++ "\x1e""#,
);
const IS_EMPTY: &str = r#"if(empty, "1", "0")"#;
const HAS_CONFLICT: &str = r#"if(conflict, "1", "0")"#;
const TRUNK_BOOKMARKS: &str =
    r#"remote_bookmarks.map(|b| b.name() ++ "@" ++ b.remote()).join("\n")"#;
const BOOKMARK_TIPS: &str = concat!(
    r#"if(remote && present, name ++ "\x1f" ++ remote ++ "\x1f""#,
    r#" ++ normal_target.commit_id() ++ "\n")"#,
);
const BOOKMARK_PRESENCE: &str =
    r#"name ++ "\x1f" ++ remote ++ "\x1f" ++ if(present, "1", "0") ++ "\n""#;

#[derive(Clone)]
pub struct Commit {
    pub change: String,
    pub commit: String,
    pub parents: Vec<String>,
    pub bookmarks: Vec<String>,
    pub conflict: bool,
    pub immutable: bool,
    pub description: String,
}

impl Commit {
    pub fn short(&self) -> &str {
        trunc(&self.change, 8)
    }
}

pub fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static regex")
}

pub fn trunc(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

pub fn repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn invoke(flags: &[&str], args: &[&str], check: bool) -> Result<Output> {
    run(&[&["jj"][..], flags, args].concat(), check)
}

fn jj(args: &[&str]) -> Result<String> {
    Ok(invoke(QUIET, args, true)?.stdout)
}

pub struct Jj;

impl Jj {
    pub fn config(key: &str) -> Result<Option<String>> {
        let p = invoke(&[], &["config", "get", key], false)?;
        let value = p.stdout.trim();
        Ok((p.ok && !value.is_empty()).then(|| value.to_string()))
    }

    pub fn root() -> Result<String> {
        Ok(jj(&["root"])?.trim().to_string())
    }

    pub fn fetch() -> Result<()> {
        jj(&["git", "fetch"])?;
        Ok(())
    }

    pub fn remote_url(remote: &str) -> Result<Option<String>> {
        let mut url = None;
        for line in jj(&["git", "remote", "list"])?.lines() {
            let (name, u) = line.split_once(' ').unwrap_or((line, ""));
            if name == remote {
                url = Some(u.trim().to_string());
            }
        }
        Ok(url.filter(|u| !u.is_empty()))
    }

    pub fn push(remote: &str, branches: &[&str]) -> Result<Output> {
        let exact: Vec<String> = branches.iter().map(|b| format!("exact:{b}")).collect();
        let mut args = vec!["git", "push", "--remote", remote];
        for name in &exact {
            args.extend(["-b", name.as_str()]);
        }
        invoke(&["--no-pager"], &args, false)
    }

    pub fn file_tracked(path: &str) -> Result<bool> {
        let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
        let out = jj(&[
            "file",
            "list",
            "-r",
            "@",
            "--",
            &format!(r#"file:"{escaped}""#),
        ])?;
        Ok(!out.trim().is_empty())
    }

    pub fn commits(revset: &str) -> Result<Vec<Commit>> {
        let out = jj(&[
            "log",
            "--no-graph",
            "--reversed",
            "-r",
            revset,
            "-T",
            COMMIT_TEMPLATE,
        ])?;
        let list = |s: &str| -> Vec<String> {
            s.split(',')
                .filter(|p| !p.is_empty())
                .map(String::from)
                .collect()
        };
        let mut commits = Vec::new();
        for rec in out.split(RS) {
            if rec.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = rec.trim_start_matches('\n').splitn(7, FS).collect();
            let [change, commit, parents, bookmarks, conflict, immutable, desc] = f[..] else {
                return Err(format!("unexpected `jj log` output: {}", repr(rec)));
            };
            commits.push(Commit {
                change: change.to_string(),
                commit: commit.to_string(),
                parents: list(parents),
                bookmarks: list(bookmarks),
                conflict: conflict == "1",
                immutable: immutable == "1",
                description: desc.trim().to_string(),
            });
        }
        Ok(commits)
    }

    fn log_field(revset: &str, field: &str) -> Result<Vec<String>> {
        let out = jj(&[
            "log",
            "--no-graph",
            "-r",
            revset,
            "-T",
            &format!(r#"{field} ++ "\n""#),
        ])?;
        Ok(out.split_whitespace().map(String::from).collect())
    }

    pub fn commit_ids(revset: &str) -> Result<Vec<String>> {
        Self::log_field(revset, "commit_id")
    }

    pub fn change_ids(revset: &str) -> Result<Vec<String>> {
        Self::log_field(revset, "change_id")
    }

    pub fn is_empty(rev: &str) -> Result<bool> {
        Ok(Self::log_field(rev, IS_EMPTY)? == ["1"])
    }

    pub fn has_conflict(rev: &str) -> Result<bool> {
        Ok(Self::log_field(rev, HAS_CONFLICT)? == ["1"])
    }

    pub fn evolog_commit_ids(rev: &str) -> Result<Vec<String>> {
        let out = jj(&[
            "evolog",
            "-r",
            rev,
            "--no-graph",
            "-T",
            r#"commit.commit_id() ++ "\n""#,
        ])?;
        Ok(out.split_whitespace().map(String::from).collect())
    }

    pub fn interdiff_is_empty(from: &str, to: &str) -> Result<bool> {
        let out = jj(&["interdiff", "--from", from, "--to", to, "--summary"])?;
        Ok(out.trim().is_empty())
    }

    pub fn trunk_branch(remote: &str) -> Result<String> {
        if let Some(configured) = Self::config("upload.target-branch")? {
            return Ok(configured);
        }
        let out = jj(&["log", "--no-graph", "-r", "trunk()", "-T", TRUNK_BOOKMARKS])?;
        for line in out.lines() {
            let (name, rem) = line.rsplit_once('@').unwrap_or(("", line));
            if rem == remote {
                return Ok(name.to_string());
            }
        }
        Err(format!(
            "cannot find a {remote} bookmark on trunk(); set upload.target-branch"
        ))
    }

    pub fn remote_branches(remote: &str) -> Result<BTreeMap<String, String>> {
        let out = jj(&["bookmark", "list", "--all-remotes", "-T", BOOKMARK_TIPS])?;
        let mut tips = BTreeMap::new();
        for line in out.lines() {
            let f: Vec<&str> = line.split(FS).collect();
            let [name, rem, commit] = f[..] else {
                return Err(format!(
                    "unexpected `jj bookmark list` output: {}",
                    repr(line)
                ));
            };
            if rem == remote {
                tips.insert(name.to_string(), commit.to_string());
            }
        }
        Ok(tips)
    }

    pub fn deleted_managed_bookmarks(prefix: &str, remote: &str) -> Result<Vec<String>> {
        let out = jj(&["bookmark", "list", "--all-remotes", "-T", BOOKMARK_PRESENCE])?;
        let mut local_gone = BTreeSet::new();
        let mut on_remote = BTreeSet::new();
        for line in out.lines() {
            let f: Vec<&str> = line.split(FS).collect();
            let [name, rem, present] = f[..] else {
                return Err(format!(
                    "unexpected `jj bookmark list` output: {}",
                    repr(line)
                ));
            };
            if !name.starts_with(prefix) {
                continue;
            }
            if rem.is_empty() && present == "0" {
                local_gone.insert(name.to_string());
            } else if rem == remote && present == "1" {
                on_remote.insert(name.to_string());
            }
        }
        Ok(local_gone.intersection(&on_remote).cloned().collect())
    }

    pub fn bookmark_set(name: &str, rev: &str) -> Result<()> {
        jj(&["bookmark", "set", name, "-r", rev, "--allow-backwards"])?;
        Ok(())
    }

    pub fn bookmark_delete(names: &[String]) -> Result<()> {
        let exact: Vec<String> = names.iter().map(|n| format!("exact:{n}")).collect();
        let mut args = vec!["bookmark", "delete"];
        args.extend(exact.iter().map(String::as_str));
        jj(&args)?;
        Ok(())
    }

    pub fn describe(rev: &str, message: &str) -> Result<()> {
        jj(&["describe", "-r", rev, "-m", message])?;
        Ok(())
    }

    pub fn abandon(rev: &str) -> Result<()> {
        jj(&["abandon", rev])?;
        Ok(())
    }

    pub fn abandon_all(revs: &[&str]) -> Result<()> {
        jj(&[&["abandon"][..], revs].concat())?;
        Ok(())
    }

    pub fn new_on(rev: &str) -> Result<()> {
        jj(&["new", rev])?;
        Ok(())
    }

    pub fn new_detached(base: &str, message: &str) -> Result<()> {
        jj(&["new", base, "--no-edit", "-m", message])?;
        Ok(())
    }

    pub fn restore(from: &str, into: &str) -> Result<()> {
        jj(&["restore", "--from", from, "--into", into])?;
        Ok(())
    }

    pub fn squash_into(from: &str, into: &str) -> Result<()> {
        jj(&["squash", "--from", from, "--into", into, "-u"])?;
        Ok(())
    }

    pub fn rebase_onto_trunk(also: Option<&str>) -> Result<()> {
        let mut args = vec!["rebase", "-b", "@"];
        if let Some(change) = also {
            args.extend(["-b", change]);
        }
        args.extend(["-o", "trunk()", "--skip-emptied"]);
        jj(&args)?;
        Ok(())
    }

    pub fn rebase_onto(rev: &str, dest: &str) -> Result<()> {
        jj(&["rebase", "-r", rev, "-o", dest])?;
        Ok(())
    }

    pub fn rebase_after(rev: &str, after: &str) -> Result<()> {
        jj(&["rebase", "-r", rev, "-A", after])?;
        Ok(())
    }

    pub fn exec_log_stack(args: &[String]) -> String {
        let e = Command::new("jj")
            .args(["log", "-r", "stack()"])
            .args(args)
            .exec();
        if e.kind() == ErrorKind::NotFound {
            "jj not found on PATH".to_string()
        } else {
            e.to_string()
        }
    }
}

pub fn parse_remote_url(url: &str) -> Result<(String, String)> {
    let caps = re(r"^(?:ssh|https?|git)://(?:[^@/]+@)?([^/:]+)(?::\d+)?/(.+?)(?:\.git)?/?$")
        .captures(url)
        .or_else(|| re(r"^(?:[^@/]+@)?([^/:]+):(.+?)(?:\.git)?/?$").captures(url))
        .ok_or_else(|| {
            format!(
                "cannot parse remote URL {}; set upload.host and upload.project",
                repr(url)
            )
        })?;
    Ok((caps[1].to_string(), caps[2].to_string()))
}

pub fn forge_target(remote: &str) -> Result<(String, String)> {
    let host = Jj::config("upload.host")?;
    let project = Jj::config("upload.project")?;
    if let (Some(host), Some(project)) = (&host, &project) {
        return Ok((host.clone(), project.clone()));
    }
    let url =
        Jj::remote_url(remote)?.ok_or_else(|| format!("remote {} not found", repr(remote)))?;
    let (h, p) = parse_remote_url(&url)?;
    Ok((host.unwrap_or(h), project.unwrap_or(p)))
}

pub fn slugify(text: &str, limit: usize) -> String {
    let lower = text.to_lowercase();
    let slug = re(r"[^a-z0-9]+").replace_all(&lower, "-");
    let slug = trunc(slug.trim_matches('-'), limit).trim_end_matches('-');
    (if slug.is_empty() { "change" } else { slug }).to_string()
}

pub fn default_prefix() -> Result<String> {
    let email = Jj::config("user.email")?.unwrap_or_default();
    let local = email.split('@').next().unwrap_or_default();
    let user: String = local
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "._-".contains(*c))
        .collect();
    Ok(format!("{}/", if user.is_empty() { "jj" } else { &user }))
}

pub fn choose_bookmark(c: &Commit, prefix: &str) -> (String, Vec<String>) {
    let managed: Vec<&String> = c
        .bookmarks
        .iter()
        .filter(|b| b.starts_with(prefix))
        .collect();
    let own = managed.iter().find(|b| {
        c.change
            .starts_with(b.rsplit('-').next().unwrap_or_default())
    });
    let keep = match (own, managed.first()) {
        (Some(b), _) | (None, Some(b)) => *b,
        (None, None) => {
            if let Some(b) = c.bookmarks.first() {
                return (b.clone(), vec![]);
            }
            let title = c.description.lines().next().unwrap_or_default();
            return (
                format!("{prefix}{}-{}", slugify(title, 40), c.short()),
                vec![],
            );
        }
    };
    let extra = managed
        .iter()
        .filter(|b| **b != keep)
        .map(|b| (*b).clone())
        .collect();
    (keep.clone(), extra)
}
