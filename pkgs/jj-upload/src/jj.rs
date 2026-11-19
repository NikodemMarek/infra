use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::process::{Command, Stdio};

use regex::Regex;

pub type Result<T> = std::result::Result<T, String>;

pub const DEFAULT_REVSET: &str = r#"trunk()..(@::) ~ (empty() & description(exact:""))"#;
pub const REMOTE_TITLE: &str = "remote changes on ";
const FS: char = '\x1f';
const RS: char = '\x1e';

const COMMIT_TEMPLATE: &str = concat!(
    r#"change_id ++ "\x1f" ++ commit_id ++ "\x1f""#,
    r#" ++ parents.map(|p| p.commit_id()).join(",") ++ "\x1f""#,
    r#" ++ local_bookmarks.map(|b| b.name()).join(",") ++ "\x1f""#,
    r#" ++ if(conflict, "1", "0") ++ "\x1f""#,
    r#" ++ if(immutable, "1", "0") ++ "\x1f""#,
    r#" ++ description ++ "\x1e""#,
);

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

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

pub fn jj(args: &[&str]) -> Result<String> {
    let mut cmd = vec!["jj", "--no-pager", "--color=never"];
    cmd.extend_from_slice(args);
    Ok(run(&cmd, true)?.stdout)
}

pub fn jj_config(key: &str) -> Result<Option<String>> {
    let p = run(&["jj", "config", "get", key], false)?;
    let value = p.stdout.trim();
    Ok((p.ok && !value.is_empty()).then(|| value.to_string()))
}

pub fn load_commits(revset: &str) -> Result<Vec<Commit>> {
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

pub fn trunk_branch(remote: &str) -> Result<String> {
    if let Some(configured) = jj_config("upload.target-branch")? {
        return Ok(configured);
    }
    let out = jj(&[
        "log",
        "--no-graph",
        "-r",
        "trunk()",
        "-T",
        r#"remote_bookmarks.map(|b| b.name() ++ "@" ++ b.remote()).join("\n")"#,
    ])?;
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

pub fn log_field(revset: &str, field: &str) -> Result<Vec<String>> {
    let template = format!(r#"{field} ++ "\n""#);
    Ok(jj(&["log", "--no-graph", "-r", revset, "-T", &template])?
        .split_whitespace()
        .map(String::from)
        .collect())
}

pub fn log_ids(revset: &str) -> Result<Vec<String>> {
    log_field(revset, "commit_id")
}

pub fn remote_branches(remote: &str) -> Result<BTreeMap<String, String>> {
    let out = jj(&[
        "bookmark",
        "list",
        "--all-remotes",
        "-T",
        concat!(
            r#"if(remote && present, name ++ "\x1f" ++ remote ++ "\x1f""#,
            r#" ++ normal_target.commit_id() ++ "\n")"#,
        ),
    ])?;
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
    let out = jj(&[
        "bookmark",
        "list",
        "--all-remotes",
        "-T",
        r#"name ++ "\x1f" ++ remote ++ "\x1f" ++ if(present, "1", "0") ++ "\n""#,
    ])?;
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
    let host = jj_config("upload.host")?;
    let project = jj_config("upload.project")?;
    if let (Some(host), Some(project)) = (&host, &project) {
        return Ok((host.clone(), project.clone()));
    }
    let mut url = None;
    for line in jj(&["git", "remote", "list"])?.lines() {
        let (name, u) = line.split_once(' ').unwrap_or((line, ""));
        if name == remote {
            url = Some(u.trim().to_string());
        }
    }
    let url = url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| format!("remote {} not found", repr(remote)))?;
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
    let email = jj_config("user.email")?.unwrap_or_default();
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
