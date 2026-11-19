use serde::Deserialize;

use super::{quote_plus, Api, Forge, Mr};
use crate::cmd::Result;

#[derive(Deserialize)]
struct GitHubBranch {
    #[serde(rename = "ref")]
    name: String,
    sha: Option<String>,
}

#[derive(Deserialize)]
struct GitHubPr {
    number: u64,
    html_url: String,
    state: String,
    merged_at: Option<String>,
    #[serde(default)]
    draft: bool,
    title: String,
    base: GitHubBranch,
    head: GitHubBranch,
    body: Option<String>,
    merge_commit_sha: Option<String>,
}

impl From<GitHubPr> for Mr {
    fn from(p: GitHubPr) -> Mr {
        Mr {
            id: p.number.to_string(),
            reference: format!("#{}", p.number),
            url: p.html_url,
            state: if p.merged_at.is_some() {
                "merged".into()
            } else {
                p.state
            },
            draft: p.draft,
            title: p.title,
            target: p.base.name,
            source: p.head.name,
            description: p.body.unwrap_or_default(),
            head: p.head.sha.filter(|s| !s.is_empty()),
            landed: p
                .merge_commit_sha
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect(),
        }
    }
}

struct Gh {
    api: Api,
}

impl Gh {
    fn new(host: String) -> Gh {
        Gh {
            api: Api { bin: "gh", host },
        }
    }

    fn find_prs(&self, project: &str, branch: &str) -> Result<Vec<GitHubPr>> {
        let owner = project.split('/').next().unwrap_or_default();
        let head = quote_plus(&format!("{owner}:{branch}"));
        self.api.many(&[&format!(
            "repos/{project}/pulls?head={head}&state=all&sort=created&direction=desc"
        )])
    }

    fn recent_prs(&self, project: &str) -> Result<Vec<GitHubPr>> {
        self.api.many(&[
            "-X",
            "GET",
            &format!("repos/{project}/pulls?state=all&sort=updated&direction=desc&per_page=100"),
        ])
    }

    fn create_pr(
        &self,
        project: &str,
        source: &str,
        target: &str,
        title: &str,
        description: &str,
        draft: bool,
    ) -> Result<GitHubPr> {
        self.api.required(&[
            "-X",
            "POST",
            &format!("repos/{project}/pulls"),
            "-f",
            &format!("head={source}"),
            "-f",
            &format!("base={target}"),
            "-f",
            &format!("title={title}"),
            "-f",
            &format!("body={description}"),
            "-F",
            &format!("draft={draft}"),
        ])
    }

    fn update_pr(
        &self,
        project: &str,
        number: &str,
        title: Option<&str>,
        target: Option<&str>,
        description: Option<&str>,
    ) -> Result<GitHubPr> {
        let url = format!("repos/{project}/pulls/{number}");
        let fields: Vec<String> = [
            title.map(|v| format!("title={v}")),
            target.map(|v| format!("base={v}")),
            description.map(|v| format!("body={v}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut args = vec!["-X", "PATCH", url.as_str()];
        for field in &fields {
            args.extend(["-f", field.as_str()]);
        }
        self.api.required(&args)
    }

    fn close_pr(&self, project: &str, number: &str) -> Result<Option<GitHubPr>> {
        self.api.one(&[
            "-X",
            "PATCH",
            &format!("repos/{project}/pulls/{number}"),
            "-f",
            "state=closed",
        ])
    }

    fn reopen_pr(&self, project: &str, number: &str) -> Result<Option<GitHubPr>> {
        let p = self.api.run(
            &[
                "-X",
                "PATCH",
                &format!("repos/{project}/pulls/{number}"),
                "-f",
                "state=open",
            ],
            false,
        )?;
        if !p.ok {
            return Ok(None);
        }
        self.api.decode(&p.stdout)
    }
}

pub struct GitHub {
    gh: Gh,
    project: String,
    recent: Option<Vec<Mr>>,
}

impl GitHub {
    pub fn new(host: String, project: &str) -> GitHub {
        GitHub {
            gh: Gh::new(host),
            project: project.to_string(),
            recent: None,
        }
    }
}

impl Forge for GitHub {
    fn term(&self) -> &'static str {
        "PR"
    }

    fn find(&self, branch: &str) -> Result<Option<Mr>> {
        let prs = self.gh.find_prs(&self.project, branch)?;
        Ok(prs.into_iter().next().map(Mr::from))
    }

    fn recent(&mut self) -> Result<&[Mr]> {
        if self.recent.is_none() {
            let prs = self.gh.recent_prs(&self.project)?;
            self.recent = Some(prs.into_iter().map(Mr::from).collect());
        }
        Ok(self.recent.as_deref().unwrap_or_default())
    }

    fn create(
        &self,
        source: &str,
        target: &str,
        title: &str,
        description: &str,
        draft: bool,
    ) -> Result<Mr> {
        let pr = self
            .gh
            .create_pr(&self.project, source, target, title, description, draft)?;
        Ok(pr.into())
    }

    fn update(
        &self,
        mr: &Mr,
        title: Option<&str>,
        target: Option<&str>,
        description: Option<&str>,
    ) -> Result<Mr> {
        if title.is_none() && target.is_none() && description.is_none() {
            return Ok(mr.clone());
        }
        let updated = self
            .gh
            .update_pr(&self.project, &mr.id, title, target, description)?;
        Ok(updated.into())
    }

    fn set_state(&self, mr: &Mr, open: bool) -> Result<Option<Mr>> {
        let updated = if open {
            let reopened = self.gh.reopen_pr(&self.project, &mr.id)?;
            if reopened.is_none() {
                eprintln!("  cannot reopen {}; opening a new PR", mr.reference);
            }
            reopened
        } else {
            self.gh.close_pr(&self.project, &mr.id)?
        };
        Ok(updated.map(Mr::from))
    }

    fn upload(&self, path: &str) -> Result<Option<String>> {
        eprintln!("  {path}: GitHub cannot attach files to a PR description; link left as is");
        Ok(None)
    }
}
