use super::*;

use crate::kv::{Bucket, Kv};
use std::time::{Duration, Instant};

#[path = "../common/nats.rs"]
#[allow(clippy::duplicate_mod)]
mod nats;

#[test]
fn finish_retrying_waits_for_nats_and_writes_once_it_is_back() {
    let mut server = nats::NatsServer::start();
    let url = server.url();
    server.stop();
    let start = Instant::now();
    let restarter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        server.restart();
        server
    });
    let got = finish_retrying(&url, "w", ResultKind::Result, "done", |_| true);
    assert_eq!(got, Ok(Some(1)));
    assert!(start.elapsed() >= Duration::from_secs(3));
    let server = restarter.join().unwrap();
    let kv = Kv::connect(&server.url()).unwrap();
    let entry = kv.get(Bucket::Results, "w.1").unwrap().unwrap();
    let body: serde_json::Value = serde_json::from_slice(&entry.value).unwrap();
    assert_eq!(body["text"], "done");
    assert_eq!(body["kind"], "result");
    let keys: Vec<String> = kv
        .keys(Bucket::Results)
        .unwrap()
        .into_iter()
        .filter(|k| k.starts_with("w."))
        .collect();
    assert_eq!(keys, vec!["w.1".to_string()]);
}

#[test]
fn finish_makes_a_single_attempt_and_fails_fast_when_nats_is_down() {
    let mut server = nats::NatsServer::start();
    let records = Records::connect(&server.url()).unwrap();
    server.stop();
    let start = Instant::now();
    let got = finish(&records, "w", ResultKind::Result, "done", |_| true);
    assert!(got.is_err(), "{got:?}");
    assert!(start.elapsed() < Duration::from_secs(20), "no retry");
}
