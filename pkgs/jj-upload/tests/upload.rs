mod common;

use common::{stack_block, Change, Env, Forge};
use serde_json::{json, Value};

fn target(v: &Value) -> &str {
    v.get("target_branch")
        .or_else(|| v.pointer("/base/ref"))
        .and_then(Value::as_str)
        .expect("target")
}

fn desc(v: &Value) -> &str {
    v.get("description")
        .or_else(|| v.get("body"))
        .and_then(Value::as_str)
        .expect("description")
}

fn stack(forge: Forge) -> (Env, Change, Change) {
    let (e, a, b) = Env::with_stack(forge);
    assert_eq!(e.mr_count(), 0);
    (e, a, b)
}

fn refs(forge: Forge) -> [&'static str; 2] {
    match forge {
        Forge::GitLab => ["!1", "!2"],
        Forge::GitHub => ["#1", "#2"],
    }
}

#[test]
fn gitlab_stack_creates_one_mr_per_change() {
    let (e, a, b) = stack(Forge::GitLab);
    let a_commit = e.commit_of(&a.change);
    let b_commit = e.commit_of(&b.change);

    let out = e.upload_with(
        &[],
        &[("CLAUDE_CODE_SESSION_ID", "sid"), ("AGENT_SESSION", "s-1")],
    );
    out.ok();

    assert_eq!(e.remote_branch(&a.branch), Some(a_commit));
    assert_eq!(e.remote_branch(&b.branch), Some(b_commit));
    assert_eq!(e.mr_count(), 2);

    let r = refs(Forge::GitLab);
    let mr1 = e.mr(1);
    assert_eq!(mr1["source_branch"], a.branch.as_str());
    assert_eq!(target(&mr1), "main");
    assert_eq!(mr1["title"], "feat: a");
    assert_eq!(
        desc(&mr1),
        format!("Body A\n\n{}", stack_block(&r, "!1", Forge::GitLab))
    );
    let mr2 = e.mr(2);
    assert_eq!(mr2["source_branch"], b.branch.as_str());
    assert_eq!(target(&mr2), a.branch);
    assert_eq!(mr2["title"], "feat: b");
    assert_eq!(desc(&mr2), stack_block(&r, "!2", Forge::GitLab));

    let creates: Vec<_> = e
        .calls()
        .into_iter()
        .filter(|c| c.iter().any(|a| a == "POST"))
        .collect();
    assert_eq!(creates.len(), 2);
    for c in &creates {
        assert!(
            c.iter().any(|a| a == "remove_source_branch=true"),
            "missing remove_source_branch=true: {c:?}"
        );
    }

    for (branch, change) in [(&a.branch, &a), (&b.branch, &b)] {
        let name = format!("{}.json", branch.replace('/', "%2F"));
        let p = e.state_dir().join("jj-upload/repo").join(&name);
        let rec: Value = serde_json::from_str(
            &std::fs::read_to_string(&p).unwrap_or_else(|err| panic!("{}: {err}", p.display())),
        )
        .unwrap();
        assert_eq!(rec["session_id"], "sid", "{}", change.change);
        assert_eq!(
            rec["workspace"],
            std::fs::canonicalize(&e.repo)
                .unwrap()
                .to_string_lossy()
                .as_ref()
        );
        assert!(rec["uploaded_at"].is_string() || rec["uploaded_at"].is_number());
    }

    let jw = e.jw_calls();
    for n in [1, 2] {
        let url = e.mr(n)["web_url"].as_str().unwrap().to_string();
        let hits = jw
            .iter()
            .filter(|c| {
                c.len() >= 2
                    && c[0] == "session"
                    && c[1] == "set"
                    && c.windows(2).any(|w| w[0] == "--mr" && w[1] == url)
            })
            .count();
        assert_eq!(hits, 1, "jw session set --mr {url}: {jw:?}");
    }
}

#[test]
fn reupload_updates_existing_mrs() {
    let (e, a, b) = stack(Forge::GitLab);
    e.upload(&[]).ok();
    let r = refs(Forge::GitLab);
    let block = stack_block(&r, "!1", Forge::GitLab);

    let mut mr1 = e.mr(1);
    mr1["title"] = json!("feat: a edited");
    mr1["description"] = json!(format!("Edited body\n\n{block}"));
    e.set_mr(1, &mr1);

    e.write("b.txt", "changed\n");
    e.jj(&["squash", "--into", &b.change]);
    let calls_before = e.calls().len();

    e.upload(&[]).ok();

    let new_calls = &e.calls()[calls_before..];
    assert!(
        !new_calls.iter().any(|c| c.iter().any(|a| a == "POST")),
        "unexpected create: {new_calls:?}"
    );
    assert_eq!(e.mr_count(), 2);
    assert_eq!(e.remote_branch(&b.branch), Some(e.commit_of(&b.change)));
    assert_eq!(e.remote_branch(&a.branch), Some(e.commit_of(&a.change)));
    assert_eq!(
        e.description_of(&a.change).trim(),
        "feat: a edited\n\nEdited body"
    );
    assert_eq!(e.mr(1)["source_branch"], a.branch.as_str());
    assert_eq!(e.mr(2)["source_branch"], b.branch.as_str());
    assert_eq!(desc(&e.mr(1)), format!("Edited body\n\n{block}"));
}

#[test]
fn github_stack_creates_one_pr_per_change() {
    let (e, a, b) = stack(Forge::GitHub);
    let a_commit = e.commit_of(&a.change);
    let b_commit = e.commit_of(&b.change);

    e.upload(&[]).ok();

    assert_eq!(e.remote_branch(&a.branch), Some(a_commit));
    assert_eq!(e.remote_branch(&b.branch), Some(b_commit));
    assert_eq!(e.mr_count(), 2);

    let r = refs(Forge::GitHub);
    let pr1 = e.mr(1);
    assert_eq!(pr1["head"]["ref"], a.branch.as_str());
    assert_eq!(target(&pr1), "main");
    assert_eq!(pr1["title"], "feat: a");
    assert_eq!(pr1["draft"], json!(false));
    assert_eq!(
        desc(&pr1),
        format!("Body A\n\n{}", stack_block(&r, "#1", Forge::GitHub))
    );
    let pr2 = e.mr(2);
    assert_eq!(pr2["head"]["ref"], b.branch.as_str());
    assert_eq!(target(&pr2), a.branch);
    assert_eq!(pr2["title"], "feat: b");
    assert_eq!(pr2["draft"], json!(false));
    assert_eq!(desc(&pr2), stack_block(&r, "#2", Forge::GitHub));
}

#[test]
fn upload_rebases_stack_onto_new_trunk() {
    let (e, a, b) = stack(Forge::GitLab);
    e.upload(&[]).ok();
    let new_main = e.advance_remote_main();

    e.upload(&[]).ok();

    assert_eq!(e.parent_commit_of(&a.change).trim(), new_main);
    assert_eq!(e.remote_branch(&a.branch), Some(e.commit_of(&a.change)));
    assert_eq!(e.remote_branch(&b.branch), Some(e.commit_of(&b.change)));
    assert_ne!(e.remote_branch(&a.branch).unwrap(), e.base);
    assert_eq!(e.mr_count(), 2);
}

#[test]
fn merged_bottom_retargets_rest() {
    let (e, a, b) = stack(Forge::GitLab);
    e.upload(&[]).ok();
    let a_sha = e.remote_branch(&a.branch).unwrap();
    e.set_remote_main(&a_sha);
    let mut mr1 = e.mr(1);
    mr1["state"] = json!("merged");
    mr1["sha"] = json!(a_sha);
    e.set_mr(1, &mr1);

    e.upload(&[]).ok();

    assert_eq!(e.mr_count(), 2);
    let mr2 = e.mr(2);
    assert_eq!(mr2["source_branch"], b.branch.as_str());
    assert_eq!(target(&mr2), "main");
    assert!(!desc(&mr2).contains("jj-stack"), "{}", desc(&mr2));
}

#[test]
fn abandoned_change_closes_mr_and_deletes_branch() {
    let (e, a, b) = stack(Forge::GitLab);
    e.upload(&[]).ok();
    assert!(e.remote_branch(&b.branch).is_some());
    e.jj(&["abandon", &b.change]);

    e.upload(&[]).ok();

    assert_eq!(e.mr(2)["state"], "closed");
    assert!(
        e.calls()
            .iter()
            .any(|c| c.iter().any(|a| a == "state_event=close")),
        "no close call: {:?}",
        e.calls()
    );
    assert_eq!(e.remote_branch(&b.branch), None);
    assert!(e.remote_branch(&a.branch).is_some());
    assert_eq!(desc(&e.mr(1)), "Body A");
}

#[test]
fn stack_status_lines() {
    let (e, a, b) = stack(Forge::GitLab);
    e.upload(&[]).ok();
    let c = e.add_change("c.txt", "feat: c");

    let out = e.upload(&["stack", "status"]);
    let url1 = e.mr(1)["web_url"].as_str().unwrap().to_string();
    let url2 = e.mr(2)["web_url"].as_str().unwrap().to_string();
    assert_eq!(
        out.stdout,
        format!(
            "{}\topen\t{url1}\tfeat: a\n{}\topen\t{url2}\tfeat: b\n{c}\tno-mr\t-\tfeat: c\nnot merged\n",
            a.change, b.change
        ),
        "stderr: {}",
        out.stderr
    );
    assert_eq!(out.code, 1);

    let empty = Env::new(Forge::GitLab);
    let out = empty.upload(&["stack", "status"]);
    assert_eq!(out.stdout, "merged\n", "stderr: {}", out.stderr);
    assert_eq!(out.code, 0);
}

#[test]
fn dry_run_writes_nothing() {
    let (e, a, b) = stack(Forge::GitLab);

    let out = e.upload(&["-n"]);

    out.ok();
    for want in [
        "dry run: not syncing",
        "Uploading 2 change(s) to origin, bottom MR targets main:",
        a.branch.as_str(),
        b.branch.as_str(),
    ] {
        assert!(
            out.stderr.contains(want),
            "stderr lacks {want:?}:\n{}",
            out.stderr
        );
    }
    assert!(e.remote_branches("nm/").is_empty());
    assert!(
        !e.calls().iter().any(|c| Env::is_mutating(c)),
        "mutating call: {:?}",
        e.calls()
    );
    assert_eq!(e.mr_count(), 0);
}
