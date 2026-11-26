use serde::Deserialize;
use serde_json::{json, Value};

use super::{blank, partial, Comment, Review, Side};
use crate::api::Api;
use crate::cmd::Result;
use crate::pr::github::view_path;
use crate::target::Target;

#[derive(Deserialize)]
struct Created {
    node_id: String,
}

struct Pending {
    id: String,
    body: String,
    commit: String,
}

pub fn submit(
    api: &Api,
    target: &Target,
    number: u64,
    commit: Option<&str>,
    review: &Review,
) -> Result<()> {
    let lookup = lookup(api, target, number)?;
    let threads = reply_threads(&lookup, review, number)?;
    let head = lookup
        .pointer("/data/repository/pullRequest/headRefOid")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match pending_review(&lookup) {
        Some(pending) => append(
            api,
            number,
            commit.unwrap_or(head),
            review,
            &pending,
            &threads,
        ),
        None => create(api, target, number, commit, review, &threads),
    }
}

fn lookup(api: &Api, target: &Target, number: u64) -> Result<Value> {
    let (owner, name) = target
        .project
        .split_once('/')
        .ok_or_else(|| format!("'{}' is not an owner/repo project", target.project))?;
    graphql(
        api,
        json!({
            "operationName": "Review",
            "query": "query Review($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){headRefOid reviews(states:PENDING,first:10){nodes{id body viewerDidAuthor commit{oid}}} reviewThreads(first:100){nodes{id comments(first:1){nodes{fullDatabaseId}}}}}}}",
            "variables": { "owner": owner, "name": name, "number": number },
        }),
    )
}

fn pending_review(lookup: &Value) -> Option<Pending> {
    let node = lookup
        .pointer("/data/repository/pullRequest/reviews/nodes")?
        .as_array()?
        .iter()
        .find(|n| n["viewerDidAuthor"] == true)?;
    let text = |pointer: &str| node.pointer(pointer).and_then(Value::as_str);
    Some(Pending {
        id: text("/id")?.to_string(),
        body: text("/body").unwrap_or_default().to_string(),
        commit: text("/commit/oid").unwrap_or_default().to_string(),
    })
}

fn reply_threads(lookup: &Value, review: &Review, number: u64) -> Result<Vec<String>> {
    let nodes = lookup
        .pointer("/data/repository/pullRequest/reviewThreads/nodes")
        .and_then(Value::as_array);
    let mut problems = Vec::new();
    let mut threads = Vec::new();
    for (i, r) in review.replies.iter().enumerate() {
        let id = nodes.into_iter().flatten().find_map(|n| {
            let first = match n.pointer("/comments/nodes/0/fullDatabaseId")? {
                Value::String(s) => s.clone(),
                Value::Number(num) => num.to_string(),
                _ => return None,
            };
            (first == r.thread).then(|| n["id"].as_str())?
        });
        match id {
            Some(id) => threads.push(id.to_string()),
            None => problems.push(format!(
                "replies[{i}]: thread {} not found on #{number}",
                r.thread
            )),
        }
    }
    if problems.is_empty() {
        Ok(threads)
    } else {
        Err(problems.join("\n"))
    }
}

fn create(
    api: &Api,
    target: &Target,
    number: u64,
    commit: Option<&str>,
    review: &Review,
    threads: &[String],
) -> Result<()> {
    let total = review.total();
    let comments: Vec<Value> = review
        .comments
        .iter()
        .map(|c| {
            json!({
                "path": c.path,
                "line": c.line,
                "side": side_name(c.side),
                "body": c.body,
            })
        })
        .collect();
    let mut body = json!({ "comments": comments });
    if let Some(sha) = commit {
        body["commit_id"] = sha.into();
    }
    if let Some(summary) = &review.summary {
        body["body"] = summary.as_str().into();
    }
    let path = format!("{}/reviews", view_path(&target.project, number));
    let created: Created = api
        .send("POST", &path, &body)
        .map_err(|e| partial(e, 0, total))?;

    let done = total - review.replies.len();
    for (i, (reply, thread)) in review.replies.iter().zip(threads).enumerate() {
        graphql(api, add_reply(&created.node_id, thread, &reply.body)).map_err(|e| {
            format!(
                "{e} (pending review created with {} of {total} drafts; publish or delete it in the web UI)",
                done + i
            )
        })?;
    }
    Ok(())
}

fn append(
    api: &Api,
    number: u64,
    target_commit: &str,
    review: &Review,
    pending: &Pending,
    threads: &[String],
) -> Result<()> {
    if pending.commit != target_commit {
        return Err(format!(
            "you have a pending review on #{number} at commit {}: publish or delete it to review {target_commit}",
            pending.commit
        ));
    }
    let writes: Vec<Value> = review
        .summary
        .iter()
        .map(|summary| update_review(pending, summary))
        .chain(review.comments.iter().map(|c| add_thread(&pending.id, c)))
        .chain(
            review
                .replies
                .iter()
                .zip(threads)
                .map(|(r, thread)| add_reply(&pending.id, thread, &r.body)),
        )
        .collect();
    for (done, write) in writes.into_iter().enumerate() {
        graphql(api, write).map_err(|e| partial(e, done, review.total()))?;
    }
    Ok(())
}

fn update_review(pending: &Pending, summary: &str) -> Value {
    let body = if blank(&pending.body) {
        summary.to_string()
    } else {
        format!("{}\n\n{summary}", pending.body)
    };
    json!({
        "operationName": "UpdateReview",
        "query": "mutation UpdateReview($review:ID!,$body:String!){updatePullRequestReview(input:{pullRequestReviewId:$review,body:$body}){pullRequestReview{id}}}",
        "variables": { "review": pending.id, "body": body },
    })
}

fn add_thread(review: &str, comment: &Comment) -> Value {
    json!({
        "operationName": "AddThread",
        "query": "mutation AddThread($review:ID!,$path:String!,$line:Int!,$side:DiffSide!,$body:String!){addPullRequestReviewThread(input:{pullRequestReviewId:$review,path:$path,line:$line,side:$side,body:$body}){thread{id}}}",
        "variables": {
            "review": review,
            "path": comment.path,
            "line": comment.line,
            "side": side_name(comment.side),
            "body": comment.body,
        },
    })
}

fn add_reply(review: &str, thread: &str, body: &str) -> Value {
    json!({
        "operationName": "AddReply",
        "query": "mutation AddReply($review:ID!,$thread:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewId:$review,pullRequestReviewThreadId:$thread,body:$body}){comment{id}}}",
        "variables": { "review": review, "thread": thread, "body": body },
    })
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::New => "RIGHT",
        Side::Old => "LEFT",
    }
}

/// GraphQL errors can come back with exit 0, so they are read from the response.
fn graphql(api: &Api, body: Value) -> Result<Value> {
    let response: Value = api.send("POST", "graphql", &body)?;
    if let Some(errors) = response["errors"].as_array().filter(|e| !e.is_empty()) {
        return Err(errors
            .iter()
            .map(|e| e["message"].as_str().unwrap_or("unknown GraphQL error"))
            .collect::<Vec<_>>()
            .join("; "));
    }
    Ok(response)
}
