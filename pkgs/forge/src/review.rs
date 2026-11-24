use serde::{Deserialize, Serialize};

use crate::api::Api;
use crate::cmd::Result;
use crate::target::{Kind, Target};

mod github;
mod gitlab;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    summary: Option<String>,
    #[serde(default)]
    comments: Vec<Comment>,
    #[serde(default)]
    replies: Vec<Reply>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Comment {
    path: String,
    line: i64,
    #[serde(default)]
    side: Side,
    body: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    thread: String,
    body: String,
    #[serde(default)]
    resolve: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Side {
    #[default]
    New,
    Old,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::New => "new",
            Side::Old => "old",
        }
    }
}

impl Review {
    fn total(&self) -> usize {
        usize::from(self.summary.is_some()) + self.comments.len() + self.replies.len()
    }
}

#[derive(Serialize)]
pub struct Submitted {
    url: String,
    drafts: usize,
}

pub fn submit(
    target: &Target,
    number: u64,
    commit: Option<&str>,
    input: &str,
) -> Result<Submitted> {
    let review = validate(input, target.kind)?;
    let api = Api::connect(target)?;
    let (host, project) = (&target.host, &target.project);
    let url = match target.kind {
        Kind::GitLab => {
            gitlab::submit(&api, target, number, commit, &review)?;
            format!("https://{host}/{project}/-/merge_requests/{number}")
        }
        Kind::GitHub => {
            github::submit(&api, target, number, commit, &review)?;
            format!("https://{host}/{project}/pull/{number}")
        }
    };
    Ok(Submitted {
        url,
        drafts: review.total(),
    })
}

fn validate(input: &str, kind: Kind) -> Result<Review> {
    let review: Review =
        serde_json::from_str(input).map_err(|e| format!("invalid review JSON: {e}"))?;
    let mut problems = Vec::new();
    if review.total() == 0 {
        problems.push("nothing to submit".to_string());
    }
    if review.summary.as_deref().is_some_and(blank) {
        problems.push("summary: empty".to_string());
    }
    for (i, c) in review.comments.iter().enumerate() {
        if blank(&c.body) {
            problems.push(format!("comments[{i}]: empty body"));
        }
        if blank(&c.path) {
            problems.push(format!("comments[{i}]: empty path"));
        }
        if c.line < 1 {
            problems.push(format!("comments[{i}]: line must be >= 1"));
        }
    }
    for (i, r) in review.replies.iter().enumerate() {
        if blank(&r.body) {
            problems.push(format!("replies[{i}]: empty body"));
        }
        if blank(&r.thread) {
            problems.push(format!("replies[{i}]: empty thread"));
        }
        if r.resolve && matches!(kind, Kind::GitHub) {
            problems.push(format!("replies[{i}]: resolve is not supported on GitHub"));
        }
    }
    if problems.is_empty() {
        Ok(review)
    } else {
        Err(problems.join("\n"))
    }
}

fn blank(s: &str) -> bool {
    s.trim().is_empty()
}

fn partial(error: String, created: usize, total: usize) -> String {
    format!("{error} ({created} of {total} drafts created; publish or delete them in the web UI)")
}
