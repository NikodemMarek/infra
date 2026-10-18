mod common;

use std::process::Command;

use common::{f, json_lines, nums, with_line, Env, FIXTURE, KUBE, SUNSHINE, WARDROBE, WIREGUARD};
use serde_json::{json, Value};

#[test]
fn list_in_flag_is_usage_error() {
    let e = Env::fixture();
    e.tm(&["list", "--in", "backlog"]).usage_error();
}

#[test]
fn list_no_flags_lists_all_tasks_in_file_order() {
    let e = Env::fixture();
    let o = e.tm(&["list"]);
    o.ok();
    assert_eq!(nums(&o.stdout), ["2", "3", "4", "5", "6", "7", "8"]);
    assert_eq!(o.stdout.lines().nth(1), Some(format!("3: {KUBE}").as_str()));
}

#[test]
fn list_open_is_todo_and_doing() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--status", "open"]);
    o.ok();
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert_eq!(
        lines,
        [
            format!("2: {WIREGUARD}"),
            format!("3: {KUBE}"),
            format!("4: {SUNSHINE}"),
            "6: - [ ] write a blog post @est(2h) #fun".to_string(),
            format!("8: {WARDROBE}"),
        ]
    );
}

#[test]
fn list_doing() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--status", "doing"]);
    o.ok();
    assert_eq!(o.stdout, format!("3: {KUBE}\n8: {WARDROBE}\n"));
    assert_eq!(e.tm(&["list", "-s", "doing"]).ok().stdout, o.stdout);
}

#[test]
fn list_todo() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--status", "todo"]);
    o.ok();
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert_eq!(
        lines,
        [
            format!("2: {WIREGUARD}"),
            format!("4: {SUNSHINE}"),
            "6: - [ ] write a blog post @est(2h) #fun".to_string(),
        ]
    );
}

#[test]
fn list_done() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--status", "done"]);
    o.ok();
    assert_eq!(
        o.stdout,
        "5: - [x] old done thing @est(1h)\n\
         7: - [x] read ML slides @est(2h) @due(2026-10-14) #university @done(2026-10-14)\n"
    );
}

#[test]
fn list_status_takes_one_known_value() {
    let e = Env::fixture();
    for args in [
        &["list", "--status", "bogus"][..],
        &["list", "--status", "open", "--status", "done"][..],
        &["list", "--open"][..],
    ] {
        e.tm(args).usage_error();
    }
}

#[test]
fn list_status_filters_combine_with_other_filters() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--status", "doing", "--tag", "self-hosting"]);
    o.ok();
    assert_eq!(nums(&o.stdout), ["3"]);

    let o = e.tm(&["list", "--status", "open", "--tag", "self-hosting"]);
    o.ok();
    assert_eq!(nums(&o.stdout), ["2", "3", "4"]);

    let o = e.tm(&["list", "--status", "open", "--due-before", "2026-10-30"]);
    o.ok();
    assert_eq!(nums(&o.stdout), ["3"]);

    let o = e.tm(&["list", "--status", "doing", "wardrobe"]);
    o.ok();
    assert_eq!(nums(&o.stdout), ["8"]);
}

#[test]
fn list_json_status_field() {
    let e = Env::fixture();
    let v = e.tm(&["list", "--json"]).ok().json();
    let items = v.as_array().expect("array");
    let status =
        |line: f64| -> &Value { &items.iter().find(|i| f(&i["line"]) == line).unwrap()["status"] };
    assert_eq!(status(2.0), "todo");
    assert_eq!(status(3.0), "doing");
    assert_eq!(status(5.0), "done");
    assert_eq!(status(8.0), "doing");
    assert!(items.iter().all(|i| i.get("done").is_none()));
}

#[test]
fn list_json_doing() {
    let e = Env::fixture();
    let v = e.tm(&["list", "--json", "--status", "doing"]).ok().json();
    assert_eq!(json_lines(&v), [3.0, 8.0]);
    for it in v.as_array().unwrap() {
        assert_eq!(it["status"], "doing");
    }
}

#[test]
fn list_json_open_shape() {
    let e = Env::fixture();
    let v = e.tm(&["list", "--status", "open", "--json"]).ok().json();
    let items = v.as_array().expect("array");
    assert_eq!(json_lines(&v), [2.0, 3.0, 4.0, 6.0, 8.0]);
    let it = &items[1];
    assert_eq!(it["status"], "doing");
    assert_eq!(it["description"], "kubernetes cluster runs wireguard");
    assert_eq!(it["tags"], json!(["self-hosting"]));
    assert_eq!(it["est"], "4h");
    assert_eq!(it["due"], "2026-10-25");
    for k in ["created", "done_on"] {
        assert_eq!(it.get(k), Some(&Value::Null), "{k}");
    }
    assert!(it.get("tokens").is_none());
    assert_eq!(it["text"], KUBE);

    let mut keys: Vec<&str> = it.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    keys.sort();
    let mut want = vec![
        "line",
        "status",
        "description",
        "tags",
        "est",
        "due",
        "created",
        "done_on",
        "text",
    ];
    want.sort();
    assert_eq!(keys, want);
}

#[test]
fn list_json_done_item() {
    let e = Env::fixture();
    let v = e.tm(&["list", "--status", "done", "--json"]).ok().json();
    let items = v.as_array().expect("array");
    assert_eq!(json_lines(&v), [5.0, 7.0]);
    assert_eq!(items[1]["status"], "done");
    assert_eq!(items[1]["due"], "2026-10-14");
    assert_eq!(items[1]["done_on"], "2026-10-14");
    assert_eq!(items[1]["tags"], json!(["university"]));
}

#[test]
fn list_tag_filters() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--tag", "self"]);
    o.ok();
    assert_eq!(o.stdout, "");
    let o = e.tm(&["list", "--tag", "self-hosting"]);
    o.ok();
    assert_eq!(nums(&o.stdout), ["2", "3", "4"], "{}", o.stdout);
}

#[test]
fn list_tag_matches_subtags_and_requires_all() {
    let e = Env::empty();
    e.write(
        "tasks.md",
        "- [ ] a #health/sleep\n- [ ] b #health #fun\n- [ ] c #healthy\n",
    );
    let o = e.tm(&["list", "--tag", "health"]);
    o.ok();
    assert_eq!(
        o.stdout,
        "1: - [ ] a #health/sleep\n2: - [ ] b #health #fun\n"
    );
    let o = e.tm(&["list", "--tag", "health", "--tag", "fun"]);
    o.ok();
    assert_eq!(o.stdout, "2: - [ ] b #health #fun\n");
}

#[test]
fn list_due_before() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--due-before", "2026-10-20"]);
    o.ok();
    assert_eq!(
        o.stdout,
        "7: - [x] read ML slides @est(2h) @due(2026-10-14) #university @done(2026-10-14)\n"
    );
}

#[test]
fn list_proj_flag_is_usage_error() {
    Env::fixture().tm(&["list", "--proj", "x"]).usage_error();
}

#[test]
fn list_query_and_empty_json() {
    let e = Env::fixture();
    let o = e.tm(&["list", "BLOG"]);
    o.ok();
    assert_eq!(o.stdout, "6: - [ ] write a blog post @est(2h) #fun\n");

    let o = e.tm(&["list", "zzzz", "--json"]);
    o.ok();
    assert_eq!(o.json(), json!([]));
    let o = e.tm(&["list", "zzzz"]);
    o.ok();
    assert_eq!(o.stdout, "");
}

#[test]
fn list_json_typed_shape() {
    let e = Env::empty();
    e.write(
        "tasks.md",
        "- [x] a @est(0.5h) @created(2026-10-01) @done(2026-10-18) #t\n",
    );
    let v = e.tm(&["list", "--json"]).ok().json();
    let it = &v.as_array().expect("array")[0];
    assert_eq!(it["est"], "0.5h");
    assert_eq!(it["created"], "2026-10-01");
    assert_eq!(it["done_on"], "2026-10-18");
    assert_eq!(it["tags"], json!(["t"]));
}

#[test]
fn list_skips_invalid_line_with_warning() {
    let e = Env::fixture();
    e.write("tasks.md", &with_line(FIXTURE, "- [ ] x @project(foo)"));
    let o = e.tm(&["list"]);
    o.ok();
    assert!(!o.stdout.contains("10:"), "{}", o.stdout);
    assert_eq!(o.stderr, "tm: skipping line 10: unknown token @project\n");
}

#[test]
fn list_json_closed_pipe_does_not_panic() {
    let e = Env::empty();
    let mut b = String::new();
    for i in 0..20000 {
        b.push_str(&format!("- [ ] task number {i} @est(1h) #tag\n"));
    }
    e.write("tasks.md", &b);
    let mut child = Command::new(env!("CARGO_BIN_EXE_tm"))
        .args(["list", "--json"])
        .env("TM_DIR", &e.dir)
        .env("TM_TODAY", "2026-10-18")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let o = child.wait_with_output().unwrap();
    assert_ne!(o.status.code(), Some(101));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn json_flag_before_subcommand() {
    let e = Env::fixture();
    let before = e.tm(&["--json", "list"]).ok().json();
    let after = e.tm(&["list", "--json"]).ok().json();
    assert_eq!(before, after);
    assert!(before.is_array());
}
