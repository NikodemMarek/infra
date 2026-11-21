use serde::Deserialize;

use super::Pr;
use crate::api::quote;
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
