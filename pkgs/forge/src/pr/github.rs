use serde::Deserialize;

use super::{Checks, Comment, FailedJob, Pr};
use crate::api::{quote, Api};
use crate::cmd::Result;
use crate::target::Kind;

#[derive(Deserialize)]
struct Branch {
    #[serde(rename = "ref")]
    name: String,
    sha: Option<String>,
}

#[derive(Deserialize)]
pub struct Pull {
    number: u64,
    html_url: String,
    state: String,
    merged_at: Option<String>,
    #[serde(default)]
    draft: bool,
    title: String,
    base: Branch,
    head: Branch,
    body: Option<String>,
    merge_commit_sha: Option<String>,
}

impl From<Pull> for Pr {
    fn from(p: Pull) -> Pr {
        let merged = p.merged_at.is_some();
        Pr {
            forge: Kind::GitHub,
            number: p.number,
            reference: format!("#{}", p.number),
            url: p.html_url,
            state: if merged { "merged".into() } else { p.state },
            draft: p.draft,
            title: p.title,
            body: p.body.unwrap_or_default(),
            base: p.base.name,
            head: p.head.name,
            head_sha: p.head.sha.filter(|s| !s.is_empty()),
            landed: p
                .merge_commit_sha
                .filter(|s| merged && !s.is_empty())
                .into_iter()
                .collect(),
        }
    }
}

pub fn list_path(project: &str, head: Option<&str>, limit: u8) -> String {
    match head {
        Some(branch) => {
            let owner = project.split('/').next().unwrap_or_default();
            let head = quote(&format!("{owner}:{branch}"));
            format!(
                "repos/{project}/pulls?head={head}&state=all&sort=created&direction=desc&per_page={limit}"
            )
        }
        None => {
            format!("repos/{project}/pulls?state=all&sort=updated&direction=desc&per_page={limit}")
        }
    }
}

pub fn create_path(project: &str) -> String {
    format!("repos/{project}/pulls")
}

pub fn view_path(project: &str, number: u64) -> String {
    format!("repos/{project}/pulls/{number}")
}

#[derive(Deserialize)]
struct HeadSha {
    sha: Option<String>,
}

#[derive(Deserialize)]
struct PullHead {
    html_url: String,
    head: HeadSha,
}

#[derive(Deserialize)]
struct CheckRuns {
    check_runs: Vec<CheckRun>,
}

#[derive(Deserialize)]
struct CheckRun {
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
}

pub fn checks(api: &Api, project: &str, number: u64) -> Result<Checks> {
    let pull: PullHead = api.get(&view_path(project, number))?;
    let Some(sha) = pull.head.sha.filter(|s| !s.is_empty()) else {
        return Ok(Checks {
            sha: None,
            state: "none".into(),
            url: None,
            failed: Vec::new(),
        });
    };
    let runs: CheckRuns = api.get(&format!(
        "repos/{project}/commits/{sha}/check-runs?filter=latest&per_page=100"
    ))?;
    let runs = runs.check_runs;
    let is_failed = |r: &CheckRun| matches!(r.conclusion.as_deref(), Some("failure" | "timed_out"));
    let state = if runs.is_empty() {
        "none"
    } else if runs.iter().any(is_failed) {
        "failed"
    } else if runs.iter().any(|r| r.status != "completed") {
        "pending"
    } else {
        "success"
    };
    Ok(Checks {
        sha: Some(sha),
        state: state.into(),
        url: Some(format!("{}/checks", pull.html_url)),
        failed: runs
            .into_iter()
            .filter(is_failed)
            .map(|r| FailedJob {
                name: r.name,
                url: r.html_url,
            })
            .collect(),
    })
}

#[derive(Deserialize)]
struct User {
    login: String,
}

#[derive(Deserialize)]
struct Review {
    id: u64,
    state: String,
    body: Option<String>,
    submitted_at: Option<String>,
    html_url: Option<String>,
    user: Option<User>,
    author_association: Option<String>,
}

#[derive(Deserialize)]
struct Note {
    id: u64,
    pull_request_review_id: Option<u64>,
    body: Option<String>,
    created_at: String,
    html_url: Option<String>,
    user: Option<User>,
    author_association: Option<String>,
    path: Option<String>,
    line: Option<u64>,
    original_line: Option<u64>,
}

fn login(user: Option<User>) -> String {
    user.map_or_else(|| "ghost".into(), |u| u.login)
}

pub fn comments(api: &Api, project: &str, number: u64) -> Result<Vec<Comment>> {
    let reviews: Vec<Review> = api.get_pages(&format!(
        "repos/{project}/pulls/{number}/reviews?per_page=100"
    ))?;
    let issue_comments: Vec<Note> = api.get_pages(&format!(
        "repos/{project}/issues/{number}/comments?per_page=100"
    ))?;
    let inline_comments: Vec<Note> = api.get_pages(&format!(
        "repos/{project}/pulls/{number}/comments?per_page=100"
    ))?;

    let pending: Vec<u64> = reviews
        .iter()
        .filter(|r| r.state == "PENDING")
        .map(|r| r.id)
        .collect();
    let mut comments = Vec::new();
    for r in reviews {
        let blank = r.body.as_deref().unwrap_or_default().trim().is_empty();
        if matches!(r.state.as_str(), "PENDING" | "DISMISSED") || (r.state == "COMMENTED" && blank)
        {
            continue;
        }
        comments.push(Comment {
            id: r.id,
            kind: "review".into(),
            author: login(r.user),
            association: r.author_association,
            created_at: r.submitted_at.unwrap_or_default(),
            body: r.body.unwrap_or_default(),
            url: r.html_url,
            path: None,
            line: None,
            review_state: Some(r.state.to_lowercase()),
        });
    }
    for n in issue_comments {
        comments.push(Comment {
            id: n.id,
            kind: "comment".into(),
            author: login(n.user),
            association: n.author_association,
            created_at: n.created_at,
            body: n.body.unwrap_or_default(),
            url: n.html_url,
            path: None,
            line: None,
            review_state: None,
        });
    }
    for n in inline_comments {
        if n.pull_request_review_id
            .is_some_and(|id| pending.contains(&id))
        {
            continue;
        }
        comments.push(Comment {
            id: n.id,
            kind: "inline".into(),
            author: login(n.user),
            association: n.author_association,
            created_at: n.created_at,
            body: n.body.unwrap_or_default(),
            url: n.html_url,
            path: n.path,
            line: n.line.or(n.original_line),
            review_state: None,
        });
    }
    comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(comments)
}
