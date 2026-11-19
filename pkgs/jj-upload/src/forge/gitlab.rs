use serde::Deserialize;

use super::{quote, quote_plus, Api, Forge, Mr};
use crate::cmd::Result;
use crate::jj::re;

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

struct Glab {
    api: Api,
}

impl Glab {
    fn new(host: String) -> Glab {
        Glab {
            api: Api { bin: "glab", host },
        }
    }

    fn find_mrs(&self, project: &str, branch: &str) -> Result<Vec<GitLabMr>> {
        let branch = quote_plus(branch);
        self.api.many(&[&format!(
            "projects/{project}/merge_requests?source_branch={branch}&order_by=created_at&sort=desc"
        )])
    }

    fn recent_mrs(&self, project: &str) -> Result<Vec<GitLabMr>> {
        self.api.many(&[
            "-X",
            "GET",
            &format!(
                "projects/{project}/merge_requests?state=all&order_by=updated_at&sort=desc&per_page=100"
            ),
        ])
    }

    fn create_mr(
        &self,
        project: &str,
        source: &str,
        target: &str,
        title: &str,
        description: &str,
    ) -> Result<GitLabMr> {
        self.api.required(&[
            "-X",
            "POST",
            &format!("projects/{project}/merge_requests"),
            "-f",
            &format!("source_branch={source}"),
            "-f",
            &format!("target_branch={target}"),
            "-f",
            &format!("title={title}"),
            "-f",
            &format!("description={description}"),
            "-f",
            "remove_source_branch=true",
        ])
    }

    fn update_mr(
        &self,
        project: &str,
        iid: &str,
        title: Option<&str>,
        target: Option<&str>,
        description: Option<&str>,
    ) -> Result<GitLabMr> {
        let url = format!("projects/{project}/merge_requests/{iid}");
        let fields: Vec<String> = [
            title.map(|v| format!("title={v}")),
            target.map(|v| format!("target_branch={v}")),
            description.map(|v| format!("description={v}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut args = vec!["-X", "PUT", url.as_str()];
        for field in &fields {
            args.extend(["-f", field.as_str()]);
        }
        self.api.required(&args)
    }

    fn set_state(&self, project: &str, iid: &str, open: bool) -> Result<Option<GitLabMr>> {
        let event = if open { "reopen" } else { "close" };
        self.api.one(&[
            "-X",
            "PUT",
            &format!("projects/{project}/merge_requests/{iid}"),
            "-f",
            &format!("state_event={event}"),
        ])
    }

    fn upload(&self, project: &str, path: &str) -> Result<std::result::Result<String, String>> {
        let p = self.api.run(
            &[
                "-X",
                "POST",
                &format!("projects/{project}/uploads"),
                "--form",
                &format!("file=@{path}"),
            ],
            false,
        )?;
        let upload =
            p.ok.then(|| serde_json::from_str::<Upload>(&p.stdout).ok())
                .flatten();
        let reason = match p.stderr.trim() {
            "" => "invalid response",
            stderr => stderr,
        };
        Ok(upload.map(|u| u.url).ok_or_else(|| reason.to_string()))
    }
}

pub struct GitLab {
    glab: Glab,
    project: String,
    recent: Option<Vec<Mr>>,
}

impl GitLab {
    pub fn new(host: String, project: &str) -> GitLab {
        GitLab {
            glab: Glab::new(host),
            project: quote(project),
            recent: None,
        }
    }
}

impl Forge for GitLab {
    fn term(&self) -> &'static str {
        "MR"
    }

    fn find(&self, branch: &str) -> Result<Option<Mr>> {
        let mrs = self.glab.find_mrs(&self.project, branch)?;
        Ok(mrs.into_iter().next().map(Mr::from))
    }

    fn recent(&mut self) -> Result<&[Mr]> {
        if self.recent.is_none() {
            let mrs = self.glab.recent_mrs(&self.project)?;
            self.recent = Some(mrs.into_iter().map(Mr::from).collect());
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
        let mr = self
            .glab
            .create_mr(&self.project, source, target, &title, description)?;
        Ok(mr.into())
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
        if title.is_none() && target.is_none() && description.is_none() {
            return Ok(mr.clone());
        }
        let updated =
            self.glab
                .update_mr(&self.project, &mr.id, title.as_deref(), target, description)?;
        Ok(updated.into())
    }

    fn set_state(&self, mr: &Mr, open: bool) -> Result<Option<Mr>> {
        let updated = self.glab.set_state(&self.project, &mr.id, open)?;
        Ok(updated.map(Mr::from))
    }

    fn upload(&self, path: &str) -> Result<Option<String>> {
        match self.glab.upload(&self.project, path)? {
            Ok(url) => Ok(Some(url)),
            Err(reason) => {
                eprintln!("  {path}: upload failed: {reason}; link left as is");
                Ok(None)
            }
        }
    }
}
