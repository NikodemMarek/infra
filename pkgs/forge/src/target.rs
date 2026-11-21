use serde::Serialize;

use crate::cmd::Result;
use crate::jj;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    GitLab,
    GitHub,
}

impl Kind {
    pub fn bin(self) -> &'static str {
        match self {
            Kind::GitLab => "glab",
            Kind::GitHub => "gh",
        }
    }
}

#[derive(Serialize)]
pub struct Target {
    #[serde(rename = "forge")]
    pub kind: Kind,
    pub host: String,
    pub project: String,
}

pub fn resolve(remote: Option<&str>) -> Result<Target> {
    jj::root()?;
    let kind = match jj::config("forge.kind")?.as_deref() {
        None => None,
        Some("gitlab") => Some(Kind::GitLab),
        Some("github") => Some(Kind::GitHub),
        Some(other) => {
            return Err(format!(
                "unknown forge.kind '{other}'; known: gitlab, github"
            ))
        }
    };
    let host = jj::config("forge.host")?;
    let project = jj::config("forge.project")?;
    let (host, project) = match (host, project) {
        (Some(host), Some(project)) => (host, project),
        (host, project) => {
            let name = remote.unwrap_or("origin");
            let url = jj::remote_url(name)?.ok_or_else(|| format!("remote '{name}' not found"))?;
            let (url_host, url_project) = parse_remote_url(&url)?;
            (host.unwrap_or(url_host), project.unwrap_or(url_project))
        }
    };
    let kind = kind.unwrap_or_else(|| {
        if host.to_lowercase().contains("github") {
            Kind::GitHub
        } else {
            Kind::GitLab
        }
    });
    Ok(Target {
        kind,
        host,
        project,
    })
}

pub fn resolve_pr(remote: Option<&str>, arg: &str) -> Result<(Target, u64)> {
    if !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_digit()) {
        let number = arg
            .parse()
            .map_err(|_| format!("invalid PR number '{arg}'"))?;
        return Ok((resolve(remote)?, number));
    }
    parse_pr_url(arg)
}

fn parse_pr_url(arg: &str) -> Result<(Target, u64)> {
    let bad = || format!("'{arg}' is not a merge/pull request number or URL");
    let rest = arg.strip_prefix("https://").ok_or_else(bad)?;
    let (host, path) = rest.split_once('/').ok_or_else(bad)?;
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let (kind, project, number) =
        if let Some((project, tail)) = path.split_once("/-/merge_requests/") {
            (Kind::GitLab, project.to_string(), tail.split('/').next())
        } else if let [owner, repo, "pull", number, ..] = path.split('/').collect::<Vec<_>>()[..] {
            if ![owner, repo].into_iter().all(valid_github_segment) {
                return Err(bad());
            }
            (Kind::GitHub, format!("{owner}/{repo}"), Some(number))
        } else {
            return Err(bad());
        };
    let number = number.and_then(|n| n.parse().ok()).ok_or_else(bad)?;
    if host.is_empty() || project.is_empty() {
        return Err(bad());
    }
    Ok((
        Target {
            kind,
            host: host.to_string(),
            project,
        },
        number,
    ))
}

fn valid_github_segment(s: &str) -> bool {
    !matches!(s, "" | "." | "..") && !s.contains(':')
}

fn parse_remote_url(url: &str) -> Result<(String, String)> {
    split_remote_url(url)
        .filter(|(host, project)| !host.is_empty() && !project.is_empty())
        .map(|(host, project)| (host.to_string(), project.to_string()))
        .ok_or_else(|| format!("cannot parse remote URL '{url}'; set forge.host and forge.project"))
}

fn split_remote_url(url: &str) -> Option<(&str, &str)> {
    let (host, path) = match ["ssh://", "https://", "http://", "git://"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme))
    {
        Some(rest) => {
            let (authority, path) = rest.split_once('/')?;
            let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            (hostport.split(':').next()?, path)
        }
        None => {
            // scp-like: [user@]host:path
            let (head, path) = url.split_once(':')?;
            let host = head.split_once('@').map_or(head, |(_, h)| h);
            if head.contains('/') {
                return None;
            }
            (host, path)
        }
    };
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    Some((host, path))
}
