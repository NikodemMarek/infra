use serde::Deserialize;
use serde_json::{json, Value};

use super::{partial, Review, Side};
use crate::api::Api;
use crate::cmd::Result;
use crate::pr::github::view_path;
use crate::target::Target;

#[derive(Deserialize)]
struct Created {
    node_id: String,
}

pub fn submit(
    api: &Api,
    target: &Target,
    number: u64,
    commit: Option<&str>,
    review: &Review,
) -> Result<()> {
    let total = review.total();
    let mut problems = Vec::new();
    let mut threads = Vec::new();
    if !review.replies.is_empty() {
        let (owner, name) = target
            .project
            .split_once('/')
            .ok_or_else(|| format!("'{}' is not an owner/repo project", target.project))?;
        let response = graphql(
            api,
            json!({
                "operationName": "ReviewThreads",
                "query": "query ReviewThreads($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100){nodes{id comments(first:1){nodes{fullDatabaseId}}}}}}}",
                "variables": { "owner": owner, "name": name, "number": number },
            }),
        )?;
        let nodes = response
            .pointer("/data/repository/pullRequest/reviewThreads/nodes")
            .and_then(Value::as_array);
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
    }
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }

    let comments: Vec<Value> = review
        .comments
        .iter()
        .map(|c| {
            json!({
                "path": c.path,
                "line": c.line,
                "side": match c.side {
                    Side::New => "RIGHT",
                    Side::Old => "LEFT",
                },
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
    for (i, (reply, thread)) in review.replies.iter().zip(&threads).enumerate() {
        graphql(
            api,
            json!({
                "operationName": "AddReply",
                "query": "mutation AddReply($review:ID!,$thread:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewId:$review,pullRequestReviewThreadId:$thread,body:$body}){comment{id}}}",
                "variables": { "review": created.node_id, "thread": thread, "body": reply.body },
            }),
        )
        .map_err(|e| {
            format!(
                "{e} (pending review created with {} of {total} drafts; publish or delete it in the web UI)",
                done + i
            )
        })?;
    }
    Ok(())
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
