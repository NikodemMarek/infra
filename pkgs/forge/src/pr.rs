use serde::Serialize;

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
