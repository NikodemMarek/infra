use super::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Temp directory removed on drop.
struct Tmp(PathBuf);

impl std::ops::Deref for Tmp {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Fresh empty directory under the temp dir, unique per call.
fn tmp(name: &str) -> Tmp {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agent-inbox-t-{}-{}-{name}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    Tmp(dir)
}

fn put(dir: &Path, file: &str, text: &str) {
    fs::write(dir.join(file), text).unwrap();
}

fn ids(items: &[Item]) -> Vec<&str> {
    items.iter().map(|i| i.id.as_str()).collect()
}

#[test]
fn list_at_missing_dir_is_empty() {
    let dir = tmp("missing");
    let items = list_at(&dir.join("nope"), 0.0).unwrap();
    assert!(items.is_empty());
}

#[test]
fn list_at_reads_items_sorted_by_received() {
    let dir = tmp("sorted");
    put(
        &dir,
        "b2.json",
        r#"{"id":"zzz","received":200.0,"event":{"type":"claude.question"}}"#,
    );
    put(
        &dir,
        "a1.json",
        r#"{"id":"yyy","received":100.5,"event":{"type":"sudo.password_request","title":"sudo ls","reply":"/dev/null"}}"#,
    );
    put(&dir, "c3.json", r#"{"received":150,"event":{"type":"x"}}"#);
    let items = list_at(&dir, 1000.0).unwrap();
    // Item.id is the file stem, the JSON id is ignored
    assert_eq!(ids(&items), vec!["a1", "c3", "b2"]);
    assert_eq!(items[0].received, 100.5);
    assert_eq!(items[1].received, 150.0);
    assert_eq!(items[0].kind(), "sudo.password_request");
    assert_eq!(items[0].title(), "sudo ls");
    assert_eq!(items[0].reply(), Some("/dev/null"));
    assert_eq!(items[2].reply(), None);
    assert_eq!(items[2].project(), "");
}

#[test]
fn list_at_non_numeric_received_is_zero() {
    let dir = tmp("recv0");
    put(
        &dir,
        "a.json",
        r#"{"received":"soon","event":{"type":"x"}}"#,
    );
    put(&dir, "b.json", r#"{"event":{"type":"x"}}"#);
    put(&dir, "c.json", r#"{"received":5,"event":{"type":"x"}}"#);
    let items = list_at(&dir, 10.0).unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[2].id, "c");
    assert_eq!(items[0].received, 0.0);
    assert_eq!(items[1].received, 0.0);
}

#[test]
fn list_at_timeout_expires() {
    let dir = tmp("timeout");
    put(
        &dir,
        "t.json",
        r#"{"received":100,"event":{"type":"x","timeout":10}}"#,
    );
    let items = list_at(&dir, 105.0).unwrap();
    assert_eq!(ids(&items), vec!["t"]);
    assert!(dir.join("t.json").exists());
    let items = list_at(&dir, 111.0).unwrap();
    assert!(items.is_empty());
    assert!(!dir.join("t.json").exists());
}

#[test]
fn list_at_bool_timeout_is_ignored() {
    let dir = tmp("timeout-bool");
    put(
        &dir,
        "t.json",
        r#"{"received":100,"event":{"type":"x","timeout":true}}"#,
    );
    let items = list_at(&dir, 1_000_000.0).unwrap();
    assert_eq!(ids(&items), vec!["t"]);
    assert!(dir.join("t.json").exists());
}

#[test]
fn list_at_reply_path_decides_expiry() {
    let dir = tmp("reply");
    let alive = dir.join("alive-reply");
    fs::write(&alive, "").unwrap();
    let dangling = dir.join("dangling-reply");
    std::os::unix::fs::symlink(dir.join("no-such-target"), &dangling).unwrap();
    let gone = dir.join("gone-reply");
    let item =
        |reply: &str| json!({"received": 1, "event": {"type": "x", "reply": reply}}).to_string();
    put(&dir, "gone.json", &item(gone.to_str().unwrap()));
    put(&dir, "alive.json", &item(alive.to_str().unwrap()));
    put(&dir, "dangling.json", &item(dangling.to_str().unwrap()));
    put(&dir, "empty.json", &item(""));
    let items = list_at(&dir, 2.0).unwrap();
    let mut got = ids(&items);
    got.sort();
    assert_eq!(got, vec!["alive", "dangling", "empty"]);
    assert!(!dir.join("gone.json").exists());
    for f in ["alive.json", "dangling.json", "empty.json"] {
        assert!(dir.join(f).exists(), "{f}");
    }
}

#[test]
fn list_at_skips_junk_and_leaves_it_on_disk() {
    let dir = tmp("junk");
    put(&dir, "ok.json", r#"{"received":1,"event":{"type":"x"}}"#);
    put(&dir, "bad.json", "{not json");
    put(&dir, "arr.json", "[1]");
    put(&dir, "noevent.json", r#"{"received":1}"#);
    put(
        &dir,
        "bad name!.json",
        r#"{"received":1,"event":{"type":"x"}}"#,
    );
    put(&dir, "x.txt", r#"{"received":1,"event":{"type":"x"}}"#);
    let items = list_at(&dir, 2.0).unwrap();
    assert_eq!(ids(&items), vec!["ok"]);
    for f in [
        "bad.json",
        "arr.json",
        "noevent.json",
        "bad name!.json",
        "x.txt",
    ] {
        assert!(dir.join(f).exists(), "{f}");
    }
}

#[test]
fn list_at_string_timeout_does_not_expire() {
    let dir = tmp("timeout-str");
    put(
        &dir,
        "t.json",
        r#"{"received":100,"event":{"type":"x","timeout":"10"}}"#,
    );
    let items = list_at(&dir, 1_000_000.0).unwrap();
    assert_eq!(ids(&items), vec!["t"]);
    assert!(dir.join("t.json").exists());
}

#[test]
fn list_at_skips_fifos_and_oversized_files() {
    let dir = tmp("fifo-big");
    put(&dir, "ok.json", r#"{"received":1,"event":{"type":"x"}}"#);
    let big = format!(
        r#"{{"received":1,"event":{{"type":"x","pad":"{}"}}}}"#,
        "x".repeat(1024 * 1024)
    );
    put(&dir, "big.json", &big);
    let fifo = dir.join("x.json");
    let made = Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !made {
        eprintln!("mkfifo unavailable, skipping the FIFO part");
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let d: PathBuf = dir.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(list_at(&d, 2.0).map_err(|e| e.to_string()));
    });
    let items = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("list_at blocked")
        .unwrap();
    assert_eq!(ids(&items), vec!["ok"]);
    assert!(dir.join("big.json").exists());
    if made {
        assert!(fifo.exists());
    }
}

#[test]
fn list_at_accepts_dash_and_underscore_stems() {
    let dir = tmp("stems");
    put(&dir, "A-b_9.json", r#"{"received":1,"event":{"type":"x"}}"#);
    let items = list_at(&dir, 2.0).unwrap();
    assert_eq!(ids(&items), vec!["A-b_9"]);
}

#[test]
fn list_reads_the_directory() {
    let dir = tmp("list");
    put(&dir, "a.json", r#"{"received":1,"event":{"type":"x"}}"#);
    let items = list(&dir).unwrap();
    assert_eq!(ids(&items), vec!["a"]);
}

#[test]
fn dismiss_removes_existing_and_reports_missing() {
    let dir = tmp("dismiss");
    put(&dir, "a1.json", r#"{"received":1,"event":{"type":"x"}}"#);
    assert!(dismiss(&dir, "a1").unwrap());
    assert!(!dir.join("a1.json").exists());
    assert!(!dismiss(&dir, "a1").unwrap());
    assert!(!dismiss(&dir, "never").unwrap());
}

#[test]
fn dismiss_rejects_odd_ids() {
    let root = tmp("dismiss-bad");
    let dir = root.join("inbox");
    fs::create_dir(&dir).unwrap();
    put(&root, "x.json", "{}");
    assert!(dismiss(&dir, "../x").is_err());
    assert!(dismiss(&dir, "a/b").is_err());
    assert!(dismiss(&dir, "").is_err());
}

/// A stub program recording its arguments and stdin; prints `stderr` and exits with `code`.
fn stub_program(dir: &Path, code: i32, stderr: &str) -> PathBuf {
    let path = dir.join("stub.sh");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{d}/args'\ncat > '{d}/stdin'\necho '{stderr}' >&2\nexit {code}\n",
        d = dir.display()
    );
    // Children forked by parallel tests inherit any write fd we hold on the stub, and exec of a
    // file open for writing fails with ETXTBSY; let a cp child write it so we never hold one.
    let src = dir.join("stub.src");
    fs::write(&src, script).unwrap();
    assert!(Command::new("cp")
        .arg(&src)
        .arg(&path)
        .status()
        .unwrap()
        .success());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn publish_passes_type_as_argument_and_event_on_stdin() {
    let dir = tmp("publish");
    let prog = stub_program(&dir, 0, "");
    let event = json!({"type": "task.new", "prompt": "x"});
    assert_eq!(publish(prog.to_str().unwrap(), &event), Ok(()));
    assert_eq!(fs::read_to_string(dir.join("args")).unwrap(), "task.new\n");
    let stdin: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("stdin")).unwrap()).unwrap();
    assert_eq!(stdin, event);
}

#[test]
fn publish_failure_carries_stderr() {
    let dir = tmp("publish-fail");
    let prog = stub_program(&dir, 1, "boom");
    let err = publish(prog.to_str().unwrap(), &json!({"type": "t"})).unwrap_err();
    assert!(err.contains("boom"), "{err}");
}

#[test]
fn publish_fails_when_program_is_missing() {
    let dir = tmp("publish-missing");
    let prog = dir.join("does-not-exist");
    assert!(publish(prog.to_str().unwrap(), &json!({"type": "t"})).is_err());
}

#[test]
fn publish_requires_a_string_type() {
    let dir = tmp("publish-notype");
    let prog = stub_program(&dir, 0, "");
    for event in [
        json!({"prompt": "x"}),
        json!({"type": 5}),
        json!({"type": null}),
    ] {
        assert_eq!(
            publish(prog.to_str().unwrap(), &event),
            Err("event without a type".to_string())
        );
    }
    // the program was never run
    assert!(!dir.join("args").exists());
}

#[test]
fn task_event_leaves_out_what_is_empty() {
    assert_eq!(
        task_event(&Task::default()),
        serde_json::json!({"type": "task.new"})
    );
    let full = Task {
        prompt: "fix".into(),
        images: vec!["/a.png".into()],
        repo: Some("/r".into()),
        id: Some("c1".into()),
    };
    assert_eq!(
        task_event(&full),
        serde_json::json!({"type": "task.new", "prompt": "fix", "images": ["/a.png"], "repo": "/r", "id": "c1"})
    );
    let partial = Task {
        repo: Some("/r".into()),
        ..Task::default()
    };
    assert_eq!(
        task_event(&partial),
        serde_json::json!({"type": "task.new", "repo": "/r"})
    );
}

#[test]
fn questions_are_read_from_tool_input() {
    let event = serde_json::json!({"type": "claude.question", "tool_input": {"questions": [
        {"question": "Which?", "multiSelect": true, "options": [
            {"label": "A", "description": "first"}, {"label": "B"}, {"description": "no label"}]},
        {"header": "no question text"},
        {"question": "Sure?", "header": "Confirm"}
    ]}});
    let item = Item {
        id: "q".into(),
        received: 0.0,
        event,
    };
    assert_eq!(
        item.questions(),
        vec![
            Question {
                question: "Which?".into(),
                header: "".into(),
                multi: true,
                labels: vec!["A".into(), "B".into()],
                descriptions: vec!["first".into(), "".into()],
            },
            Question {
                question: "Sure?".into(),
                header: "Confirm".into(),
                multi: false,
                labels: vec![],
                descriptions: vec![],
            },
        ]
    );
    let none = Item {
        id: "n".into(),
        received: 0.0,
        event: Value::Null,
    };
    assert!(none.questions().is_empty());
}

#[test]
fn project_name_prefers_workspace_repo_then_cwd_project_host() {
    let name = |event: Value| {
        Item {
            id: "a".into(),
            received: 0.0,
            event,
        }
        .project_name()
    };
    assert_eq!(
        name(json!({"cwd": "/home/u/projects/infra.agents/s-1012-002032-2abc"})),
        "infra"
    );
    assert_eq!(name(json!({"cwd": "/home/u/projects/infra/"})), "infra");
    // a bare `.agents` component names nothing
    assert_eq!(name(json!({"cwd": "/x/.agents/w1"})), "w1");
    assert_eq!(name(json!({"cwd": "/", "project": "p", "host": "h"})), "p");
    assert_eq!(name(json!({"project": "", "host": "h"})), "h");
    assert_eq!(name(Value::Null), "");
    // the jj repo wins over everything else
    assert_eq!(
        name(json!({"cwd": "/home/u/projects/infra.agents/s-1", "jj": {"repo": "real"}})),
        "real"
    );
    assert_eq!(name(json!({"cwd": "/x/y", "jj": {"repo": ""}})), "y");
}

#[test]
fn session_id_is_empty_without_one() {
    let id = |event: Value| {
        Item {
            id: "a".into(),
            received: 0.0,
            event,
        }
        .session_id()
        .to_string()
    };
    assert_eq!(id(json!({"session_id": "abc-123"})), "abc-123");
    assert_eq!(id(json!({"cwd": "/x"})), "");
    assert_eq!(id(Value::Null), "");
}

fn item_with(event: Value) -> Item {
    Item {
        id: "i".into(),
        received: 0.0,
        event,
    }
}

#[test]
fn item_workspace_is_the_jj_workspace() {
    let ws = |event: Value| item_with(event).workspace();
    assert_eq!(
        ws(json!({"cwd": "/p/dotfiles", "jj": {"repo": "dotfiles", "workspace": "w-1"}})),
        "w-1"
    );
    // the jj workspace wins over the cwd
    assert_eq!(
        ws(json!({"cwd": "/p/a.agents/s-2", "jj": {"repo": "a", "workspace": "w-1"}})),
        "w-1"
    );
}

#[test]
fn item_workspace_falls_back_to_the_agents_dir_of_the_cwd() {
    let ws = |event: Value| item_with(event).workspace();
    // no jj
    assert_eq!(ws(json!({"cwd": "/p/a.agents/s-2"})), "s-2");
    assert_eq!(ws(json!({"cwd": "/p/a.agents/s-2/sub"})), "s-2");
    // the default or an empty jj workspace is no workspace
    assert_eq!(
        ws(json!({"cwd": "/p/a.agents/s-2", "jj": {"repo": "a", "workspace": "default"}})),
        "s-2"
    );
    assert_eq!(
        ws(json!({"cwd": "/p/a.agents/s-2", "jj": {"repo": "a", "workspace": ""}})),
        "s-2"
    );
}

#[test]
fn item_workspace_is_empty_without_a_workspace() {
    let ws = |event: Value| item_with(event).workspace();
    assert_eq!(
        ws(json!({"cwd": "/p/dotfiles", "jj": {"repo": "dotfiles", "workspace": "default"}})),
        ""
    );
    assert_eq!(
        ws(json!({"cwd": "/p/dotfiles", "jj": {"repo": "dotfiles", "workspace": ""}})),
        ""
    );
    assert_eq!(ws(json!({"cwd": "/p/dotfiles"})), "");
    assert_eq!(ws(json!({"cwd": "/p/a.agents/"})), "");
    assert_eq!(ws(json!({})), "");
}

#[test]
fn jj_line_follows_the_statusline_format() {
    let line = |jj: Value| {
        Item {
            id: "a".into(),
            received: 0.0,
            event: json!({"jj": jj}),
        }
        .jj_line()
    };
    assert_eq!(
        line(
            json!({"repo": "r", "workspace": "w", "subdir": "pkgs/x", "change": "abc",
            "stack": {"index": 2, "size": 3}})
        ),
        Some("r/pkgs/x ws:w @abc 2/3".to_string())
    );
    assert_eq!(
        line(
            json!({"repo": "r", "workspace": "default", "subdir": "", "change": null,
            "stack": null})
        ),
        Some("r ws:default".to_string())
    );
    assert_eq!(line(json!({"workspace": "w"})), None);
    assert_eq!(line(Value::Null), None);
}

#[test]
fn age_and_clean() {
    let item = Item {
        id: "a".into(),
        received: 1000.0,
        event: Value::Null,
    };
    assert_eq!(item.age(1005.0), "5s");
    assert_eq!(item.age(1000.0 + 180.0), "3m");
    assert_eq!(item.age(1000.0 + 7200.0), "2h");
    assert_eq!(item.age(900.0), "0s");
    assert_eq!(clean("a\x1b[31m\tb\nc"), "a [31m b c");
}

#[test]
fn agent_session_is_empty_without_one() {
    assert_eq!(item_with(json!({"session": "s-1"})).agent_session(), "s-1");
    assert_eq!(item_with(json!({})).agent_session(), "");
    assert_eq!(
        item_with(json!({"origin_file": "/o/1"})).agent_session(),
        ""
    );
}

const FULL: &str = r#"{"kind":"review","title":"!7 Fix","description":"d","url":"https://g/7","mrs":["https://g/8",5]}"#;

#[test]
fn record_loads_all_fields_and_skips_non_string_mrs() {
    let dir = tmp("rec-full");
    put(&dir, "s-1.json", FULL);
    assert_eq!(
        Record::load(&dir, "s-1"),
        Some(Record {
            kind: "review".into(),
            title: "!7 Fix".into(),
            description: "d".into(),
            url: "https://g/7".into(),
            mrs: vec!["https://g/8".into()],
            ..Record::default()
        })
    );
}

#[test]
fn record_loads_the_status_fields() {
    let dir = tmp("rec-status");
    put(
        &dir,
        "s.json",
        r#"{"status":"waiting","waiting_for":"permission prompt","status_at":1700000000.5}"#,
    );
    assert_eq!(
        Record::load(&dir, "s"),
        Some(Record {
            status: "waiting".into(),
            waiting_for: "permission prompt".into(),
            status_at: Some(1700000000.5),
            ..Record::default()
        })
    );
}

#[test]
fn record_with_wrong_types_reads_as_empty() {
    let dir = tmp("rec-types");
    put(
        &dir,
        "s.json",
        r#"{"kind":1,"mrs":"x","status":2,"waiting_for":true,"status_at":"now"}"#,
    );
    assert_eq!(Record::load(&dir, "s"), Some(Record::default()));
}

#[test]
fn record_is_missing_for_bad_names_and_files() {
    let dir = tmp("rec-bad");
    put(&dir, "ok.json", FULL);
    assert!(Record::load(&dir, "ok").is_some());
    assert_eq!(Record::load(&dir, "gone"), None);
    // a valid record sits at every path a bad name would reach
    let sub = dir.join("sub");
    fs::create_dir_all(sub.join("a")).unwrap();
    put(&dir, "ok.json", FULL); // "../ok" from sub
    for f in [".lock.json", "-x.json", ".json", "a/b.json"] {
        put(&sub, f, FULL);
    }
    for name in ["", "../ok", "a/b", ".lock", "-x"] {
        assert_eq!(Record::load(&sub, name), None, "{name:?}");
    }
    // a symlink is missing even when it points at a valid record
    std::os::unix::fs::symlink(dir.join("ok.json"), dir.join("link.json")).unwrap();
    assert_eq!(Record::load(&dir, "link"), None);
    fs::create_dir(dir.join("d.json")).unwrap();
    assert_eq!(Record::load(&dir, "d"), None);
    put(&dir, "arr.json", "[]");
    assert_eq!(Record::load(&dir, "arr"), None);
    put(&dir, "junk.json", "{not json");
    assert_eq!(Record::load(&dir, "junk"), None);
}

#[test]
fn record_load_does_not_block_on_a_fifo() {
    let dir = tmp("rec-fifo");
    let made = Command::new("mkfifo")
        .arg(dir.join("f.json"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !made {
        eprintln!("mkfifo unavailable, skipping");
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let d: PathBuf = dir.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(Record::load(&d, "f"));
    });
    let got = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("Record::load blocked");
    assert_eq!(got, None);
}

#[test]
fn record_size_limit_is_65536_bytes() {
    let dir = tmp("rec-size");
    let base = r#"{"title":""}"#.len();
    let with_len = |len: usize| format!(r#"{{"title":"{}"}}"#, "x".repeat(len - base));
    let ok = with_len(65536);
    assert_eq!(ok.len(), 65536);
    put(&dir, "ok.json", &ok);
    let rec = Record::load(&dir, "ok").expect("exactly 65536 bytes loads");
    assert_eq!(rec.title.len(), 65536 - base);
    let big = with_len(65537);
    assert_eq!(big.len(), 65537);
    put(&dir, "big.json", &big);
    assert_eq!(Record::load(&dir, "big"), None);
}

fn rec(url: &str, mrs: &[&str]) -> Record {
    Record {
        url: url.into(),
        mrs: mrs.iter().map(|s| s.to_string()).collect(),
        ..Record::default()
    }
}

#[test]
fn record_link_follows_the_link_rule() {
    assert_eq!(
        rec("", &["https://b", "https://c"]).link(),
        Some("https://b")
    );
    assert_eq!(rec("https://a", &["https://b"]).link(), Some("https://a"));
    assert_eq!(rec("http://a", &["https://b"]).link(), None);
    assert_eq!(rec("javascript:x", &[]).link(), None);
    assert_eq!(rec("", &["file:///etc"]).link(), None);
    assert_eq!(rec("", &[]).link(), None);
}

#[test]
fn record_link_requires_printable_ascii_https_up_to_2048() {
    let url = |len: usize| format!("https://{}", "a".repeat(len - 8));
    let ok = url(2048);
    assert_eq!(ok.len(), 2048);
    assert_eq!(rec(&ok, &[]).link(), Some(ok.as_str()));
    let long = url(2049);
    assert_eq!(rec(&long, &[]).link(), None);
    for bad in [
        "https://",
        "https://a\u{202e}",
        "https://ż",
        "https://a\tb",
        "https://a b",
    ] {
        assert_eq!(rec(bad, &[]).link(), None, "{bad:?}");
        assert_eq!(rec("", &[bad]).link(), None, "mrs {bad:?}");
    }
    let good = "https://g/a-b_c?x=1&y=%20";
    assert_eq!(rec(good, &[]).link(), Some(good));
}

#[test]
fn record_label_cleans_the_title() {
    let label = |t: &str| {
        Record {
            title: t.into(),
            ..Record::default()
        }
        .label()
    };
    assert_eq!(label("!7 Fix"), Some("!7 Fix".into()));
    assert_eq!(label("a\x1b[31m\nb"), Some("a [31m b".into()));
    assert_eq!(label("  x\t"), Some("x".into()));
    assert_eq!(label(""), None);
    assert_eq!(label(" \n\t "), None);
}

#[test]
fn session_names_are_checked() {
    for ok in ["a", "A-1", "a.b_c", "0x", "cards-api-gl-mr-1228"] {
        assert!(valid_session_name(ok), "{ok}");
    }
    for bad in [
        "", "-x", ".lock", ".x", "_x", "../x", "a/b", "a b", "ż", "a\n",
    ] {
        assert!(!valid_session_name(bad), "{bad:?}");
    }
}

#[test]
fn records_dir_prefers_an_absolute_xdg_state_home() {
    let d =
        |x: Option<&str>, h: Option<&str>| records_dir_from(x.map(Into::into), h.map(Into::into));
    assert_eq!(
        d(Some("/s"), Some("/h")),
        Some(PathBuf::from("/s/agent-sessions"))
    );
    let home = Some(PathBuf::from("/h/.local/state/agent-sessions"));
    assert_eq!(d(Some(""), Some("/h")), home);
    assert_eq!(d(Some("rel"), Some("/h")), home);
    assert_eq!(d(None, Some("/h")), home);
    assert_eq!(d(None, None), None);
}
