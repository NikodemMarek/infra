mod common;

use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

use common::{now, Env};
use serde_json::{json, Value};

const MAX: usize = 2_097_152;

fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn mode(p: &std::path::Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn event(id: &str, revision: Value) -> String {
    json!({"type": "review.requested", "review": id, "revision": revision}).to_string()
}

/// Publishes `plan.html` and returns the review id.
fn publish_one(e: &Env) -> String {
    e.write_plan("plan.html", "<html>v1</html>");
    let o = e.publish("plan.html", "My plan");
    assert_eq!(o.code, 0, "{}", o.stderr);
    let ids = e.ids();
    assert_eq!(ids.len(), 1);
    ids[0].clone()
}

fn assert_no_side_effects(e: &Env) {
    assert!(e.ids().is_empty(), "no review dir expected: {:?}", e.ids());
    assert_eq!(e.log("event-pub.log"), "");
}

// ---- publish: snapshot

#[test]
fn publish_creates_review_revision_1() {
    let e = Env::new();
    e.write_plan("plan.html", "<html>v1</html>");
    let o = e.publish("plan.html", "My plan");
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(o.stdout, "");
    let ids = e.ids();
    assert_eq!(ids.len(), 1);
    let id = &ids[0];
    assert!(is_hex(id, 16), "id {id}");
    let m = e.meta(id);
    assert!(is_hex(m["token"].as_str().unwrap(), 32));
    assert_eq!(m["session"], "sess-1");
    assert_eq!(m["title"], "My plan");
    assert_eq!(m["revision"], 1);
    assert_eq!(
        m["path"].as_str().unwrap(),
        e.work.join("plan.html").to_str().unwrap()
    );
    let updated = m["updated"].as_u64().unwrap();
    assert!(updated + 60 >= now() && updated <= now() + 60);
    assert_eq!(
        fs::read_to_string(e.root().join(id).join("r1.html")).unwrap(),
        "<html>v1</html>"
    );
}

#[test]
fn publish_absolute_path_is_stored_as_given() {
    let e = Env::new();
    e.write_plan("plan.html", "x");
    let abs = e.work.join("plan.html");
    let o = e.publish(abs.to_str().unwrap(), "t");
    assert_eq!(o.code, 0, "{}", o.stderr);
    let id = &e.ids()[0];
    assert_eq!(e.meta(id)["path"].as_str().unwrap(), abs.to_str().unwrap());
}

#[test]
fn publish_snapshots_exact_bytes() {
    let e = Env::new();
    let bytes: Vec<u8> = vec![0, 255, 10, 13, 200, 1, 2, 3];
    fs::write(e.work.join("plan.html"), &bytes).unwrap();
    assert_eq!(e.publish("plan.html", "t").code, 0);
    let id = &e.ids()[0];
    assert_eq!(fs::read(e.root().join(id).join("r1.html")).unwrap(), bytes);
}

#[test]
fn publish_again_makes_revision_2_with_same_id_and_token() {
    let e = Env::new();
    let id = publish_one(&e);
    let m1 = e.meta(&id);
    e.write_plan("plan.html", "<html>v2</html>");
    let o = e.publish("plan.html", "My plan 2");
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(e.ids(), vec![id.clone()]);
    let m2 = e.meta(&id);
    assert_eq!(m2["revision"], 2);
    assert_eq!(m2["token"], m1["token"]);
    assert_eq!(m2["title"], "My plan 2");
    let dir = e.root().join(&id);
    assert_eq!(fs::read_to_string(dir.join("r1.html")).unwrap(), "<html>v1</html>");
    assert_eq!(fs::read_to_string(dir.join("r2.html")).unwrap(), "<html>v2</html>");
}

#[test]
fn publish_other_path_makes_a_new_review() {
    let e = Env::new();
    let id1 = publish_one(&e);
    e.write_plan("other.html", "<html>o</html>");
    assert_eq!(e.publish("other.html", "Other").code, 0);
    let ids = e.ids();
    assert_eq!(ids.len(), 2);
    let id2 = ids.iter().find(|i| **i != id1).unwrap();
    assert_eq!(e.meta(id2)["revision"], 1);
    assert_ne!(e.meta(id2)["token"], e.meta(&id1)["token"]);
    assert_eq!(e.meta(&id1)["revision"], 1);
}

#[test]
fn publish_other_session_same_path_makes_a_new_review() {
    let e = Env::new();
    let id1 = publish_one(&e);
    assert_eq!(e.publish_as("sess-2", "plan.html", "t").code, 0);
    let ids = e.ids();
    assert_eq!(ids.len(), 2);
    let id2 = ids.iter().find(|i| **i != id1).unwrap();
    assert_eq!(e.meta(id2)["session"], "sess-2");
    assert_eq!(e.meta(id2)["revision"], 1);
}

#[test]
fn publish_prunes_reviews_older_than_7_days() {
    let e = Env::new();
    let old = publish_one(&e);
    e.write_plan("fresh.html", "f");
    assert_eq!(e.publish("fresh.html", "Fresh").code, 0);
    let fresh = e.ids().into_iter().find(|i| *i != old).unwrap();
    e.write_plan("third.html", "t");
    assert_eq!(e.publish("third.html", "Third").code, 0);
    assert_eq!(e.ids().len(), 3);

    e.set_meta(&old, "updated", json!(now() - 8 * 86400));
    e.set_meta(&fresh, "updated", json!(now() - 6 * 86400));
    e.write_plan("fourth.html", "4");
    assert_eq!(e.publish("fourth.html", "Fourth").code, 0);

    let ids = e.ids();
    assert!(!ids.contains(&old), "old review kept");
    assert!(ids.contains(&fresh), "6 day old review pruned");
    assert_eq!(ids.len(), 3);
}

#[test]
fn publish_prunes_old_review_dir_entirely() {
    let e = Env::new();
    let old = publish_one(&e);
    e.set_meta(&old, "updated", json!(now() - 8 * 86400));
    e.write_plan("b.html", "b");
    assert_eq!(e.publish("b.html", "B").code, 0);
    assert!(!e.root().join(&old).exists());
}

#[test]
fn publish_of_an_old_review_itself_makes_next_revision() {
    let e = Env::new();
    let id = publish_one(&e);
    e.set_meta(&id, "updated", json!(now() - 8 * 86400));
    e.write_plan("plan.html", "v2");
    assert_eq!(e.publish("plan.html", "t").code, 0);
    assert_eq!(e.ids(), vec![id.clone()]);
    assert_eq!(e.meta(&id)["revision"], 2);
}

// ---- publish: event

#[test]
fn publish_sends_review_requested_event() {
    let e = Env::new();
    let id = publish_one(&e);
    let log = e.log("event-pub.log");
    let mut lines = log.lines();
    assert_eq!(lines.next(), Some("type=review.requested"));
    let stdin = lines.next().unwrap().strip_prefix("stdin=").unwrap();
    assert_eq!(lines.next(), None);
    let v: Value = serde_json::from_str(stdin).unwrap();
    assert_eq!(v, json!({"review": id, "revision": 1}));

    e.write_plan("plan.html", "v2");
    assert_eq!(e.publish("plan.html", "t").code, 0);
    let log = e.log("event-pub.log");
    let last = log.lines().last().unwrap().strip_prefix("stdin=").unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(last).unwrap(),
        json!({"review": id, "revision": 2})
    );
}

#[test]
fn publish_event_pub_failure_exits_1_and_keeps_snapshot() {
    let e = Env::new();
    e.set_flag("event-pub.fail", "");
    e.write_plan("plan.html", "v1");
    let o = e.publish("plan.html", "t");
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("plan-review:"), "{}", o.stderr);
    assert_eq!(o.stdout, "");
    let ids = e.ids();
    assert_eq!(ids.len(), 1);
    let id = &ids[0];
    assert_eq!(e.meta(id)["revision"], 1);
    assert_eq!(fs::read_to_string(e.root().join(id).join("r1.html")).unwrap(), "v1");

    e.clear_flag("event-pub.fail");
    e.write_plan("plan.html", "v2");
    assert_eq!(e.publish("plan.html", "t").code, 0);
    assert_eq!(e.ids(), vec![id.clone()]);
    assert_eq!(e.meta(id)["revision"], 2);
}

// ---- publish: modes

#[test]
fn publish_modes_are_0700_dirs_and_0600_files() {
    let e = Env::new();
    let id = publish_one(&e);
    let root = e.root();
    let dir = root.join(&id);
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("meta.json")), 0o600);
    assert_eq!(mode(&dir.join("r1.html")), 0o600);
    assert_eq!(mode(&root.join(".lock")), 0o600);
}

// ---- publish: title

#[test]
fn publish_title_is_flattened_to_one_line() {
    let e = Env::new();
    e.write_plan("plan.html", "x");
    let o = e.publish("plan.html", "  a\n\tb \u{2028} c\u{1}d\u{85}e  ");
    assert_eq!(o.code, 0, "{}", o.stderr);
    let id = &e.ids()[0];
    assert_eq!(e.meta(id)["title"], "a b cd e");
}

#[test]
fn publish_title_is_cut_to_200_chars() {
    let e = Env::new();
    e.write_plan("plan.html", "x");
    let long: String = "é".repeat(300);
    assert_eq!(e.publish("plan.html", &long).code, 0);
    let id = &e.ids()[0];
    let t = e.meta(id)["title"].as_str().unwrap().to_string();
    assert_eq!(t.chars().count(), 200);
    assert_eq!(t, "é".repeat(200));
}

#[test]
fn publish_rejects_empty_title_after_oneline() {
    for title in ["", "   ", "\n\t", "\u{1}\u{2028}", "\u{7}"] {
        let e = Env::new();
        e.write_plan("plan.html", "x");
        let o = e.publish("plan.html", title);
        assert_eq!(o.code, 1, "title {title:?}");
        assert!(o.stderr.contains("plan-review:"), "{}", o.stderr);
        assert_eq!(o.stdout, "");
        assert_no_side_effects(&e);
    }
}

// ---- publish: session

#[test]
fn publish_rejects_unset_session() {
    let e = Env::new();
    e.write_plan("plan.html", "x");
    let o = e.run(&e.work, &[], "", &["publish", "plan.html", "t"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("plan-review:"), "{}", o.stderr);
    assert_no_side_effects(&e);
}

#[test]
fn publish_rejects_empty_and_invalid_session() {
    for s in ["", "-x", ".x", "a b", "a/b", "../x", "a\nb", "é", "a:b"] {
        let e = Env::new();
        e.write_plan("plan.html", "x");
        let o = e.publish_as(s, "plan.html", "t");
        assert_eq!(o.code, 1, "session {s:?}");
        assert!(o.stderr.contains("plan-review:"), "{}", o.stderr);
        assert_no_side_effects(&e);
    }
}

#[test]
fn publish_accepts_valid_session_names() {
    for s in ["a", "0", "s-1126-155455", "a.b_c-d", "A1"] {
        let e = Env::new();
        e.write_plan("plan.html", "x");
        let o = e.publish_as(s, "plan.html", "t");
        assert_eq!(o.code, 0, "session {s:?}: {}", o.stderr);
        assert_eq!(e.meta(&e.ids()[0])["session"], s);
    }
}

// ---- publish: file checks

#[test]
fn publish_rejects_symlink() {
    let e = Env::new();
    e.write_plan("real.html", "x");
    symlink(e.work.join("real.html"), e.work.join("link.html")).unwrap();
    let o = e.publish("link.html", "t");
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("plan-review:"), "{}", o.stderr);
    assert_no_side_effects(&e);
}

#[test]
fn publish_rejects_fifo_without_blocking() {
    let e = Env::new();
    let p = std::ffi::CString::new(e.work.join("pipe.html").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o600) }, 0);
    // Env::run panics when the binary blocks for 20 s
    let o = e.publish("pipe.html", "t");
    assert_eq!(o.code, 1);
    assert_no_side_effects(&e);
}

#[test]
fn publish_rejects_directory_and_missing_file() {
    let e = Env::new();
    fs::create_dir(e.work.join("dir.html")).unwrap();
    assert_eq!(e.publish("dir.html", "t").code, 1);
    assert_eq!(e.publish("missing.html", "t").code, 1);
    assert_no_side_effects(&e);
}

#[test]
fn publish_rejects_file_over_2_mib() {
    let e = Env::new();
    fs::write(e.work.join("big.html"), vec![b'a'; MAX + 1]).unwrap();
    let o = e.publish("big.html", "t");
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("plan-review:"), "{}", o.stderr);
    assert_no_side_effects(&e);
}

#[test]
fn publish_accepts_file_of_exactly_2_mib() {
    let e = Env::new();
    fs::write(e.work.join("big.html"), vec![b'a'; MAX]).unwrap();
    let o = e.publish("big.html", "t");
    assert_eq!(o.code, 0, "{}", o.stderr);
    let id = &e.ids()[0];
    assert_eq!(fs::metadata(e.root().join(id).join("r1.html")).unwrap().len(), MAX as u64);
}

#[test]
fn publish_rejects_non_html_extension() {
    for name in ["plan.txt", "plan.html.bak", "plan", "plan.htm"] {
        let e = Env::new();
        e.write_plan(name, "x");
        let o = e.publish(name, "t");
        assert_eq!(o.code, 1, "{name}");
        assert!(o.stderr.contains("plan-review:"), "{}", o.stderr);
        assert_no_side_effects(&e);
    }
}

#[test]
fn publish_rejection_keeps_existing_review_untouched() {
    let e = Env::new();
    let id = publish_one(&e);
    e.write_plan("plan.html", "v2");
    assert_eq!(e.publish("plan.html", "").code, 1);
    assert_eq!(e.meta(&id)["revision"], 1);
    assert!(!e.root().join(&id).join("r2.html").exists());
}

// ---- intake

const URL_PREFIX: &str = "http://127.0.0.1:4388/r/";

#[test]
fn intake_notifies_with_expected_argv() {
    let e = Env::new();
    let id = publish_one(&e);
    let o = e.intake(&event(&id, json!(1)));
    assert_eq!(o.code, 0, "{}", o.stderr);
    let calls = e.calls("notify-send.log");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0],
        vec![
            "-a",
            "plan-review",
            "-A",
            "open=Open",
            "--",
            "Plan review: My plan",
            "sess-1 \u{b7} r1",
        ]
    );
    assert!(e.calls("xdg-open.log").is_empty());
}

#[test]
fn intake_body_is_markup_escaped() {
    let e = Env::new();
    let id = publish_one(&e);
    e.set_meta(&id, "session", json!("a&b<c>"));
    let o = e.intake(&event(&id, json!(1)));
    assert_eq!(o.code, 0, "{}", o.stderr);
    let calls = e.calls("notify-send.log");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][6], "a&amp;b&lt;c&gt; \u{b7} r1");
}

#[test]
fn intake_uses_latest_revision_and_title_in_notification() {
    let e = Env::new();
    let id = publish_one(&e);
    e.write_plan("plan.html", "v2");
    assert_eq!(e.publish("plan.html", "Second title").code, 0);
    assert_eq!(e.intake(&event(&id, json!(2))).code, 0);
    let calls = e.calls("notify-send.log");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][5], "Plan review: Second title");
    assert_eq!(calls[0][6], "sess-1 \u{b7} r2");
}

#[test]
fn intake_open_action_runs_xdg_open_with_url() {
    let e = Env::new();
    let id = publish_one(&e);
    let token = e.meta(&id)["token"].as_str().unwrap().to_string();
    e.set_flag("notify-send.out", "open\n");
    let o = e.intake(&event(&id, json!(1)));
    assert_eq!(o.code, 0, "{}", o.stderr);
    let calls = e.calls("xdg-open.log");
    assert_eq!(calls, vec![vec![format!("{URL_PREFIX}{id}?t={token}")]]);
    assert!(!o.stdout.contains(&token));
    assert!(!o.stderr.contains(&token));
}

#[test]
fn intake_other_action_output_does_not_open() {
    for out in ["", "close", "opened", "x open"] {
        let e = Env::new();
        let id = publish_one(&e);
        e.set_flag("notify-send.out", &format!("{out}\n"));
        let o = e.intake(&event(&id, json!(1)));
        assert_eq!(o.code, 0, "{out:?}: {}", o.stderr);
        assert_eq!(e.calls("notify-send.log").len(), 1);
        assert!(e.calls("xdg-open.log").is_empty(), "{out:?}");
    }
}

#[test]
fn intake_never_prints_the_url() {
    let e = Env::new();
    let id = publish_one(&e);
    let token = e.meta(&id)["token"].as_str().unwrap().to_string();
    e.set_flag("notify-send.out", "open\n");
    let o = e.intake(&event(&id, json!(1)));
    assert_eq!(o.stdout, "");
    assert!(!o.stderr.contains(&token));
    assert!(!o.stderr.contains(URL_PREFIX));
}

#[test]
fn intake_stale_revision_does_nothing() {
    let e = Env::new();
    let id = publish_one(&e);
    e.write_plan("plan.html", "v2");
    assert_eq!(e.publish("plan.html", "t").code, 0);
    e.set_flag("notify-send.out", "open\n");
    let o = e.intake(&event(&id, json!(1)));
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(e.log("notify-send.log"), "");
    assert_eq!(e.log("xdg-open.log"), "");
}

#[test]
fn intake_future_revision_does_nothing() {
    let e = Env::new();
    let id = publish_one(&e);
    let o = e.intake(&event(&id, json!(2)));
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(e.log("notify-send.log"), "");
}

#[test]
fn intake_unknown_id_does_nothing() {
    let e = Env::new();
    publish_one(&e);
    e.set_flag("notify-send.out", "open\n");
    let o = e.intake(&event("0123456789abcdef", json!(1)));
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(e.log("notify-send.log"), "");
    assert_eq!(e.log("xdg-open.log"), "");
}

#[test]
fn intake_unknown_id_without_store_does_nothing() {
    let e = Env::new();
    let o = e.intake(&event("0123456789abcdef", json!(1)));
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(e.log("notify-send.log"), "");
}

#[test]
fn intake_bad_id_does_nothing() {
    for id in [
        "../x",
        "..",
        "0123456789ABCDEF",
        "0123456789abcde",
        "0123456789abcdef0",
        "",
        "0123456789abcdeg",
        "0123456789abcde/",
    ] {
        let e = Env::new();
        publish_one(&e);
        let o = e.intake(&event(id, json!(1)));
        assert_eq!(o.code, 0, "id {id:?}: {}", o.stderr);
        assert_eq!(e.log("notify-send.log"), "", "id {id:?}");
        assert_eq!(e.log("xdg-open.log"), "", "id {id:?}");
    }
}

#[test]
fn intake_bad_revision_does_nothing() {
    let e = Env::new();
    let id = publish_one(&e);
    for rev in [json!(0), json!(-1), json!("1"), json!(1.5), json!(null), json!([1])] {
        let o = e.intake(&event(&id, rev.clone()));
        assert_eq!(o.code, 0, "revision {rev}: {}", o.stderr);
        assert_eq!(e.log("notify-send.log"), "", "revision {rev}");
    }
    let o = e.intake(&json!({"review": id}).to_string());
    assert_eq!(o.code, 0);
    let o = e.intake(&json!({"revision": 1}).to_string());
    assert_eq!(o.code, 0);
    assert_eq!(e.log("notify-send.log"), "");
}

#[test]
fn intake_notify_failure_exits_0_without_opening() {
    let e = Env::new();
    let id = publish_one(&e);
    e.set_flag("notify-send.fail", "");
    e.set_flag("notify-send.out", "open\n");
    let o = e.intake(&event(&id, json!(1)));
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(e.calls("notify-send.log").len(), 1);
    assert_eq!(e.log("xdg-open.log"), "");
}

#[test]
fn intake_xdg_open_failure_exits_0() {
    let e = Env::new();
    let id = publish_one(&e);
    e.set_flag("xdg-open.fail", "");
    e.set_flag("notify-send.out", "open\n");
    let o = e.intake(&event(&id, json!(1)));
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(e.calls("xdg-open.log").len(), 1);
}

#[test]
fn intake_invalid_json_exits_1() {
    for input in ["", "nope", "{", "[1]", "\"str\"", "42", "null"] {
        let e = Env::new();
        publish_one(&e);
        let o = e.intake(input);
        assert_eq!(o.code, 1, "input {input:?}");
        assert!(o.stderr.contains("plan-review:"), "{}", o.stderr);
        assert_eq!(e.log("notify-send.log"), "");
    }
}

// ---- list

fn list_line(id: &str, token: &str, state: &str, session: &str, rev: u64, title: &str) -> String {
    format!("{URL_PREFIX}{id}?t={token}\t{state}\t{session}\tr{rev}\t{title}")
}

#[test]
fn list_empty_store_prints_nothing() {
    let e = Env::new();
    let o = e.list();
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(o.stdout, "");

    fs::create_dir_all(e.root()).unwrap();
    let o = e.list();
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(o.stdout, "");
}

#[test]
fn list_line_format() {
    let e = Env::new();
    let id = publish_one(&e);
    let token = e.meta(&id)["token"].as_str().unwrap().to_string();
    let o = e.list();
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(
        o.stdout,
        format!("{}\n", list_line(&id, &token, "open", "sess-1", 1, "My plan"))
    );
}

#[test]
fn list_shows_latest_revision() {
    let e = Env::new();
    let id = publish_one(&e);
    let token = e.meta(&id)["token"].as_str().unwrap().to_string();
    e.write_plan("plan.html", "v2");
    assert_eq!(e.publish("plan.html", "Renamed").code, 0);
    assert_eq!(
        e.list().stdout,
        format!("{}\n", list_line(&id, &token, "open", "sess-1", 2, "Renamed"))
    );
}

#[test]
fn list_is_newest_updated_first() {
    let e = Env::new();
    let mut ids = vec![];
    for (name, title) in [("a.html", "A"), ("b.html", "B"), ("c.html", "C")] {
        e.write_plan(name, "x");
        assert_eq!(e.publish(name, title).code, 0);
        let id = e.ids().into_iter().find(|i| !ids.contains(i)).unwrap();
        ids.push(id);
    }
    let t = now();
    e.set_meta(&ids[0], "updated", json!(t - 100));
    e.set_meta(&ids[1], "updated", json!(t - 300));
    e.set_meta(&ids[2], "updated", json!(t - 200));
    let out = e.list().stdout;
    let titles: Vec<&str> = out
        .lines()
        .map(|l| l.rsplit('\t').next().unwrap())
        .collect();
    assert_eq!(titles, vec!["A", "C", "B"]);
}

#[test]
fn list_sent_iff_feedback_for_latest_revision_exists() {
    let e = Env::new();
    let id = publish_one(&e);
    let dir = e.root().join(&id);
    let state = |e: &Env| e.list().stdout.split('\t').nth(1).unwrap().to_string();
    assert_eq!(state(&e), "open");

    fs::write(dir.join("feedback-r1.json"), "{}").unwrap();
    assert_eq!(state(&e), "sent");

    e.write_plan("plan.html", "v2");
    assert_eq!(e.publish("plan.html", "t").code, 0);
    assert_eq!(state(&e), "open");

    fs::write(dir.join("feedback-r2.json"), "{}").unwrap();
    assert_eq!(state(&e), "sent");
}

#[test]
fn list_mixed_states_and_sessions() {
    let e = Env::new();
    e.write_plan("a.html", "x");
    e.write_plan("b.html", "x");
    assert_eq!(e.publish_as("s-one", "a.html", "First").code, 0);
    let a = e.ids()[0].clone();
    assert_eq!(e.publish_as("s-two", "b.html", "Second").code, 0);
    let b = e.ids().into_iter().find(|i| *i != a).unwrap();
    let t = now();
    e.set_meta(&a, "updated", json!(t - 10));
    e.set_meta(&b, "updated", json!(t - 5));
    fs::write(e.root().join(&a).join("feedback-r1.json"), "{}").unwrap();
    let ta = e.meta(&a)["token"].as_str().unwrap().to_string();
    let tb = e.meta(&b)["token"].as_str().unwrap().to_string();
    assert_eq!(
        e.list().stdout,
        format!(
            "{}\n{}\n",
            list_line(&b, &tb, "open", "s-two", 1, "Second"),
            list_line(&a, &ta, "sent", "s-one", 1, "First")
        )
    );
}

#[test]
fn list_does_not_modify_the_store() {
    let e = Env::new();
    let id = publish_one(&e);
    let before = fs::metadata(e.root().join(&id).join("meta.json")).unwrap().mtime();
    e.list();
    let after = fs::metadata(e.root().join(&id).join("meta.json")).unwrap().mtime();
    assert_eq!(before, after);
    assert_eq!(e.log("event-pub.log").lines().count(), 2);
}

// ---- store location

#[test]
fn store_falls_back_to_home_local_state_when_xdg_unset_or_relative() {
    let e = Env::new();
    e.write_plan("plan.html", "x");
    let home = e.dir.join("home");
    for xdg in [None, Some("relative/dir")] {
        let unset: &[&str] = if xdg.is_none() { &["XDG_STATE_HOME"] } else { &[] };
        let envs: Vec<(&str, &str)> = [("AGENT_SESSION", "sess-1")]
            .into_iter()
            .chain(xdg.map(|x| ("XDG_STATE_HOME", x)))
            .collect();
        let o = e.run_without(unset, &e.work, &envs, "", &["publish", "plan.html", "t"]);
        assert_eq!(o.code, 0, "{}", o.stderr);
        let root = home.join(".local/state/plan-review");
        assert!(root.is_dir(), "xdg {xdg:?}");
        assert!(!e.work.join("relative").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}

// ---- concurrency

#[test]
fn concurrent_publishes_are_serialized() {
    let e = Env::new();
    e.write_plan("plan.html", "<html>v</html>");
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..8)
            .map(|_| s.spawn(|| e.publish("plan.html", "t")))
            .collect();
        for h in hs {
            let o = h.join().unwrap();
            assert_eq!(o.code, 0, "{}", o.stderr);
        }
    });
    let ids = e.ids();
    assert_eq!(ids.len(), 1, "{ids:?}");
    assert_eq!(e.meta(&ids[0])["revision"], json!(8));
    for n in 1..=8 {
        let f = e.root().join(&ids[0]).join(format!("r{n}.html"));
        assert!(f.is_file(), "r{n}.html");
    }
}
