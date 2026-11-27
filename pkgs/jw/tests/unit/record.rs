use super::*;

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "../common/nats.rs"]
#[allow(clippy::duplicate_mod)]
mod nats;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf, nats::NatsServer);

impl Tmp {
    fn new() -> Tmp {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = fs::canonicalize(env::temp_dir()).unwrap();
        let dir = base.join(format!("jw-record-test-{}-{}", process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Tmp(dir, nats::NatsServer::start())
    }

    fn records(&self) -> Records {
        let mut records = Records::connect(&self.1.url()).unwrap();
        records.dir = self.0.join("agent-sessions");
        records
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn always(_: &mut Record) -> bool {
    true
}

fn result_file(r: &Records, name: &str, seq: u64) -> PathBuf {
    r.dir.join("results").join(name).join(format!("{seq}.json"))
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn old(p: &Path) {
    let t = SystemTime::now() - Duration::from_secs((RECORD_DAYS + 1) * 86400);
    fs::File::open(p).unwrap().set_modified(t).unwrap();
}

#[test]
fn long_lived_is_mate_or_headless() {
    let plain = Record::default();
    assert!(!plain.long_lived());
    let task = Record {
        kind: Some("task".to_string()),
        ..Record::default()
    };
    assert!(!task.long_lived());
    let mate = Record {
        kind: Some(MATE_KIND.to_string()),
        ..Record::default()
    };
    assert!(mate.long_lived());
    let headless = Record {
        headless: true,
        ..Record::default()
    };
    assert!(headless.long_lived());
}

#[test]
fn new_fields_are_omitted_when_unset() {
    let json = serde_json::to_value(Record::default()).unwrap();
    for key in ["headless", "turn_base", "result_seq"] {
        assert!(json.get(key).is_none(), "{key}: {json}");
    }
}

#[test]
fn new_fields_round_trip() {
    let t = Tmp::new();
    let r = t.records();
    r.update("w", |rec| {
        rec.headless = true;
        rec.turn_base = Some(4);
        rec.result_seq = 4;
        true
    })
    .unwrap();
    let rec = r.read("w").unwrap().unwrap();
    assert!(rec.headless);
    assert_eq!(rec.turn_base, Some(4));
    assert_eq!(rec.result_seq, 4);
    let json: Value =
        serde_json::from_str(&fs::read_to_string(r.dir.join("w.json")).unwrap()).unwrap();
    assert_eq!(json["headless"], true);
    assert_eq!(json["turn_base"], 4);
    assert_eq!(json["result_seq"], 4);
}

#[test]
fn new_fields_of_the_wrong_type_are_ignored() {
    let t = Tmp::new();
    let r = t.records();
    fs::create_dir_all(&r.dir).unwrap();
    fs::write(
        r.dir.join("w.json"),
        r#"{"kind":"task","headless":"yes","turn_base":"x","result_seq":"y"}"#,
    )
    .unwrap();
    let rec = r.read("w").unwrap().unwrap();
    assert_eq!(rec.kind.as_deref(), Some("task"));
    assert!(!rec.headless);
    assert_eq!(rec.turn_base, None);
    assert_eq!(rec.result_seq, 0);
}

#[test]
fn clear_marks_closes_the_turn() {
    let mut rec = Record {
        turn_base: Some(2),
        result_seq: 2,
        ..Record::default()
    };
    rec.clear_marks();
    assert_eq!(rec.turn_base, None);
    assert_eq!(rec.result_seq, 2);
}

#[test]
fn result_kind_serializes_lowercase() {
    assert_eq!(
        serde_json::to_string(&ResultKind::Result).unwrap(),
        "\"result\""
    );
    assert_eq!(
        serde_json::to_string(&ResultKind::Died).unwrap(),
        "\"died\""
    );
}

#[test]
fn write_result_numbers_from_one() {
    let t = Tmp::new();
    let r = t.records();
    assert_eq!(
        r.write_result("w", ResultKind::Result, "first", always),
        Ok(Some(1))
    );
    assert_eq!(
        r.write_result("w", ResultKind::Died, "second", always),
        Ok(Some(2))
    );
    let rec = r.read("w").unwrap().unwrap();
    assert_eq!(rec.result_seq, 2);
    assert_eq!(rec.turn_base, None);
    let file: Value =
        serde_json::from_str(&fs::read_to_string(result_file(&r, "w", 1)).unwrap()).unwrap();
    assert_eq!(file["seq"], 1);
    assert_eq!(file["kind"], "result");
    assert_eq!(file["text"], "first");
    assert!(file["at"].as_f64().unwrap() > 1.0e9);
    let file: Value =
        serde_json::from_str(&fs::read_to_string(result_file(&r, "w", 2)).unwrap()).unwrap();
    assert_eq!(file["seq"], 2);
    assert_eq!(file["kind"], "died");
    assert_eq!(file["text"], "second");
}

#[test]
fn write_result_modes() {
    let t = Tmp::new();
    let r = t.records();
    r.write_result("w", ResultKind::Result, "x", always)
        .unwrap();
    assert_eq!(mode(&r.dir.join("results/w")), 0o700);
    assert_eq!(mode(&result_file(&r, "w", 1)), 0o600);
}

#[test]
fn write_result_closes_the_open_turn() {
    let t = Tmp::new();
    let r = t.records();
    r.update("w", |rec| {
        rec.headless = true;
        rec.turn_base = Some(0);
        true
    })
    .unwrap();
    r.write_result("w", ResultKind::Result, "x", always)
        .unwrap();
    let rec = r.read("w").unwrap().unwrap();
    assert_eq!(rec.turn_base, None);
    assert_eq!(rec.result_seq, 1);
    assert!(rec.headless);
}

#[test]
fn write_result_guard_false_writes_nothing() {
    let t = Tmp::new();
    let r = t.records();
    r.update("w", |rec| {
        rec.turn_base = Some(0);
        true
    })
    .unwrap();
    let got = r.write_result("w", ResultKind::Died, "x", |rec| {
        rec.status = Some("idle".to_string());
        false
    });
    assert_eq!(got, Ok(None));
    assert!(!result_file(&r, "w", 1).exists());
    assert!(r.results("w").unwrap().is_empty());
    let rec = r.read("w").unwrap().unwrap();
    assert_eq!(rec.result_seq, 0);
    assert_eq!(rec.turn_base, Some(0));
    assert_eq!(rec.status, None);
}

#[test]
fn write_result_guard_sees_the_record_and_its_changes_are_saved() {
    let t = Tmp::new();
    let r = t.records();
    r.update("w", |rec| {
        rec.title = Some("T".to_string());
        rec.turn_base = Some(0);
        true
    })
    .unwrap();
    let got = r.write_result("w", ResultKind::Result, "x", |rec| {
        let open = rec.turn_base == Some(rec.result_seq) && rec.title.as_deref() == Some("T");
        rec.status = Some("idle".to_string());
        open
    });
    assert_eq!(got, Ok(Some(1)));
    let rec = r.read("w").unwrap().unwrap();
    assert_eq!(rec.status.as_deref(), Some("idle"));
    assert_eq!(rec.title.as_deref(), Some("T"));
    assert_eq!(rec.result_seq, 1);
    assert_eq!(rec.turn_base, None);
}

#[test]
fn write_result_caps_long_text() {
    let t = Tmp::new();
    let r = t.records();
    let marker = "\n[truncated]";
    let exact = "a".repeat(262144);
    r.write_result("w", ResultKind::Result, &exact, always)
        .unwrap();
    let long = format!("{}{}", "b".repeat(262144), "c".repeat(100));
    r.write_result("w", ResultKind::Result, &long, always)
        .unwrap();
    let results = r.results("w").unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].text, exact);
    assert_eq!(
        results[1].text.chars().count(),
        262144 + marker.chars().count()
    );
    assert!(results[1].text.starts_with(&"b".repeat(262144)));
    assert!(results[1].text.ends_with(marker));
    assert!(!results[1].text.contains("cc"));
}

#[test]
fn write_result_caps_characters_not_bytes() {
    let t = Tmp::new();
    let r = t.records();
    let long = "ż".repeat(262144 + 5);
    r.write_result("w", ResultKind::Result, &long, always)
        .unwrap();
    let text = &r.results("w").unwrap()[0].text;
    assert!(text.starts_with(&"ż".repeat(262144)));
    assert!(text.ends_with("\n[truncated]"));
    assert_eq!(
        text.chars().count(),
        262144 + "\n[truncated]".chars().count()
    );
}

#[test]
fn write_result_invalid_name() {
    let t = Tmp::new();
    let r = t.records();
    assert!(r
        .write_result("x/y", ResultKind::Result, "x", always)
        .is_err());
    assert!(r
        .write_result("..", ResultKind::Result, "x", always)
        .is_err());
}

#[test]
fn results_are_by_number_and_unacked_only() {
    let t = Tmp::new();
    let r = t.records();
    assert!(r.results("w").unwrap().is_empty());
    for i in 1..=11 {
        r.write_result("w", ResultKind::Result, &format!("t{i}"), always)
            .unwrap();
    }
    let seqs: Vec<u64> = r.results("w").unwrap().iter().map(|x| x.seq).collect();
    assert_eq!(seqs, (1..=11).collect::<Vec<u64>>());
    let first = &r.results("w").unwrap()[0];
    assert_eq!(first.kind, ResultKind::Result);
    assert_eq!(first.text, "t1");
    r.ack("w", 9).unwrap();
    let seqs: Vec<u64> = r.results("w").unwrap().iter().map(|x| x.seq).collect();
    assert_eq!(seqs, [10, 11]);
}

#[test]
fn results_are_per_session() {
    let t = Tmp::new();
    let r = t.records();
    r.write_result("a", ResultKind::Result, "ta", always)
        .unwrap();
    r.write_result("b", ResultKind::Died, "tb", always).unwrap();
    assert_eq!(r.results("a").unwrap().len(), 1);
    assert_eq!(r.results("a").unwrap()[0].text, "ta");
    assert_eq!(r.results("b").unwrap()[0].kind, ResultKind::Died);
    assert!(r.results("c").unwrap().is_empty());
}

#[test]
fn ack_deletes_up_to_seq() {
    let t = Tmp::new();
    let r = t.records();
    for _ in 0..3 {
        r.write_result("w", ResultKind::Result, "x", always)
            .unwrap();
    }
    assert_eq!(r.ack("w", 2), Ok(()));
    assert!(!result_file(&r, "w", 1).exists());
    assert!(!result_file(&r, "w", 2).exists());
    assert!(result_file(&r, "w", 3).exists());
    assert_eq!(r.ack("w", 2), Ok(()));
    assert_eq!(r.ack("w", 1), Ok(()));
    assert_eq!(r.ack("w", 3), Ok(()));
    assert!(r.results("w").unwrap().is_empty());
}

#[test]
fn ack_beyond_the_last_result_fails_and_deletes_nothing() {
    let t = Tmp::new();
    let r = t.records();
    r.write_result("w", ResultKind::Result, "x", always)
        .unwrap();
    r.write_result("w", ResultKind::Result, "y", always)
        .unwrap();
    assert!(r.ack("w", 3).is_err());
    assert_eq!(r.results("w").unwrap().len(), 2);
}

#[test]
fn numbers_are_not_reused_after_an_ack() {
    let t = Tmp::new();
    let r = t.records();
    r.write_result("w", ResultKind::Result, "x", always)
        .unwrap();
    r.write_result("w", ResultKind::Result, "y", always)
        .unwrap();
    r.ack("w", 2).unwrap();
    assert_eq!(
        r.write_result("w", ResultKind::Result, "z", always),
        Ok(Some(3))
    );
    assert_eq!(r.results("w").unwrap()[0].seq, 3);
    assert_eq!(r.read("w").unwrap().unwrap().result_seq, 3);
}

#[test]
fn results_dirs_are_not_records() {
    let t = Tmp::new();
    let r = t.records();
    r.write_result("w", ResultKind::Result, "x", always)
        .unwrap();
    let names: Vec<String> = r.load_all().unwrap().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["w"]);
}

fn write_old(r: &Records, name: &str, json: &str) {
    fs::create_dir_all(&r.dir).unwrap();
    let p = r.dir.join(format!("{name}.json"));
    fs::write(&p, json).unwrap();
    old(&p);
    let d = r.dir.join("results").join(name);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("1.json"), "{}").unwrap();
    r.enqueue(name, "pending").unwrap();
}

fn trigger_prune(r: &Records) {
    r.update("fresh", always).unwrap();
}

#[test]
fn prune_removes_results_and_queue_with_the_record() {
    let t = Tmp::new();
    let r = t.records();
    write_old(&r, "gone", r#"{"kind":"task"}"#);
    write_old(&r, "other", r#"{"kind":"task"}"#);
    fs::write(r.dir.join("other.json"), r#"{"kind":"task"}"#).unwrap();
    trigger_prune(&r);
    assert!(!r.dir.join("gone.json").exists());
    assert!(!r.dir.join("results/gone").exists());
    assert!(r.queued("gone").unwrap().is_empty());
    assert!(r.dir.join("other.json").exists());
    assert!(r.dir.join("results/other").exists());
    assert_eq!(r.queued("other").unwrap().len(), 1);
    assert!(r.dir.join("fresh.json").exists());
}

#[test]
fn prune_keeps_a_live_headless_record_and_its_files() {
    let t = Tmp::new();
    let r = t.records();
    write_old(&r, "w", r#"{"kind":"task","headless":true}"#);
    trigger_prune(&r);
    assert!(r.dir.join("w.json").exists());
    assert!(r.dir.join("results/w/1.json").exists());
    assert_eq!(r.queued("w").unwrap().len(), 1);
}

#[test]
fn prune_removes_a_reaped_headless_record_with_its_files() {
    let t = Tmp::new();
    let r = t.records();
    write_old(&r, "w", r#"{"kind":"task","headless":true,"reaped":1}"#);
    trigger_prune(&r);
    assert!(!r.dir.join("w.json").exists());
    assert!(!r.dir.join("results/w").exists());
    assert!(r.queued("w").unwrap().is_empty());
}

#[test]
fn prune_still_keeps_a_live_mate_and_removes_a_reaped_one() {
    let t = Tmp::new();
    let r = t.records();
    write_old(&r, "m", r#"{"kind":"mate"}"#);
    write_old(&r, "n", r#"{"kind":"mate","reaped":1}"#);
    trigger_prune(&r);
    assert!(r.dir.join("m.json").exists());
    assert!(!r.dir.join("n.json").exists());
}

#[test]
fn orphan_result_file_is_not_overwritten_and_can_be_acked() {
    let t = Tmp::new();
    let r = t.records();
    r.update("w", |rec| {
        rec.headless = true;
        true
    })
    .unwrap();
    let d = r.dir.join("results/w");
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("1.json"),
        r#"{"seq":1,"kind":"result","text":"orphan","at":1.0}"#,
    )
    .unwrap();
    assert_eq!(r.read("w").unwrap().unwrap().result_seq, 0);
    let got = r.write_result("w", ResultKind::Result, "next", always);
    assert_eq!(got, Ok(Some(2)));
    assert!(d.join("1.json").exists());
    assert!(d.join("2.json").exists());
    assert_eq!(r.ack("w", 1), Ok(()));
    assert!(!d.join("1.json").exists());
}

#[test]
fn unit_name() {
    assert_eq!(crate::worker::unit("w"), "jw-worker-w");
    assert_eq!(crate::worker::unit("a.b_c-1"), "jw-worker-a.b_c-1");
}

mod queue {
    use crate::kv::{Bucket, Kv};
    use crate::record::Records;

    use super::nats::{closed_url, NatsServer};

    struct Q {
        _server: NatsServer,
        r: Records,
        kv: Kv,
    }

    fn setup() -> Q {
        let server = NatsServer::start();
        let r = Records::connect(&server.url()).expect("connect");
        let kv = Kv::connect(&server.url()).expect("connect");
        Q {
            _server: server,
            r,
            kv,
        }
    }

    fn texts(q: &Q, name: &str) -> Vec<String> {
        q.r.queued(name)
            .unwrap()
            .into_iter()
            .map(|e| e.text)
            .collect()
    }

    fn key(name: &str, id: &str) -> String {
        format!("{name}.{id}")
    }

    fn is_id(id: &str) -> bool {
        match id.split_once('-') {
            Some((t, pid)) => {
                t.len() == 20
                    && t.bytes().all(|b| b.is_ascii_digit())
                    && !pid.is_empty()
                    && pid.bytes().all(|b| b.is_ascii_digit())
            }
            None => false,
        }
    }

    #[test]
    fn connect_failure_names_the_url() {
        let url = closed_url();
        let err = Records::connect(&url).err().expect("must fail");
        assert!(
            err.starts_with(&format!("cannot reach NATS at {url}: ")),
            "{err}"
        );
    }

    #[test]
    fn enqueue_then_queued_lists_the_entry() {
        let q = setup();
        let (id, rev) = q.r.enqueue("w", "hello").unwrap();
        assert!(is_id(&id), "{id}");
        let got = q.r.queued("w").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, id);
        assert_eq!(got[0].text, "hello");
        assert_eq!(got[0].revision, rev);
        let e = q.kv.get(Bucket::Queue, &key("w", &id)).unwrap().unwrap();
        assert_eq!(e.value, b"hello");
        assert_eq!(e.revision, rev);
    }

    #[test]
    fn queued_of_an_unknown_session_is_empty() {
        let q = setup();
        assert!(q.r.queued("nobody").unwrap().is_empty());
    }

    #[test]
    fn ids_are_ordered_and_queued_follows_them() {
        let q = setup();
        let mut ids = vec![];
        for i in 0..5 {
            ids.push(q.r.enqueue("w", &format!("m{i}")).unwrap().0);
        }
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        let mut unique = ids.clone();
        unique.dedup();
        assert_eq!(unique.len(), 5);
        let got = q.r.queued("w").unwrap();
        assert_eq!(got.iter().map(|e| e.id.clone()).collect::<Vec<_>>(), ids);
        assert_eq!(texts(&q, "w"), ["m0", "m1", "m2", "m3", "m4"]);
        let revs: Vec<u64> = got.iter().map(|e| e.revision).collect();
        assert!(revs.windows(2).all(|w| w[0] < w[1]), "{revs:?}");
    }

    #[test]
    fn queued_orders_by_id_not_by_insertion() {
        let q = setup();
        q.kv.put(
            Bucket::Queue,
            &key("w", "00000000000000000002-1"),
            b"second",
        )
        .unwrap();
        q.kv.put(Bucket::Queue, &key("w", "00000000000000000001-1"), b"first")
            .unwrap();
        assert_eq!(texts(&q, "w"), ["first", "second"]);
    }

    #[test]
    fn enqueue_rejects_an_empty_message() {
        let q = setup();
        assert_eq!(
            q.r.enqueue("w", "").err().unwrap(),
            "cannot queue the message for w: empty message"
        );
        assert!(q.r.queued("w").unwrap().is_empty());
    }

    #[test]
    fn enqueue_size_limit() {
        let q = setup();
        let err = q.r.enqueue("w", &"a".repeat(786433)).err().unwrap();
        assert_eq!(
            err,
            "cannot queue the message for w: it is larger than 786432 bytes"
        );
        assert!(q.r.queued("w").unwrap().is_empty());
        let (id, _) = q.r.enqueue("w", &"a".repeat(786432)).unwrap();
        assert_eq!(q.r.queued("w").unwrap()[0].id, id);
    }

    #[test]
    fn queued_leaves_out_other_sessions_with_a_dotted_name() {
        let q = setup();
        q.r.enqueue("a", "mine").unwrap();
        q.r.enqueue("a.b", "theirs").unwrap();
        q.kv.put(Bucket::Queue, &key("a.b", "00000000000000000001-1"), b"x")
            .unwrap();
        assert_eq!(texts(&q, "a"), ["mine"]);
        assert_eq!(texts(&q, "a.b"), ["x", "theirs"]);
    }

    #[test]
    fn queued_skips_cancelled_entries_and_reads_non_utf8_lossily() {
        let q = setup();
        q.kv.put(Bucket::Queue, &key("w", "00000000000000000001-1"), b"")
            .unwrap();
        q.kv.put(
            Bucket::Queue,
            &key("w", "00000000000000000002-1"),
            &[b'a', 0xff, b'b'],
        )
        .unwrap();
        assert_eq!(texts(&q, "w"), ["a\u{fffd}b"]);
    }

    #[test]
    fn claim_succeeds_once_then_the_old_revision_conflicts() {
        let q = setup();
        let (id, _) = q.r.enqueue("w", "go").unwrap();
        let entry = q.r.queued("w").unwrap().remove(0);
        assert_eq!(q.r.claim("w", &entry), Ok(true));
        assert_eq!(q.r.claim("w", &entry), Ok(false));
        let after = q.r.queued("w").unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, id);
        assert_eq!(after[0].text, "go");
        assert!(after[0].revision > entry.revision);
        assert_eq!(q.r.claim("w", &after[0]), Ok(true));
    }

    #[test]
    fn cancel_unclaimed_removes_the_entry() {
        let q = setup();
        let (id, rev) = q.r.enqueue("w", "go").unwrap();
        assert_eq!(q.r.cancel("w", &id, rev), Ok(true));
        assert!(q.r.queued("w").unwrap().is_empty());
        assert!(q.kv.get(Bucket::Queue, &key("w", &id)).unwrap().is_none());
        assert!(!q.kv.keys(Bucket::Queue).unwrap().contains(&key("w", &id)));
    }

    #[test]
    fn cancel_after_claim_conflicts_and_keeps_the_entry() {
        let q = setup();
        let (id, rev) = q.r.enqueue("w", "go").unwrap();
        let entry = q.r.queued("w").unwrap().remove(0);
        assert_eq!(q.r.claim("w", &entry), Ok(true));
        assert_eq!(q.r.cancel("w", &id, rev), Ok(false));
        assert_eq!(texts(&q, "w"), ["go"]);
    }

    #[test]
    fn claim_after_cancel_is_false() {
        let q = setup();
        let (id, rev) = q.r.enqueue("w", "go").unwrap();
        let entry = q.r.queued("w").unwrap().remove(0);
        assert_eq!(q.r.cancel("w", &id, rev), Ok(true));
        assert_eq!(q.r.claim("w", &entry), Ok(false));
        assert!(q.r.queued("w").unwrap().is_empty());
    }

    #[test]
    fn claim_after_a_hand_written_cancel_marker_is_false() {
        let q = setup();
        let (id, _) = q.r.enqueue("w", "go").unwrap();
        let entry = q.r.queued("w").unwrap().remove(0);
        q.kv.put(Bucket::Queue, &key("w", &id), b"").unwrap();
        assert_eq!(q.r.claim("w", &entry), Ok(false));
        assert!(q.r.queued("w").unwrap().is_empty());
    }

    #[test]
    fn dequeue_removes_the_entry_and_tolerates_absence() {
        let q = setup();
        let (id, _) = q.r.enqueue("w", "one").unwrap();
        let (other, _) = q.r.enqueue("w", "two").unwrap();
        assert_eq!(q.r.dequeue("w", &id), Ok(()));
        assert_eq!(texts(&q, "w"), ["two"]);
        assert_eq!(q.r.dequeue("w", &id), Ok(()));
        assert_eq!(q.r.dequeue("nobody", "00000000000000000001-1"), Ok(()));
        assert_eq!(q.r.queued("w").unwrap()[0].id, other);
    }

    #[test]
    fn dequeue_of_a_claimed_entry_removes_it() {
        let q = setup();
        let (id, _) = q.r.enqueue("w", "go").unwrap();
        let entry = q.r.queued("w").unwrap().remove(0);
        assert_eq!(q.r.claim("w", &entry), Ok(true));
        assert_eq!(q.r.dequeue("w", &id), Ok(()));
        assert!(q.r.queued("w").unwrap().is_empty());
        assert!(q.kv.get(Bucket::Queue, &key("w", &id)).unwrap().is_none());
    }

    #[test]
    fn dequeue_does_not_touch_a_prefix_session() {
        let q = setup();
        let (id, _) = q.r.enqueue("a", "mine").unwrap();
        q.r.enqueue("a.b", "theirs").unwrap();
        assert_eq!(q.r.dequeue("a", &id), Ok(()));
        assert_eq!(texts(&q, "a.b"), ["theirs"]);
    }
}
