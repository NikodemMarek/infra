use serde::Deserialize;

use super::{quote_plus, text_fields, Api, Field, Forge, Mr};
use crate::jj::{run, Result};

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

pub struct GitHub {
    api: Api,
    owner: String,
    base: String,
    recent: Option<Vec<Mr>>,
}

impl GitHub {
    pub fn new(host: String, project: &str) -> GitHub {
        GitHub {
            api: Api { bin: "gh", host },
            owner: project.split('/').next().unwrap_or_default().to_string(),
            base: format!("repos/{project}/pulls"),
            recent: None,
        }
    }
}

impl Forge for GitHub {
    fn term(&self) -> &'static str {
        "PR"
    }

    fn find(&self, branch: &str) -> Result<Option<Mr>> {
        let path = format!(
            "{}?head={}&state=all&sort=created&direction=desc",
            self.base,
            quote_plus(&format!("{}:{branch}", self.owner))
        );
        let out = self.api.output(&[&path])?;
        Ok(self.api.many::<GitHubPr>(&out)?.into_iter().next())
    }

    fn recent(&mut self) -> Result<&[Mr]> {
        if self.recent.is_none() {
            let path = format!(
                "{}?state=all&sort=updated&direction=desc&per_page=100",
                self.base
            );
            let out = self.api.call("GET", &path, &[])?;
            self.recent = Some(self.api.many::<GitHubPr>(&out)?);
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
        let mr = self.api.send::<GitHubPr>(
            "POST",
            &self.base,
            &[
                ("head", Field::Text(source)),
                ("base", Field::Text(target)),
                ("title", Field::Text(title)),
                ("body", Field::Text(description)),
                ("draft", Field::Flag(draft)),
            ],
        )?;
        self.api.required(mr)
    }

    fn update(
        &self,
        mr: &Mr,
        title: Option<&str>,
        target: Option<&str>,
        description: Option<&str>,
    ) -> Result<Mr> {
        let fields = text_fields(["title", "base", "body"], [title, target, description]);
        if fields.is_empty() {
            return Ok(mr.clone());
        }
        let updated =
            self.api
                .send::<GitHubPr>("PATCH", &format!("{}/{}", self.base, mr.id), &fields)?;
        self.api.required(updated)
    }

    fn set_state(&self, mr: &Mr, open: bool) -> Result<Option<Mr>> {
        let path = format!("{}/{}", self.base, mr.id);
        if !open {
            return self
                .api
                .send::<GitHubPr>("PATCH", &path, &[("state", Field::Text("closed"))]);
        }
        let p = run(
            &self.api.full(&["-X", "PATCH", &path, "-f", "state=open"]),
            false,
        )?;
        if !p.ok {
            eprintln!("  cannot reopen {}; opening a new PR", mr.reference);
            return Ok(None);
        }
        self.api.one::<GitHubPr>(&p.stdout)
    }

    fn upload(&self, path: &str) -> Result<Option<String>> {
        eprintln!("  {path}: GitHub cannot attach files to a PR description; link left as is");
        Ok(None)
    }
}
