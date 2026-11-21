use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::api::Api;
use crate::cmd::Result;
use crate::target::{Kind, Target};

mod github;
mod gitlab;

#[derive(Serialize)]
pub struct Pr {
    forge: Kind,
    number: u64,
    #[serde(rename = "ref")]
    reference: String,
    url: String,
    state: String,
    draft: bool,
    title: String,
    body: String,
    base: String,
    head: String,
    head_sha: Option<String>,
    landed: Vec<String>,
}

pub fn list(target: &Target, head: Option<&str>, limit: u8) -> Result<Vec<Pr>> {
    let api = Api::connect(target)?;
    let project = &target.project;
    Ok(match target.kind {
        Kind::GitLab => api
            .get::<Vec<gitlab::Mr>>(&gitlab::list_path(project, head, limit))?
            .into_iter()
            .map(Pr::from)
            .collect(),
        Kind::GitHub => api
            .get::<Vec<github::Pull>>(&github::list_path(project, head, limit))?
            .into_iter()
            .map(Pr::from)
            .collect(),
    })
}

pub fn create(
    target: &Target,
    head: &str,
    base: &str,
    title: &str,
    body: Option<&str>,
    draft: bool,
) -> Result<Pr> {
    let api = Api::connect(target)?;
    let project = &target.project;
    let body = body.unwrap_or_default();
    Ok(match target.kind {
        Kind::GitLab => {
            let title = if draft {
                format!("Draft: {title}")
            } else {
                title.to_string()
            };
            api.send::<gitlab::Mr>(
                "POST",
                &gitlab::create_path(project),
                &json!({
                    "source_branch": head,
                    "target_branch": base,
                    "title": title,
                    "description": body,
                    "remove_source_branch": true,
                }),
            )?
            .into()
        }
        Kind::GitHub => api
            .send::<github::Pull>(
                "POST",
                &github::create_path(project),
                &json!({
                    "head": head,
                    "base": base,
                    "title": title,
                    "body": body,
                    "draft": draft,
                }),
            )?
            .into(),
    })
}

pub fn edit(
    target: &Target,
    number: u64,
    title: Option<&str>,
    base: Option<&str>,
    body: Option<&str>,
) -> Result<Pr> {
    if title.is_none() && base.is_none() && body.is_none() {
        return view(target, number);
    }
    let api = Api::connect(target)?;
    let project = &target.project;
    Ok(match target.kind {
        Kind::GitLab => {
            let path = gitlab::view_path(project, number);
            let title = match title {
                Some(t) => Some(if api.get::<gitlab::Mr>(&path)?.is_draft() {
                    format!("Draft: {}", gitlab::strip_draft(t))
                } else {
                    t.to_string()
                }),
                None => None,
            };
            let fields = given(&[
                ("title", title.as_deref()),
                ("target_branch", base),
                ("description", body),
            ]);
            api.send::<gitlab::Mr>("PUT", &path, &fields)?.into()
        }
        Kind::GitHub => {
            let fields = given(&[("title", title), ("base", base), ("body", body)]);
            api.send::<github::Pull>("PATCH", &github::view_path(project, number), &fields)?
                .into()
        }
    })
}

pub fn close(target: &Target, number: u64) -> Result<Pr> {
    set_state(target, number, false)
}

pub fn reopen(target: &Target, number: u64) -> Result<Pr> {
    set_state(target, number, true)
}

fn set_state(target: &Target, number: u64, open: bool) -> Result<Pr> {
    let api = Api::connect(target)?;
    let project = &target.project;
    Ok(match target.kind {
        Kind::GitLab => api
            .send::<gitlab::Mr>(
                "PUT",
                &gitlab::view_path(project, number),
                &json!({ "state_event": if open { "reopen" } else { "close" } }),
            )?
            .into(),
        Kind::GitHub => api
            .send::<github::Pull>(
                "PATCH",
                &github::view_path(project, number),
                &json!({ "state": if open { "open" } else { "closed" } }),
            )?
            .into(),
    })
}

fn given(fields: &[(&str, Option<&str>)]) -> Value {
    fields
        .iter()
        .filter_map(|(key, value)| value.map(|v| (key.to_string(), Value::from(v))))
        .collect::<Map<_, _>>()
        .into()
}

pub fn view(target: &Target, number: u64) -> Result<Pr> {
    let api = Api::connect(target)?;
    let project = &target.project;
    Ok(match target.kind {
        Kind::GitLab => api
            .get::<gitlab::Mr>(&gitlab::view_path(project, number))?
            .into(),
        Kind::GitHub => api
            .get::<github::Pull>(&github::view_path(project, number))?
            .into(),
    })
}
