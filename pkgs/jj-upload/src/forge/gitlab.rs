use serde::Deserialize;

use super::{quote, quote_plus, text_fields, Api, Field, Forge, Mr};
use crate::jj::{re, run, Result};

#[derive(Deserialize)]
struct GitLabMr {
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

impl From<GitLabMr> for Mr {
    fn from(m: GitLabMr) -> Mr {
        let draft = m.draft || m.work_in_progress;
        Mr {
            id: m.iid.to_string(),
            reference: format!("!{}", m.iid),
            url: m.web_url,
            state: if m.state == "opened" {
                "open".into()
            } else {
                m.state
            },
            draft,
            title: if draft {
                re(r"(?i)^\s*(?:\[draft\]|\(draft\)|draft:)\s*")
                    .replace(&m.title, "")
                    .into_owned()
            } else {
                m.title
            },
            target: m.target_branch,
            source: m.source_branch,
            description: m.description.unwrap_or_default(),
            head: m.sha.filter(|s| !s.is_empty()),
            landed: [m.merge_commit_sha, m.squash_commit_sha]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect(),
        }
    }
}

#[derive(Deserialize)]
struct Upload {
    url: String,
}

pub struct GitLab {
    api: Api,
    prefix: String,
    base: String,
    recent: Option<Vec<Mr>>,
}

impl GitLab {
    pub fn new(host: String, project: &str) -> GitLab {
        let prefix = format!("projects/{}", quote(project));
        let base = format!("{prefix}/merge_requests");
        GitLab {
            api: Api { bin: "glab", host },
            prefix,
            base,
            recent: None,
        }
    }
}

impl Forge for GitLab {
    fn term(&self) -> &'static str {
        "MR"
    }

    fn find(&self, branch: &str) -> Result<Option<Mr>> {
        let path = format!(
            "{}?source_branch={}&order_by=created_at&sort=desc",
            self.base,
            quote_plus(branch)
        );
        let out = self.api.output(&[&path])?;
        Ok(self.api.many::<GitLabMr>(&out)?.into_iter().next())
    }

    fn recent(&mut self) -> Result<&[Mr]> {
        if self.recent.is_none() {
            let path = format!(
                "{}?state=all&order_by=updated_at&sort=desc&per_page=100",
                self.base
            );
            let out = self.api.call("GET", &path, &[])?;
            self.recent = Some(self.api.many::<GitLabMr>(&out)?);
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
        let title = if draft {
            format!("Draft: {title}")
        } else {
            title.to_string()
        };
        let mr = self.api.send::<GitLabMr>(
            "POST",
            &self.base,
            &[
                ("source_branch", Field::Text(source)),
                ("target_branch", Field::Text(target)),
                ("title", Field::Text(&title)),
                ("description", Field::Text(description)),
                ("remove_source_branch", Field::Text("true")),
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
        let title = title.map(|t| {
            if mr.draft {
                format!("Draft: {t}")
            } else {
                t.to_string()
            }
        });
        let fields = text_fields(
            ["title", "target_branch", "description"],
            [title.as_deref(), target, description],
        );
        if fields.is_empty() {
            return Ok(mr.clone());
        }
        let updated =
            self.api
                .send::<GitLabMr>("PUT", &format!("{}/{}", self.base, mr.id), &fields)?;
        self.api.required(updated)
    }

    fn set_state(&self, mr: &Mr, open: bool) -> Result<Option<Mr>> {
        let event = if open { "reopen" } else { "close" };
        self.api.send::<GitLabMr>(
            "PUT",
            &format!("{}/{}", self.base, mr.id),
            &[("state_event", Field::Text(event))],
        )
    }

    fn upload(&self, path: &str) -> Result<Option<String>> {
        let endpoint = format!("{}/uploads", self.prefix);
        let form = format!("file=@{path}");
        let p = run(
            &self.api.full(&["-X", "POST", &endpoint, "--form", &form]),
            false,
        )?;
        if p.ok {
            let url = serde_json::from_str::<Upload>(&p.stdout)
                .ok()
                .map(|u| u.url);
            if url.is_some() {
                return Ok(url);
            }
        }
        let reason = match p.stderr.trim() {
            "" => "invalid response",
            stderr => stderr,
        };
        eprintln!("  {path}: upload failed: {reason}; link left as is");
        Ok(None)
    }
}
