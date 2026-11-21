use serde::Deserialize;

use super::Pr;
use crate::api::quote;
use crate::target::Kind;

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

impl From<Mr> for Pr {
    fn from(m: Mr) -> Pr {
        let draft = m.draft || m.work_in_progress;
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
            },
            body: m.description.unwrap_or_default(),
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

fn strip_draft(title: &str) -> &str {
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

pub fn view_path(project: &str, number: u64) -> String {
    format!("projects/{}/merge_requests/{number}", quote(project))
}
