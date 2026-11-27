use super::*;

use crate::kv::{Bucket, Kv};
use serde_json::Value;
use std::cell::Cell;
use std::time::{Duration, Instant};

#[path = "../common/nats.rs"]
#[allow(clippy::duplicate_mod)]
mod nats;

struct Tmp(nats::NatsServer);

impl Tmp {
    fn new() -> Tmp {
        Tmp(nats::NatsServer::start())
    }

    fn records(&self) -> Records {
        Records::connect(&self.0.url()).unwrap()
    }

    fn kv(&self) -> Kv {
        Kv::connect(&self.0.url()).unwrap()
    }
}

fn always(_: &mut Record) -> bool {
    true
}

fn put(kv: &Kv, b: Bucket, key: &str, value: &str) {
    kv.put(b, key, value.as_bytes()).unwrap();
}

fn raw(kv: &Kv, b: Bucket, key: &str) -> Option<Value> {
    kv.get(b, key)
        .unwrap()
        .map(|e| serde_json::from_slice(&e.value).unwrap())
}

fn result_keys(kv: &Kv, name: &str) -> Vec<String> {
    let prefix = format!("{name}.");
    kv.keys(Bucket::Results)
        .unwrap()
        .into_iter()
        .filter(|k| k.starts_with(&prefix))
        .collect()
}

fn queue_keys(kv: &Kv, name: &str) -> Vec<String> {
    let prefix = format!("{name}.");
    kv.keys(Bucket::Queue)
        .unwrap()
        .into_iter()
        .filter(|k| k.starts_with(&prefix))
        .collect()
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
    let json = raw(&t.kv(), Bucket::Sessions, "w").unwrap();
    assert_eq!(json["headless"], true);
    assert_eq!(json["turn_base"], 4);
    assert_eq!(json["result_seq"], 4);
}

#[test]
fn new_fields_of_the_wrong_type_are_ignored() {
    let t = Tmp::new();
    let r = t.records();
    put(
        &t.kv(),
        Bucket::Sessions,
        "w",
        r#"{"kind":"task","headless":"yes","turn_base":"x","result_seq":"y"}"#,
    );
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
fn valid_name_rules() {
    for ok in ["a", "a.b", "s-1127-130155", "A_b-1.c", "0"] {
        assert!(valid_name(ok), "{ok}");
    }
    for bad in ["", "a..b", "a.", "..", ".a", "-a", "x/y", "a b", "a*"] {
        assert!(!valid_name(bad), "{bad}");
    }
}

#[test]
fn invalid_names_read_none_and_update_fails() {
    let t = Tmp::new();
    let r = t.records();
    for bad in ["a..b", "a.", "x/y"] {
        assert!(r.read(bad).unwrap().is_none(), "{bad}");
        assert_eq!(
            r.update(bad, always).err().unwrap(),
            format!("invalid session name: '{bad}'")
        );
        assert!(r
            .write_result(bad, ResultKind::Result, "x", always)
            .is_err());
    }
}

#[test]
fn read_of_absent_oversized_or_non_object_values_is_none() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    assert!(r.read("absent").unwrap().is_none());
    let big = format!(r#"{{"title":"{}"}}"#, "a".repeat(65536));
    put(&kv, Bucket::Sessions, "big", &big);
    put(&kv, Bucket::Sessions, "arr", "[1]");
    put(&kv, Bucket::Sessions, "str", "\"x\"");
    put(&kv, Bucket::Sessions, "junk", "not json");
    for n in ["big", "arr", "str", "junk"] {
        assert!(r.read(n).unwrap().is_none(), "{n}");
    }
}

#[test]
fn load_all_is_sorted_and_skips_invalid_names_and_unreadable_values() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    put(&kv, Bucket::Sessions, "b.c", r#"{"kind":"task"}"#);
    put(&kv, Bucket::Sessions, "a", r#"{"kind":"task"}"#);
    put(&kv, Bucket::Sessions, "c", r#"{"kind":"task"}"#);
    put(&kv, Bucket::Sessions, "junk", "not json");
    put(&kv, Bucket::Sessions, "arr", "[]");
    put(&kv, Bucket::Sessions, "-bad", r#"{"kind":"task"}"#);
    let names: Vec<String> = r.load_all().unwrap().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["a", "b.c", "c"]);
}

#[test]
fn update_false_writes_nothing() {
    let t = Tmp::new();
    let r = t.records();
    let got = r
        .update("w", |rec| {
            rec.title = Some("x".to_string());
            false
        })
        .unwrap();
    assert_eq!(got.title.as_deref(), Some("x"));
    assert!(r.read("w").unwrap().is_none());
    assert!(t.kv().get(Bucket::Sessions, "w").unwrap().is_none());
}

#[test]
fn update_replaces_an_unreadable_value() {
    let t = Tmp::new();
    let r = t.records();
    put(&t.kv(), Bucket::Sessions, "w", "not json");
    r.update("w", |rec| {
        rec.title = Some("T".to_string());
        true
    })
    .unwrap();
    assert_eq!(r.read("w").unwrap().unwrap().title.as_deref(), Some("T"));
}

#[test]
fn update_rejects_a_record_over_the_size_limit_and_keeps_the_old_one() {
    let t = Tmp::new();
    let r = t.records();
    r.update("w", |rec| {
        rec.title = Some("small".to_string());
        true
    })
    .unwrap();
    let err = r
        .update("w", |rec| {
            rec.description = Some("d".repeat(70000));
            true
        })
        .err()
        .unwrap();
    assert_eq!(
        err,
        "cannot write the session record: it is larger than 65536 bytes"
    );
    let rec = r.read("w").unwrap().unwrap();
    assert_eq!(rec.title.as_deref(), Some("small"));
    assert!(rec.description.is_none());
    let err = r
        .update("fresh", |rec| {
            rec.description = Some("d".repeat(70000));
            true
        })
        .err()
        .unwrap();
    assert!(err.contains("larger than 65536 bytes"), "{err}");
    assert!(r.read("fresh").unwrap().is_none());
}

#[test]
fn update_reruns_the_change_after_a_conflict_on_an_existing_record() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    r.update("w", |rec| {
        rec.title = Some("orig".to_string());
        true
    })
    .unwrap();
    let calls = Cell::new(0);
    let got = r
        .update("w", |rec| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                put(&kv, Bucket::Sessions, "w", r#"{"kind":"concurrent"}"#);
            }
            rec.description = Some(format!("call {}", calls.get()));
            true
        })
        .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(got.kind.as_deref(), Some("concurrent"));
    assert_eq!(got.description.as_deref(), Some("call 2"));
    assert_eq!(got.title, None);
    let rec = r.read("w").unwrap().unwrap();
    assert_eq!(rec.kind.as_deref(), Some("concurrent"));
    assert_eq!(rec.description.as_deref(), Some("call 2"));
}

#[test]
fn update_reruns_the_change_after_a_conflict_on_a_new_record() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    let calls = Cell::new(0);
    r.update("w", |rec| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            put(&kv, Bucket::Sessions, "w", r#"{"kind":"concurrent"}"#);
        }
        rec.title = Some("mine".to_string());
        true
    })
    .unwrap();
    assert_eq!(calls.get(), 2);
    let rec = r.read("w").unwrap().unwrap();
    assert_eq!(rec.kind.as_deref(), Some("concurrent"));
    assert_eq!(rec.title.as_deref(), Some("mine"));
}

#[test]
fn update_gives_up_when_records_stay_busy() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    put(&kv, Bucket::Sessions, "w", r#"{"kind":"task"}"#);
    let start = Instant::now();
    let err = r
        .update("w", |rec| {
            put(&kv, Bucket::Sessions, "w", r#"{"kind":"task"}"#);
            rec.title = Some("x".to_string());
            true
        })
        .err()
        .unwrap();
    assert_eq!(err, "session records are busy");
    let took = start.elapsed();
    assert!(took >= Duration::from_secs(4), "{took:?}");
    assert!(took < Duration::from_secs(20), "{took:?}");
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
    let kv = t.kv();
    let file = raw(&kv, Bucket::Results, "w.1").unwrap();
    assert_eq!(file["seq"], 1);
    assert_eq!(file["kind"], "result");
    assert_eq!(file["text"], "first");
    assert!(file["at"].as_f64().unwrap() > 1.0e9);
    let file = raw(&kv, Bucket::Results, "w.2").unwrap();
    assert_eq!(file["seq"], 2);
    assert_eq!(file["kind"], "died");
    assert_eq!(file["text"], "second");
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
    assert!(result_keys(&t.kv(), "w").is_empty());
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
fn write_result_after_a_concurrent_record_change_reruns_and_keeps_one_result() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    r.update("w", |rec| {
        rec.headless = true;
        true
    })
    .unwrap();
    let calls = Cell::new(0);
    let got = r.write_result("w", ResultKind::Result, "x", |_| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            put(&kv, Bucket::Sessions, "w", r#"{"kind":"concurrent"}"#);
        }
        true
    });
    assert_eq!(got, Ok(Some(1)));
    assert_eq!(calls.get(), 2);
    assert_eq!(result_keys(&kv, "w"), ["w.1"]);
    let rec = r.read("w").unwrap().unwrap();
    assert_eq!(rec.kind.as_deref(), Some("concurrent"));
    assert_eq!(rec.result_seq, 1);
}

#[test]
fn write_result_guard_true_then_false_on_rerun_leaves_no_result() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    r.update("w", |rec| {
        rec.headless = true;
        true
    })
    .unwrap();
    let calls = Cell::new(0);
    let got = r.write_result("w", ResultKind::Result, "x", |_| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            put(&kv, Bucket::Sessions, "w", r#"{"kind":"concurrent"}"#);
            true
        } else {
            false
        }
    });
    assert_eq!(got, Ok(None));
    assert_eq!(calls.get(), 2);
    assert!(result_keys(&kv, "w").is_empty());
    assert_eq!(r.read("w").unwrap().unwrap().result_seq, 0);
}

#[test]
fn write_result_over_the_record_size_limit_fails_and_leaves_no_result() {
    let t = Tmp::new();
    let r = t.records();
    let got = r.write_result("w", ResultKind::Result, "x", |rec| {
        rec.description = Some("d".repeat(70000));
        true
    });
    assert_eq!(
        got,
        Err("cannot write the session record: it is larger than 65536 bytes".to_string())
    );
    assert!(result_keys(&t.kv(), "w").is_empty());
}

#[test]
fn write_result_caps_long_text() {
    let t = Tmp::new();
    let r = t.records();
    let marker = "\n[truncated]";
    let exact = "a".repeat(262144);
    r.write_result("w", ResultKind::Result, &exact, always)
        .unwrap();
    r.write_result("w", ResultKind::Result, &"a".repeat(300000), always)
        .unwrap();
    let results = r.results("w").unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].text, exact);
    assert_eq!(results[1].text, format!("{exact}{marker}"));
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
fn write_result_keeps_the_result_json_within_the_value_limit() {
    let t = Tmp::new();
    let r = t.records();
    let long = "\u{1}".repeat(200000);
    r.write_result("w", ResultKind::Result, &long, always)
        .unwrap();
    let entry = t.kv().get(Bucket::Results, "w.1").unwrap().unwrap();
    assert!(entry.value.len() <= 786432, "{}", entry.value.len());
    let text = r.results("w").unwrap().remove(0).text;
    assert!(text.ends_with("\n[truncated]"));
    let body = text.strip_suffix("\n[truncated]").unwrap();
    assert!(long.starts_with(body));
    assert!(body.len() < long.len());
}

#[test]
fn write_result_skips_past_an_orphan_key() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    r.update("w", |rec| {
        rec.result_seq = 1;
        true
    })
    .unwrap();
    put(
        &kv,
        Bucket::Results,
        "w.3",
        r#"{"seq":3,"kind":"result","text":"orphan","at":1.0}"#,
    );
    assert_eq!(
        r.write_result("w", ResultKind::Result, "next", always),
        Ok(Some(4))
    );
    assert_eq!(result_keys(&kv, "w"), ["w.3", "w.4"]);
    assert_eq!(raw(&kv, Bucket::Results, "w.3").unwrap()["text"], "orphan");
}

#[test]
fn result_seqs_are_numeric_and_own_keys_only() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    for key in ["w.10", "w.9", "w.x", "w.1.2", "w2.1", "w.11"] {
        put(&kv, Bucket::Results, key, "{}");
    }
    assert_eq!(r.result_seqs("w"), Ok(vec![9, 10, 11]));
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
fn results_skip_unreadable_values() {
    let t = Tmp::new();
    let r = t.records();
    r.write_result("w", ResultKind::Result, "ok", always)
        .unwrap();
    put(&t.kv(), Bucket::Results, "w.2", "not json");
    let seqs: Vec<u64> = r.results("w").unwrap().iter().map(|x| x.seq).collect();
    assert_eq!(seqs, [1]);
}

#[test]
fn results_are_per_session_and_a_dotted_name_is_its_own() {
    let t = Tmp::new();
    let r = t.records();
    r.write_result("a", ResultKind::Result, "ta", always)
        .unwrap();
    r.write_result("b", ResultKind::Died, "tb", always).unwrap();
    r.write_result("a.b", ResultKind::Result, "tab", always)
        .unwrap();
    put(
        &t.kv(),
        Bucket::Results,
        "a.b.3",
        r#"{"seq":3,"kind":"result","text":"x","at":1.0}"#,
    );
    assert_eq!(r.results("a").unwrap().len(), 1);
    assert_eq!(r.results("a").unwrap()[0].text, "ta");
    assert_eq!(r.result_seqs("a"), Ok(vec![1]));
    assert_eq!(r.result_seqs("a.b"), Ok(vec![1, 3]));
    assert_eq!(r.results("b").unwrap()[0].kind, ResultKind::Died);
    assert!(r.results("c").unwrap().is_empty());
    assert_eq!(r.read("a").unwrap().unwrap().result_seq, 1);
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
    assert_eq!(result_keys(&t.kv(), "w"), ["w.3"]);
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
    assert_eq!(r.ack("w", 3), Err("no result 3 of w".to_string()));
    assert_eq!(r.results("w").unwrap().len(), 2);
}

#[test]
fn ack_without_a_record_fails() {
    let t = Tmp::new();
    let r = t.records();
    assert_eq!(
        r.ack("nobody", 1),
        Err("no session record nobody".to_string())
    );
}

#[test]
fn ack_counts_an_orphan_key_and_leaves_a_dotted_session_alone() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    r.update("a", always).unwrap();
    put(&kv, Bucket::Results, "a.1", "{}");
    put(&kv, Bucket::Results, "a.b.1", "{}");
    assert_eq!(r.ack("a", 1), Ok(()));
    assert_eq!(result_keys(&kv, "a"), ["a.b.1"]);
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

fn plain(t: &Tmp, name: &str, record: &str) {
    let kv = t.kv();
    put(&kv, Bucket::Sessions, name, record);
    put(&kv, Bucket::Results, &format!("{name}.1"), "{}");
    t.records().enqueue(name, "pending").unwrap();
}

fn pruned(r: &Records, at: f64) -> Vec<String> {
    let mut names = Vec::new();
    r.prune(at, |n| names.push(n.to_string())).unwrap();
    names.sort();
    names
}

fn later() -> f64 {
    now() + 8.0 * 86400.0
}

#[test]
fn prune_removes_record_results_and_queue_of_an_old_record() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    plain(&t, "gone", r#"{"kind":"task"}"#);
    plain(&t, "mate", r#"{"kind":"mate"}"#);
    assert_eq!(pruned(&r, later()), vec!["gone".to_string()]);
    assert!(kv.get(Bucket::Sessions, "gone").unwrap().is_none());
    assert!(result_keys(&kv, "gone").is_empty());
    assert!(queue_keys(&kv, "gone").is_empty());
    assert!(kv.get(Bucket::Sessions, "mate").unwrap().is_some());
    assert_eq!(result_keys(&kv, "mate"), ["mate.1"]);
    assert_eq!(queue_keys(&kv, "mate").len(), 1);
}

#[test]
fn prune_of_a_fresh_record_does_nothing() {
    let t = Tmp::new();
    let r = t.records();
    plain(&t, "w", r#"{"kind":"task"}"#);
    assert_eq!(pruned(&r, now()), Vec::<String>::new());
    assert!(r.read("w").unwrap().is_some());
}

#[test]
fn prune_keeps_a_live_headless_record_and_its_keys() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    plain(&t, "w", r#"{"kind":"task","headless":true}"#);
    assert_eq!(pruned(&r, later()), Vec::<String>::new());
    assert!(r.read("w").unwrap().is_some());
    assert_eq!(result_keys(&kv, "w"), ["w.1"]);
    assert_eq!(r.queued("w").unwrap().len(), 1);
}

#[test]
fn prune_removes_reaped_headless_and_mate_records_sorted() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    plain(&t, "w", r#"{"kind":"task","headless":true,"reaped":1}"#);
    plain(&t, "n", r#"{"kind":"mate","reaped":1}"#);
    plain(&t, "m", r#"{"kind":"mate"}"#);
    assert_eq!(pruned(&r, later()), vec!["n".to_string(), "w".to_string()]);
    assert!(result_keys(&kv, "w").is_empty());
    assert!(queue_keys(&kv, "w").is_empty());
    assert!(r.read("m").unwrap().is_some());
}

#[test]
fn prune_removes_an_unreadable_old_value() {
    let t = Tmp::new();
    let r = t.records();
    put(&t.kv(), Bucket::Sessions, "junk", "not json");
    assert_eq!(pruned(&r, later()), vec!["junk".to_string()]);
    assert!(t.kv().get(Bucket::Sessions, "junk").unwrap().is_none());
}

#[test]
fn prune_of_a_session_leaves_the_dotted_session_with_its_keys() {
    let t = Tmp::new();
    let r = t.records();
    let kv = t.kv();
    plain(&t, "a", r#"{"kind":"task"}"#);
    plain(&t, "a.b", r#"{"kind":"task","headless":true}"#);
    assert_eq!(pruned(&r, later()), vec!["a".to_string()]);
    assert!(r.read("a.b").unwrap().is_some());
    assert_eq!(result_keys(&kv, "a.b"), ["a.b.1"]);
    assert_eq!(queue_keys(&kv, "a.b").len(), 1);
    assert!(kv.get(Bucket::Results, "a.1").unwrap().is_none());
    assert!(queue_keys(&kv, "a").iter().all(|k| k.starts_with("a.b.")));
}

#[test]
fn update_after_prune_creates_a_fresh_record() {
    let t = Tmp::new();
    let r = t.records();
    plain(&t, "w", r#"{"kind":"task","title":"old"}"#);
    pruned(&r, later());
    let rec = r
        .update("w", |rec| {
            rec.description = Some("new".to_string());
            true
        })
        .unwrap();
    assert_eq!(rec.title, None);
    let rec = r.read("w").unwrap().unwrap();
    assert_eq!(rec.description.as_deref(), Some("new"));
    assert_eq!(rec.title, None);
}

#[test]
fn update_does_not_prune() {
    let t = Tmp::new();
    let r = t.records();
    plain(&t, "old", r#"{"kind":"task"}"#);
    r.update("fresh", always).unwrap();
    assert!(r.read("old").unwrap().is_some());
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
