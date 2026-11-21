use serde::Deserialize;

use super::{Checks, Comment, FailedJob, Pr};
use crate::api::{quote, Api};
use crate::cmd::Result;
use crate::target::{Kind, Target};

#[derive(Deserialize)]
pub struct Mr {
    iid: u64,
    web_url: String,
    state: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    work_in_progress: bool,
    title: String,
    target_branch: String,
    source_branch: String,
    description: Option<String>,
    sha: Option<String>,
    merge_commit_sha: Option<String>,
    squash_commit_sha: Option<String>,
}

impl Mr {
    pub fn is_draft(&self) -> bool {
        self.draft || self.work_in_progress
    }
}

impl From<Mr> for Pr {
    fn from(m: Mr) -> Pr {
        let draft = m.is_draft();
        Pr {
            forge: Kind::GitLab,
            number: m.iid,
            reference: format!("!{}", m.iid),
            url: m.web_url,
            state: if matches!(m.state.as_str(), "opened" | "locked") {
                "open".into()
            } else {
                m.state
            },
            draft,
            title: if draft {
                strip_draft(&m.title).to_string()
            } else {
                m.title
            }
            .replace("\r\n", "\n"),
            body: m.description.unwrap_or_default().replace("\r\n", "\n"),
            base: m.target_branch,
            head: m.source_branch,
            head_sha: m.sha.filter(|s| !s.is_empty()),
            landed: [m.merge_commit_sha, m.squash_commit_sha]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect(),
        }
    }
}

pub(super) fn strip_draft(title: &str) -> &str {
    let trimmed = title.trim_start();
    for prefix in ["[draft]", "(draft)", "draft:"] {
        let matches = trimmed
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix));
        if matches {
            return trimmed[prefix.len()..].trim_start();
        }
    }
    title
}

pub fn list_path(project: &str, head: Option<&str>, limit: u8) -> String {
    let project = quote(project);
    match head {
        Some(branch) => format!(
            "projects/{project}/merge_requests?source_branch={}&state=all&order_by=created_at&sort=desc&per_page={limit}",
            quote(branch)
        ),
        None => format!(
            "projects/{project}/merge_requests?state=all&order_by=updated_at&sort=desc&per_page={limit}"
        ),
    }
}

pub fn create_path(project: &str) -> String {
    format!("projects/{}/merge_requests", quote(project))
}

pub fn view_path(project: &str, number: u64) -> String {
    format!("projects/{}/merge_requests/{number}", quote(project))
}

#[derive(Deserialize)]
struct HeadPipeline {
    id: u64,
    sha: Option<String>,
    status: String,
    web_url: Option<String>,
}

#[derive(Deserialize)]
struct MrPipeline {
    sha: Option<String>,
    head_pipeline: Option<HeadPipeline>,
}

#[derive(Deserialize)]
struct Job {
    name: String,
    web_url: Option<String>,
    #[serde(default)]
    allow_failure: bool,
}

pub fn checks(api: &Api, project: &str, number: u64) -> Result<Checks> {
    let mr: MrPipeline = api.get(&view_path(project, number))?;
    let Some(pipeline) = mr.head_pipeline else {
        return Ok(Checks {
            sha: mr.sha.filter(|s| !s.is_empty()),
            state: "none".into(),
            url: None,
            failed: Vec::new(),
        });
    };
    let state = match pipeline.status.as_str() {
        "success" => "success",
        "failed" => "failed",
        "canceled" | "skipped" => "canceled",
        _ => "pending",
    };
    let failed = if state == "failed" {
        let path = format!(
            "projects/{}/pipelines/{}/jobs?scope%5B%5D=failed&per_page=100",
            quote(project),
            pipeline.id
        );
        api.get::<Vec<Job>>(&path)?
            .into_iter()
            .filter(|j| !j.allow_failure)
            .map(|j| FailedJob {
                name: j.name,
                url: j.web_url,
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(Checks {
        sha: pipeline.sha.filter(|s| !s.is_empty()),
        state: state.into(),
        url: pipeline.web_url,
        failed,
    })
}

#[derive(Deserialize)]
struct NoteAuthor {
    username: String,
}

#[derive(Deserialize)]
struct NotePosition {
    new_path: Option<String>,
    old_path: Option<String>,
    new_line: Option<u64>,
    old_line: Option<u64>,
}

#[derive(Deserialize)]
struct Note {
    id: u64,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    system: bool,
    body: Option<String>,
    created_at: String,
    author: NoteAuthor,
    position: Option<NotePosition>,
}

pub fn comments(api: &Api, target: &Target, number: u64) -> Result<Vec<Comment>> {
    let path = format!(
        "{}/notes?sort=desc&order_by=created_at&per_page=100",
        view_path(&target.project, number)
    );
    let mut comments: Vec<Comment> = api
        .get::<Vec<Note>>(&path)?
        .into_iter()
        .filter(|n| !n.system)
        .map(|n| {
            let inline = n.kind.as_deref() == Some("DiffNote");
            let position = n.position.filter(|_| inline);
            Comment {
                id: n.id,
                kind: if inline { "inline" } else { "comment" }.into(),
                author: n.author.username,
                association: None,
                created_at: n.created_at,
                body: n.body.unwrap_or_default(),
                url: Some(format!(
                    "https://{}/{}/-/merge_requests/{number}#note_{}",
                    target.host, target.project, n.id
                )),
                path: position
                    .as_ref()
                    .and_then(|p| p.new_path.clone().or_else(|| p.old_path.clone())),
                line: position.as_ref().and_then(|p| p.new_line.or(p.old_line)),
                review_state: None,
            }
        })
        .collect();
    comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(comments)
}
