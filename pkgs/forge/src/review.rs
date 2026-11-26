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

#[derive(Default, Serialize)]
pub struct Draft {
    id: String,
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    side: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread: Option<String>,
}

#[derive(Serialize)]
pub struct Drafts {
    digest: String,
    drafts: Vec<Draft>,
}

impl Drafts {
    fn new(mut drafts: Vec<Draft>) -> Drafts {
        drafts.sort_by(|a, b| a.id.cmp(&b.id));
        let list = serde_json::to_string(&drafts).expect("drafts serialize to JSON");
        Drafts {
            digest: fnv1a(list.as_bytes()),
            drafts,
        }
    }

    /// `on` is the request as the user writes it (`!5`, `#5`).
    fn check(&self, read: Option<&str>, on: &str) -> Result<()> {
        if self.drafts.is_empty() || read == Some(&self.digest) {
            return Ok(());
        }
        Err(format!(
            "you have {} drafts on {on} that you have not read or that changed since: read them with `forge pr drafts` and pass --drafts DIGEST",
            self.drafts.len()
        ))
    }
}

// Change detection, not security: collisions only matter if someone crafts them.
fn fnv1a(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

#[derive(Serialize)]
pub struct Submitted {
    url: String,
    drafts: usize,
}

pub fn drafts(target: &Target, number: u64) -> Result<Drafts> {
    let api = Api::connect(target)?;
    let drafts = match target.kind {
        Kind::GitLab => gitlab::drafts(&api, target, number)?,
        Kind::GitHub => github::drafts(&api, target, number)?,
    };
    Ok(Drafts::new(drafts))
}

pub fn submit(
    target: &Target,
    number: u64,
    commit: Option<&str>,
    read: Option<&str>,
    input: &str,
) -> Result<Submitted> {
    let review = validate(input, target.kind)?;
    let api = Api::connect(target)?;
    let (host, project) = (&target.host, &target.project);
    let url = match target.kind {
        Kind::GitLab => {
            gitlab::submit(&api, target, number, commit, read, &review)?;
            format!("https://{host}/{project}/-/merge_requests/{number}")
        }
        Kind::GitHub => {
            github::submit(&api, target, number, commit, read, &review)?;
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
