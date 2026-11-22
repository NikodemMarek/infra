mod common;

use common::{stack_block, Change, Env, Forge};
use serde_json::{json, Value};

fn target(v: &Value) -> &str {
    v["base"].as_str().expect("base")
}

fn desc(v: &Value) -> &str {
    v["body"].as_str().expect("body")
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

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn count_pr(e: &Env, sub: &str) -> usize {
    e.calls().iter().filter(|c| Env::is_pr(c, sub)).count()
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
    assert_eq!(mr1["head"], a.branch.as_str());
    assert_eq!(target(&mr1), "main");
    assert_eq!(mr1["title"], "feat: a");
    assert_eq!(mr1["draft"], json!(false));
    assert_eq!(
        desc(&mr1),
        format!("Body A\n\n{}", stack_block(&r, "!1", Forge::GitLab))
    );
    let mr2 = e.mr(2);
    assert_eq!(mr2["head"], b.branch.as_str());
    assert_eq!(target(&mr2), a.branch);
    assert_eq!(mr2["title"], "feat: b");
    assert_eq!(desc(&mr2), stack_block(&r, "!2", Forge::GitLab));

    assert_eq!(count_pr(&e, "create"), 2);
    let calls = e.calls();
    assert!(
        calls.contains(&argv(&["--remote", "origin", "repo"])),
        "no repo call: {calls:?}"
    );
    for branch in [&a.branch, &b.branch] {
        let want = argv(&[
            "--remote", "origin", "pr", "list", "--head", branch, "--limit", "1",
        ]);
        assert!(calls.contains(&want), "no find call {want:?}: {calls:?}");
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
        let url = e.mr(n)["url"].as_str().unwrap().to_string();
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
    mr1["body"] = json!(format!("Edited body\n\n{block}"));
    e.set_mr(1, &mr1);

    e.write("b.txt", "changed\n");
    e.jj(&["squash", "--into", &b.change]);
    let calls_before = e.calls().len();

    e.upload(&[]).ok();

    let new_calls = &e.calls()[calls_before..];
    assert!(
        !new_calls.iter().any(|c| Env::is_pr(c, "create")),
        "unexpected create: {new_calls:?}"
    );
    assert_eq!(e.mr_count(), 2);
    assert_eq!(e.remote_branch(&b.branch), Some(e.commit_of(&b.change)));
    assert_eq!(e.remote_branch(&a.branch), Some(e.commit_of(&a.change)));
    assert_eq!(
        e.description_of(&a.change).trim(),
        "feat: a edited\n\nEdited body"
    );
    assert_eq!(e.mr(1)["head"], a.branch.as_str());
    assert_eq!(e.mr(2)["head"], b.branch.as_str());
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
    assert_eq!(pr1["head"], a.branch.as_str());
    assert_eq!(target(&pr1), "main");
    assert_eq!(pr1["title"], "feat: a");
    assert_eq!(pr1["draft"], json!(false));
    assert_eq!(
        desc(&pr1),
        format!("Body A\n\n{}", stack_block(&r, "#1", Forge::GitHub))
    );
    let pr2 = e.mr(2);
    assert_eq!(pr2["head"], b.branch.as_str());
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
    mr1["head_sha"] = json!(a_sha);
    mr1["landed"] = json!([a_sha]);
    e.set_mr(1, &mr1);

    e.upload(&[]).ok();

    assert_eq!(e.mr_count(), 2);
    let mr2 = e.mr(2);
    assert_eq!(mr2["head"], b.branch.as_str());
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
        e.calls().iter().any(|c| Env::is_pr(c, "close")),
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
    let url1 = e.mr(1)["url"].as_str().unwrap().to_string();
    let url2 = e.mr(2)["url"].as_str().unwrap().to_string();
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

fn closed_mr_env(forge: Forge) -> (Env, Change) {
    let (e, a, _b) = stack(forge);
    e.upload(&[]).ok();
    let mut mr1 = e.mr(1);
    mr1["state"] = json!("closed");
    e.set_mr(1, &mr1);
    (e, a)
}

#[test]
fn closed_mr_is_reopened() {
    let (e, _a) = closed_mr_env(Forge::GitLab);

    e.upload(&[]).ok();

    assert_eq!(e.mr_count(), 2);
    assert_eq!(e.mr(1)["state"], "open");
    assert!(
        e.calls().iter().any(|c| Env::is_pr(c, "reopen")),
        "no reopen call: {:?}",
        e.calls()
    );
}

#[test]
fn refused_reopen_warns_and_creates_a_new_mr() {
    let (e, a) = closed_mr_env(Forge::GitLab);
    e.fake_on("reopen_refuse");

    let out = e.upload(&[]);

    out.ok();
    assert!(
        out.stderr.to_lowercase().contains("reopen"),
        "no reopen warning:\n{}",
        out.stderr
    );
    assert_eq!(e.mr_count(), 3);
    assert_eq!(e.mr(1)["state"], "closed");
    let new = e.mr(3);
    assert_eq!(new["state"], "open");
    assert_eq!(new["head"], a.branch.as_str());
    assert_eq!(target(&new), "main");
    assert_eq!(new["title"], "feat: a");
}

fn picture_env(forge: Forge) -> Env {
    let e = Env::new(forge);
    e.write(".gitignore", "shot.png\n");
    e.write("shot.png", "png\n");
    e.add_change("p.txt", "feat: pic\n\n![shot](shot.png)");
    e
}

#[test]
fn ignored_image_is_uploaded_and_link_rewritten() {
    let e = picture_env(Forge::GitLab);

    e.upload(&[]).ok();

    assert_eq!(e.mr_count(), 1);
    let body = desc(&e.mr(1)).to_string();
    assert!(
        body.contains("![shot](/uploads/") && body.contains("shot.png)"),
        "link not rewritten: {body}"
    );
    assert!(
        e.calls()
            .iter()
            .any(|c| matches!(Env::args(c), [u, ..] if u == "upload")),
        "no upload call: {:?}",
        e.calls()
    );
}

#[test]
fn failed_image_upload_leaves_link_and_warns() {
    let e = picture_env(Forge::GitLab);
    e.fake_on("upload_fail");

    let out = e.upload(&[]);

    out.ok();
    assert_eq!(e.mr_count(), 1);
    assert!(
        desc(&e.mr(1)).contains("![shot](shot.png)"),
        "link changed: {}",
        desc(&e.mr(1))
    );
    assert!(
        out.stderr.contains("shot.png"),
        "no warning naming the file:\n{}",
        out.stderr
    );
}

fn pr_create_calls(e: &Env) -> Vec<Vec<String>> {
    e.calls()
        .into_iter()
        .filter(|c| Env::is_pr(c, "create"))
        .collect()
}

#[test]
fn draft_flag_creates_draft_mrs() {
    let (e, _a, _b) = stack(Forge::GitLab);

    e.upload(&["--draft"]).ok();

    assert_eq!(e.mr_count(), 2);
    assert_eq!(e.mr(1)["draft"], json!(true));
    assert_eq!(e.mr(2)["draft"], json!(true));
    let creates = pr_create_calls(&e);
    assert_eq!(creates.len(), 2);
    assert!(creates.iter().all(|c| c.iter().any(|a| a == "--draft")));
}

#[test]
fn draft_config_creates_draft_mrs() {
    let (e, _a, _b) = stack(Forge::GitLab);
    e.jj(&["config", "set", "--repo", "upload.draft", "true"]);

    e.upload(&[]).ok();

    assert_eq!(e.mr(1)["draft"], json!(true));
    assert!(pr_create_calls(&e)
        .iter()
        .all(|c| c.iter().any(|a| a == "--draft")));
}

#[test]
fn no_draft_flag_by_default() {
    let (e, _a, _b) = stack(Forge::GitLab);

    e.upload(&[]).ok();

    assert!(!pr_create_calls(&e).is_empty());
    assert!(pr_create_calls(&e)
        .iter()
        .all(|c| !c.iter().any(|a| a == "--draft")));
}

#[test]
fn github_image_link_warns_and_is_left_alone() {
    let e = picture_env(Forge::GitHub);

    let out = e.upload(&[]);

    out.ok();
    assert_eq!(e.mr_count(), 1);
    assert!(
        desc(&e.mr(1)).contains("![shot](shot.png)"),
        "link changed: {}",
        desc(&e.mr(1))
    );
    assert!(
        out.stderr.contains("shot.png") && out.stderr.contains("github has no file upload API"),
        "no warning:\n{}",
        out.stderr
    );
}

#[test]
fn status_finds_mr_through_recent_list_without_local_bookmark() {
    let (e, a, _b) = stack(Forge::GitLab);
    e.upload(&[]).ok();
    e.jj(&["bookmark", "forget", "--include-remotes", &a.branch]);
    let before = e.calls().len();

    let out = e.upload(&["stack", "status"]);

    let url1 = e.mr(1)["url"].as_str().unwrap().to_string();
    assert!(
        out.stdout
            .contains(&format!("{}\topen\t{url1}\t", a.change)),
        "stdout: {}\nstderr: {}",
        out.stdout,
        out.stderr
    );
    let calls = e.calls();
    assert!(
        calls[before..].iter().any(|c| matches!(
            Env::args(c),
            [pr, list] if pr == "pr" && list == "list"
        )),
        "no `pr list` without --head: {:?}",
        &calls[before..]
    );
}

// Commit on the remote branch behind jj's back (a forge "apply suggestion"):
// `file` is set to `content` in the tree of `parent`. Returns the new id.
fn remote_commit(e: &Env, branch: &str, parent: &str, file: &str, content: &str) -> String {
    let git = |args: &[&str], stdin: Option<&str>, index: &std::path::Path| {
        use std::io::Write;
        use std::process::Stdio;
        let mut c = std::process::Command::new("git");
        c.arg("-C")
            .arg(&e.remote)
            .args(args)
            .env("GIT_INDEX_FILE", index)
            .env("GIT_AUTHOR_NAME", "Other")
            .env("GIT_AUTHOR_EMAIL", "other@example.com")
            .env("GIT_COMMITTER_NAME", "Other")
            .env("GIT_COMMITTER_EMAIL", "other@example.com")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.unwrap_or("").as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        assert!(
            o.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let index = e.dir.join("remote-index");
    git(&["read-tree", parent], None, &index);
    let blob = git(&["hash-object", "-w", "--stdin"], Some(content), &index);
    git(
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{blob},{file}"),
        ],
        None,
        &index,
    );
    let tree = git(&["write-tree"], None, &index);
    let sha = git(
        &["commit-tree", &tree, "-p", parent, "-m", "remote edit"],
        None,
        &index,
    );
    git(
        &["update-ref", &format!("refs/heads/{branch}"), &sha],
        None,
        &index,
    );
    sha
}

fn file_at(e: &Env, rev: &str, file: &str) -> String {
    e.jj(&["file", "show", "-r", rev, file])
}

fn stack_changes(e: &Env) -> String {
    e.jj(&[
        "log",
        "--no-graph",
        "-r",
        "trunk()..@",
        "-T",
        "change_id.short(8) ++ \"\\n\"",
    ])
}

#[test]
fn sync_squashes_remote_only_commit_into_unchanged_change() {
    let (e, a, b) = stack(Forge::GitLab);
    e.upload(&[]).ok();
    let pushed = e.remote_branch(&a.branch).unwrap();
    let changes = stack_changes(&e);
    remote_commit(&e, &a.branch, &pushed, "a.txt", "remote edit\n");

    let out = e.upload(&["sync"]);

    out.ok();
    assert!(
        out.stderr
            .contains(&format!("pulled remote commits of {}", a.branch)),
        "stderr: {}",
        out.stderr
    );
    assert_eq!(file_at(&e, &a.change, "a.txt"), "remote edit\n");
    assert_eq!(file_at(&e, &b.change, "a.txt"), "remote edit\n");
    assert_eq!(stack_changes(&e), changes);
    assert_eq!(
        e.jj(&[
            "log",
            "--no-graph",
            "-r",
            r#"description(substring:"remote changes on")"#,
            "-T",
            "change_id"
        ]),
        ""
    );
}

#[test]
fn sync_puts_remote_commit_after_locally_amended_change() {
    let (e, a, b) = stack(Forge::GitLab);
    e.upload(&[]).ok();
    let pushed = e.remote_branch(&a.branch).unwrap();
    remote_commit(&e, &a.branch, &pushed, "a.txt", "remote edit\n");
    e.jj(&["edit", &a.change]);
    e.write("local.txt", "local\n");
    e.jj(&["new", &b.change]);

    let out = e.upload(&["sync"]);

    assert_eq!(out.code, 1, "stderr: {}", out.stderr);
    assert!(
        out.stderr.contains(&a.branch) && out.stderr.contains("the remote has commits not in"),
        "stderr: {}",
        out.stderr
    );
    let marker = format!(r#"description(substring:"remote changes on {}")"#, a.branch);
    let remote_change = e.change_of(&marker);
    assert_eq!(
        e.parent_commit_of(&remote_change).trim(),
        e.commit_of(&a.change).trim()
    );
    assert_eq!(file_at(&e, &a.change, "local.txt"), "local\n");
    assert_eq!(file_at(&e, &remote_change, "a.txt"), "remote edit\n");
    assert_eq!(file_at(&e, &remote_change, "local.txt"), "local\n");
}

#[test]
fn sync_fails_when_remote_branch_shares_no_version() {
    let (e, a, _b) = stack(Forge::GitLab);
    e.upload(&[]).ok();
    let base = e.base.clone();
    let before = e.commit_of(&a.change);
    remote_commit(&e, &a.branch, &base, "a.txt", "rewritten\n");

    let out = e.upload(&["sync"]);

    assert_eq!(out.code, 1, "stderr: {}", out.stderr);
    assert!(
        out.stderr.contains("shares no version"),
        "stderr: {}",
        out.stderr
    );
    assert_eq!(e.commit_of(&a.change), before);
    assert_eq!(file_at(&e, &a.change, "a.txt"), "feat: a\n\nBody A\n");
}
