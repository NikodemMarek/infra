use super::{Bucket, Kv, KvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[path = "../common/nats.rs"]
mod nats;
use nats::{closed_url, NatsServer};

use Bucket::{Queue, Results, Sessions};

fn setup() -> (NatsServer, Kv) {
    let server = NatsServer::start();
    let kv = Kv::connect(&server.url()).expect("connect");
    (server, kv)
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn value(kv: &Kv, b: Bucket, key: &str) -> Option<Vec<u8>> {
    kv.get(b, key).unwrap().map(|e| e.value)
}

#[test]
fn bucket_names() {
    assert_eq!(Sessions.name(), "sessions");
    assert_eq!(Results.name(), "results");
    assert_eq!(Queue.name(), "queue");
}

#[test]
fn error_display() {
    assert_eq!(KvError::Conflict.to_string(), "conflict");
    assert_eq!(KvError::Other("boom".into()).to_string(), "boom");
}

#[test]
fn connect_failure_names_the_url() {
    let url = closed_url();
    let start = Instant::now();
    let err = Kv::connect(&url).err().expect("connect must fail");
    assert!(
        err.starts_with(&format!("cannot reach NATS at {url}: ")),
        "{err}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn connect_without_jetstream_fails() {
    let server = NatsServer::start_without_jetstream();
    let url = server.url();
    let start = Instant::now();
    let err = Kv::connect(&url).err().expect("connect must fail");
    assert!(
        err.starts_with(&format!("NATS at {url} has no usable ")),
        "{err}"
    );
    assert!(err.contains(" bucket: "), "{err}");
    assert!(
        start.elapsed() < Duration::from_secs(8),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn connect_twice_uses_the_existing_buckets() {
    let (server, kv) = setup();
    kv.put(Sessions, "s", b"one").unwrap();
    let again = Kv::connect(&server.url()).expect("second connect");
    assert_eq!(value(&again, Sessions, "s").as_deref(), Some(&b"one"[..]));
}

#[test]
fn data_survives_a_server_restart() {
    let (mut server, kv) = setup();
    kv.put(Results, "w.1", b"kept").unwrap();
    drop(kv);
    server.restart();
    let kv = Kv::connect(&server.url()).expect("connect after restart");
    assert_eq!(value(&kv, Results, "w.1").as_deref(), Some(&b"kept"[..]));
}

#[test]
fn get_absent_is_none() {
    let (_s, kv) = setup();
    for b in [Sessions, Results, Queue] {
        assert!(kv.get(b, "nothing").unwrap().is_none());
    }
}

#[test]
fn get_returns_value_revision_and_time() {
    let (_s, kv) = setup();
    let before = now();
    let rev = kv.put(Sessions, "a", b"hello").unwrap();
    let after = now();
    let entry = kv.get(Sessions, "a").unwrap().expect("entry");
    assert_eq!(entry.value, b"hello");
    assert_eq!(entry.revision, rev);
    assert!(
        entry.at >= before - 2.0 && entry.at <= after + 2.0,
        "{}",
        entry.at
    );
}

#[test]
fn get_stores_binary_and_empty_values() {
    let (_s, kv) = setup();
    let bytes: Vec<u8> = (0..=255).collect();
    kv.put(Queue, "a.1", &bytes).unwrap();
    assert_eq!(value(&kv, Queue, "a.1"), Some(bytes));
    kv.put(Queue, "a.2", b"").unwrap();
    assert_eq!(value(&kv, Queue, "a.2"), Some(Vec::new()));
}

#[test]
fn buckets_are_separate() {
    let (_s, kv) = setup();
    kv.put(Sessions, "a", b"s").unwrap();
    assert!(kv.get(Results, "a").unwrap().is_none());
    assert!(kv.get(Queue, "a").unwrap().is_none());
    assert_eq!(kv.keys(Sessions).unwrap(), ["a"]);
    assert!(kv.keys(Results).unwrap().is_empty());
}

#[test]
fn get_deleted_is_none() {
    let (_s, kv) = setup();
    let rev = kv.put(Sessions, "a", b"x").unwrap();
    kv.delete(Sessions, "a", rev).unwrap();
    assert!(kv.get(Sessions, "a").unwrap().is_none());
}

#[test]
fn get_purged_is_none() {
    let (_s, kv) = setup();
    kv.put(Results, "a.1", b"x").unwrap();
    kv.purge(Results, "a.1").unwrap();
    assert!(kv.get(Results, "a.1").unwrap().is_none());
}

#[test]
fn create_succeeds_on_absent_key() {
    let (_s, kv) = setup();
    let rev = kv.create(Sessions, "a", b"v").unwrap();
    let entry = kv.get(Sessions, "a").unwrap().unwrap();
    assert_eq!(entry.value, b"v");
    assert_eq!(entry.revision, rev);
}

#[test]
fn create_conflicts_on_live_key() {
    let (_s, kv) = setup();
    kv.create(Sessions, "a", b"first").unwrap();
    assert_eq!(kv.create(Sessions, "a", b"second"), Err(KvError::Conflict));
    assert_eq!(value(&kv, Sessions, "a").as_deref(), Some(&b"first"[..]));
}

#[test]
fn create_conflicts_with_a_second_connection() {
    let (server, kv) = setup();
    let other = Kv::connect(&server.url()).unwrap();
    kv.create(Queue, "a.1", b"mine").unwrap();
    assert_eq!(other.create(Queue, "a.1", b"yours"), Err(KvError::Conflict));
    assert_eq!(value(&other, Queue, "a.1").as_deref(), Some(&b"mine"[..]));
}

#[test]
fn create_conflicts_on_a_put_key_too() {
    let (_s, kv) = setup();
    kv.put(Sessions, "a", b"x").unwrap();
    assert_eq!(kv.create(Sessions, "a", b"y"), Err(KvError::Conflict));
}

#[test]
fn create_succeeds_after_delete() {
    let (_s, kv) = setup();
    let rev = kv.create(Sessions, "a", b"old").unwrap();
    kv.delete(Sessions, "a", rev).unwrap();
    let new = kv.create(Sessions, "a", b"new").unwrap();
    assert!(new > rev);
    assert_eq!(value(&kv, Sessions, "a").as_deref(), Some(&b"new"[..]));
}

#[test]
fn create_succeeds_after_purge() {
    let (_s, kv) = setup();
    kv.create(Results, "a.1", b"old").unwrap();
    kv.purge(Results, "a.1").unwrap();
    kv.create(Results, "a.1", b"new").unwrap();
    assert_eq!(value(&kv, Results, "a.1").as_deref(), Some(&b"new"[..]));
}

#[test]
fn update_with_latest_revision_succeeds() {
    let (_s, kv) = setup();
    let rev = kv.create(Sessions, "a", b"one").unwrap();
    let new = kv.update(Sessions, "a", b"two", rev).unwrap();
    assert!(new > rev);
    let entry = kv.get(Sessions, "a").unwrap().unwrap();
    assert_eq!(entry.value, b"two");
    assert_eq!(entry.revision, new);
}

#[test]
fn update_chain_follows_the_new_revision() {
    let (_s, kv) = setup();
    let r1 = kv.create(Sessions, "a", b"1").unwrap();
    let r2 = kv.update(Sessions, "a", b"2", r1).unwrap();
    let r3 = kv.update(Sessions, "a", b"3", r2).unwrap();
    assert!(r1 < r2 && r2 < r3);
}

#[test]
fn update_with_stale_revision_conflicts() {
    let (_s, kv) = setup();
    let r1 = kv.create(Sessions, "a", b"1").unwrap();
    kv.update(Sessions, "a", b"2", r1).unwrap();
    assert_eq!(kv.update(Sessions, "a", b"3", r1), Err(KvError::Conflict));
    assert_eq!(value(&kv, Sessions, "a").as_deref(), Some(&b"2"[..]));
}

#[test]
fn update_after_a_concurrent_put_conflicts() {
    let (server, kv) = setup();
    let other = Kv::connect(&server.url()).unwrap();
    let rev = kv.create(Sessions, "a", b"1").unwrap();
    other.put(Sessions, "a", b"theirs").unwrap();
    assert_eq!(
        kv.update(Sessions, "a", b"mine", rev),
        Err(KvError::Conflict)
    );
    assert_eq!(value(&kv, Sessions, "a").as_deref(), Some(&b"theirs"[..]));
}

#[test]
fn update_of_absent_key_conflicts() {
    let (_s, kv) = setup();
    assert_eq!(kv.update(Sessions, "a", b"x", 1), Err(KvError::Conflict));
    assert!(kv.get(Sessions, "a").unwrap().is_none());
}

#[test]
fn update_of_deleted_key_conflicts() {
    let (_s, kv) = setup();
    let rev = kv.create(Sessions, "a", b"x").unwrap();
    kv.delete(Sessions, "a", rev).unwrap();
    assert_eq!(kv.update(Sessions, "a", b"y", rev), Err(KvError::Conflict));
    assert!(kv.get(Sessions, "a").unwrap().is_none());
}

#[test]
fn update_of_purged_key_conflicts() {
    let (_s, kv) = setup();
    let rev = kv.create(Results, "a.1", b"x").unwrap();
    kv.purge(Results, "a.1").unwrap();
    assert_eq!(kv.update(Results, "a.1", b"y", rev), Err(KvError::Conflict));
    assert!(kv.get(Results, "a.1").unwrap().is_none());
}

#[test]
fn put_is_unconditional() {
    let (_s, kv) = setup();
    kv.put(Sessions, "a", b"1").unwrap();
    kv.put(Sessions, "a", b"2").unwrap();
    assert_eq!(value(&kv, Sessions, "a").as_deref(), Some(&b"2"[..]));
}

#[test]
fn put_after_delete_revives_the_key() {
    let (_s, kv) = setup();
    let rev = kv.put(Sessions, "a", b"1").unwrap();
    kv.delete(Sessions, "a", rev).unwrap();
    kv.put(Sessions, "a", b"2").unwrap();
    assert_eq!(value(&kv, Sessions, "a").as_deref(), Some(&b"2"[..]));
}

#[test]
fn revisions_strictly_increase_within_a_bucket() {
    let (_s, kv) = setup();
    let mut last = 0;
    for i in 0..10 {
        let rev = match i % 3 {
            0 => kv.put(Sessions, "k", b"x").unwrap(),
            1 => kv.put(Sessions, &format!("p{i}"), b"y").unwrap(),
            _ => kv.create(Sessions, &format!("c{i}"), b"z").unwrap(),
        };
        assert!(rev > last, "{rev} after {last}");
        last = rev;
    }
}

#[test]
fn delete_with_latest_revision_removes_the_key() {
    let (_s, kv) = setup();
    let rev = kv.create(Sessions, "a", b"x").unwrap();
    kv.create(Sessions, "b", b"y").unwrap();
    kv.delete(Sessions, "a", rev).unwrap();
    assert!(kv.get(Sessions, "a").unwrap().is_none());
    assert_eq!(kv.keys(Sessions).unwrap(), ["b"]);
    kv.create(Sessions, "a", b"again").unwrap();
}

#[test]
fn delete_with_stale_revision_conflicts() {
    let (_s, kv) = setup();
    let r1 = kv.create(Sessions, "a", b"1").unwrap();
    kv.update(Sessions, "a", b"2", r1).unwrap();
    assert_eq!(kv.delete(Sessions, "a", r1), Err(KvError::Conflict));
    assert_eq!(value(&kv, Sessions, "a").as_deref(), Some(&b"2"[..]));
    assert_eq!(kv.keys(Sessions).unwrap(), ["a"]);
}

#[test]
fn delete_of_absent_key_conflicts() {
    let (_s, kv) = setup();
    assert_eq!(kv.delete(Sessions, "a", 1), Err(KvError::Conflict));
}

#[test]
fn delete_of_deleted_key_conflicts() {
    let (_s, kv) = setup();
    let rev = kv.create(Sessions, "a", b"x").unwrap();
    kv.delete(Sessions, "a", rev).unwrap();
    assert_eq!(kv.delete(Sessions, "a", rev), Err(KvError::Conflict));
}

#[test]
fn purge_of_one_key_leaves_no_trace() {
    let (_s, kv) = setup();
    kv.put(Results, "a.1", b"x").unwrap();
    kv.put(Results, "a.2", b"y").unwrap();
    kv.purge(Results, "a.1").unwrap();
    assert!(kv.get(Results, "a.1").unwrap().is_none());
    assert_eq!(kv.keys(Results).unwrap(), ["a.2"]);
    kv.create(Results, "a.1", b"z").unwrap();
}

#[test]
fn purge_of_history_removes_every_revision() {
    let (_s, kv) = setup();
    for i in 0..4 {
        kv.put(Sessions, "a", format!("v{i}").as_bytes()).unwrap();
    }
    kv.purge(Sessions, "a").unwrap();
    assert!(kv.get(Sessions, "a").unwrap().is_none());
    assert!(kv.keys(Sessions).unwrap().is_empty());
    kv.create(Sessions, "a", b"fresh").unwrap();
}

#[test]
fn purge_pattern_selects_exactly_one_level() {
    let (_s, kv) = setup();
    for key in ["a.1", "a.2", "a.b.1", "ab.1", "a"] {
        kv.put(Results, key, b"x").unwrap();
    }
    kv.purge(Results, "a.*").unwrap();
    assert!(kv.get(Results, "a.1").unwrap().is_none());
    assert!(kv.get(Results, "a.2").unwrap().is_none());
    assert!(kv.get(Results, "a.b.1").unwrap().is_some());
    assert!(kv.get(Results, "ab.1").unwrap().is_some());
    assert!(kv.get(Results, "a").unwrap().is_some());
    assert_eq!(kv.keys(Results).unwrap(), ["a", "a.b.1", "ab.1"]);
    kv.create(Results, "a.1", b"again").unwrap();
    kv.create(Results, "a.2", b"again").unwrap();
}

#[test]
fn purge_pattern_for_the_nested_name() {
    let (_s, kv) = setup();
    for key in ["a.1", "a.b.1", "a.b.2"] {
        kv.put(Queue, key, b"x").unwrap();
    }
    kv.purge(Queue, "a.b.*").unwrap();
    assert_eq!(kv.keys(Queue).unwrap(), ["a.1"]);
}

#[test]
fn purge_with_no_match_is_ok() {
    let (_s, kv) = setup();
    kv.purge(Results, "nothing.*").unwrap();
    kv.purge(Results, "nothing").unwrap();
    kv.put(Results, "other.1", b"x").unwrap();
    kv.purge(Results, "nothing.*").unwrap();
    assert_eq!(kv.keys(Results).unwrap(), ["other.1"]);
}

#[test]
fn purge_only_touches_its_bucket() {
    let (_s, kv) = setup();
    kv.put(Results, "a.1", b"r").unwrap();
    kv.put(Queue, "a.1", b"q").unwrap();
    kv.purge(Results, "a.*").unwrap();
    assert!(kv.get(Queue, "a.1").unwrap().is_some());
}

#[test]
fn keys_are_sorted_and_live_only() {
    let (_s, kv) = setup();
    assert!(kv.keys(Sessions).unwrap().is_empty());
    for key in ["b", "a", "c", "a.b", "B", "dead", "gone"] {
        kv.put(Sessions, key, b"x").unwrap();
    }
    let rev = kv.get(Sessions, "dead").unwrap().unwrap().revision;
    kv.delete(Sessions, "dead", rev).unwrap();
    kv.purge(Sessions, "gone").unwrap();
    assert_eq!(kv.keys(Sessions).unwrap(), ["B", "a", "a.b", "b", "c"]);
}

#[test]
fn keys_lists_each_key_once_despite_history() {
    let (_s, kv) = setup();
    for i in 0..3 {
        kv.put(Sessions, "a", format!("{i}").as_bytes()).unwrap();
    }
    assert_eq!(kv.keys(Sessions).unwrap(), ["a"]);
}

#[test]
fn values_exactly_at_the_limit_are_accepted() {
    let (_s, kv) = setup();
    for (b, limit) in [(Sessions, 65536), (Results, 786432), (Queue, 786432)] {
        let big = vec![b'x'; limit];
        kv.put(b, "put", &big).unwrap();
        let rev = kv.create(b, "create", &big).unwrap();
        kv.update(b, "create", &big, rev).unwrap();
        assert_eq!(kv.get(b, "create").unwrap().unwrap().value.len(), limit);
    }
}

#[test]
fn purge_with_a_broad_pattern_is_rejected_and_removes_nothing() {
    let (_s, kv) = setup();
    for key in ["a", "a.1", "a.b.1", "b.1"] {
        kv.put(Results, key, b"x").unwrap();
    }
    for pattern in [">", "*", "a.>", ""] {
        assert!(kv.purge(Results, pattern).is_err(), "{pattern:?}");
        assert_eq!(
            kv.keys(Results).unwrap(),
            ["a", "a.1", "a.b.1", "b.1"],
            "{pattern:?}"
        );
    }
    kv.purge(Results, "a.*").unwrap();
    kv.purge(Results, "a").unwrap();
    assert_eq!(kv.keys(Results).unwrap(), ["a.b.1", "b.1"]);
}

#[test]
fn operations_fail_in_bounded_time_when_the_server_is_down() {
    let (mut server, kv) = setup();
    kv.put(Sessions, "a", b"x").unwrap();
    server.stop();
    let start = Instant::now();
    assert!(kv.get(Sessions, "a").is_err());
    assert!(matches!(
        kv.create(Sessions, "b", b"x"),
        Err(KvError::Other(_))
    ));
    assert!(kv.put(Sessions, "c", b"x").is_err());
    assert!(kv.keys(Sessions).is_err());
    assert!(
        start.elapsed() < Duration::from_secs(25),
        "{:?}",
        start.elapsed()
    );
}
