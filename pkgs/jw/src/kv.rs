#![allow(dead_code)]

use std::fmt;
use std::future::IntoFuture;
use std::pin::pin;
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::kv::{self, CreateErrorKind, Operation, Store, UpdateErrorKind};
use async_nats::jetstream::stream::StorageType;
use futures_util::StreamExt;
use tokio::runtime::{Builder, Runtime};
use tokio::time::timeout;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bucket {
    Sessions,
    Results,
    Queue,
}

impl Bucket {
    pub fn name(self) -> &'static str {
        match self {
            Bucket::Sessions => "sessions",
            Bucket::Results => "results",
            Bucket::Queue => "queue",
        }
    }

    fn config(self) -> kv::Config {
        let (history, max_value_size) = match self {
            Bucket::Sessions => (5, 65536),
            Bucket::Results | Bucket::Queue => (1, 786432),
        };
        kv::Config {
            bucket: self.name().to_string(),
            history,
            max_value_size: max_value_size + HEADER_HEADROOM,
            storage: StorageType::File,
            ..Default::default()
        }
    }
}

pub struct Entry {
    pub value: Vec<u8>,
    pub revision: u64,
    pub at: f64,
}

#[derive(Debug, PartialEq)]
pub enum KvError {
    Conflict,
    Other(String),
}

impl fmt::Display for KvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KvError::Conflict => f.write_str("conflict"),
            KvError::Other(text) => f.write_str(text),
        }
    }
}

pub fn nats_url() -> String {
    match std::env::var("NATS_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => "nats://127.0.0.1:4222".to_string(),
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const OP_TIMEOUT: Duration = Duration::from_secs(5);
const HEADER_HEADROOM: i32 = 1024;
const BUCKETS: [Bucket; 3] = [Bucket::Sessions, Bucket::Results, Bucket::Queue];

pub struct Kv {
    rt: Runtime,
    stores: Vec<Store>,
}

impl Kv {
    pub fn connect(url: &str) -> Result<Kv, String> {
        let rt = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| format!("cannot start the NATS runtime: {e}"))?;
        let cannot_reach = |e: String| format!("cannot reach NATS at {url}: {e}");
        let client = rt
            .block_on(async {
                timeout(
                    CONNECT_TIMEOUT,
                    async_nats::ConnectOptions::new()
                        .connection_timeout(CONNECT_TIMEOUT)
                        .connect(url),
                )
                .await
            })
            .map_err(|_| cannot_reach("timed out".to_string()))?
            .map_err(|e| cannot_reach(e.to_string()))?;
        let js = {
            let _rt = rt.enter();
            jetstream::new(client)
        };
        let mut stores = Vec::new();
        for b in BUCKETS {
            let unusable =
                |e: String| format!("NATS at {url} has no usable {} bucket: {e}", b.name());
            let store = rt
                .block_on(async { timeout(OP_TIMEOUT, open_bucket(&js, b)).await })
                .map_err(|_| unusable("timed out".to_string()))?
                .map_err(unusable)?;
            stores.push(store);
        }
        Ok(Kv { rt, stores })
    }

    fn store(&self, b: Bucket) -> &Store {
        &self.stores[b as usize]
    }

    fn block<T>(&self, fut: impl IntoFuture<Output = T>) -> Result<T, String> {
        self.rt
            .block_on(async { timeout(OP_TIMEOUT, fut).await })
            .map_err(|_| "timed out".to_string())
    }

    pub fn get(&self, b: Bucket, key: &str) -> Result<Option<Entry>, String> {
        let entry = self
            .block(self.store(b).entry(key))?
            .map_err(|e| e.to_string())?;
        Ok(entry
            .filter(|e| e.operation == Operation::Put)
            .map(|e| Entry {
                value: e.value.to_vec(),
                revision: e.revision,
                at: e.created.unix_timestamp_nanos() as f64 / 1e9,
            }))
    }

    pub fn create(&self, b: Bucket, key: &str, value: &[u8]) -> Result<u64, KvError> {
        match self
            .block(self.store(b).create(key, value.to_vec().into()))
            .map_err(KvError::Other)?
        {
            Ok(revision) => Ok(revision),
            Err(e) if matches!(e.kind(), CreateErrorKind::AlreadyExists) => Err(KvError::Conflict),
            Err(e) => Err(KvError::Other(e.to_string())),
        }
    }

    pub fn update(
        &self,
        b: Bucket,
        key: &str,
        value: &[u8],
        revision: u64,
    ) -> Result<u64, KvError> {
        match self
            .block(self.store(b).update(key, value.to_vec().into(), revision))
            .map_err(KvError::Other)?
        {
            Ok(revision) => Ok(revision),
            Err(e) if matches!(e.kind(), UpdateErrorKind::WrongLastRevision) => {
                Err(KvError::Conflict)
            }
            Err(e) => Err(KvError::Other(e.to_string())),
        }
    }

    pub fn put(&self, b: Bucket, key: &str, value: &[u8]) -> Result<u64, String> {
        self.block(self.store(b).put(key, value.to_vec().into()))?
            .map_err(|e| e.to_string())
    }

    pub fn delete(&self, b: Bucket, key: &str, revision: u64) -> Result<(), KvError> {
        let failure = match self
            .block(self.store(b).delete_expect_revision(key, Some(revision)))
            .map_err(KvError::Other)?
        {
            Ok(()) => return Ok(()),
            Err(e) => e.to_string(),
        };
        match self.get(b, key).map_err(KvError::Other)? {
            Some(current) if current.revision == revision => Err(KvError::Other(failure)),
            _ => Err(KvError::Conflict),
        }
    }

    pub fn purge(&self, b: Bucket, pattern: &str) -> Result<(), String> {
        if !valid_purge_pattern(pattern) {
            return Err(format!("invalid purge pattern {pattern:?}"));
        }
        let store = self.store(b);
        let subject = format!("{}{pattern}", store.prefix);
        self.block(store.stream.purge().filter(subject))?
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn keys(&self, b: Bucket) -> Result<Vec<String>, String> {
        let store = self.store(b);
        let mut keys = self.block(async {
            let stream = store.keys().await.map_err(|e| e.to_string())?;
            let mut stream = pin!(stream);
            let mut keys = Vec::new();
            while let Some(key) = stream.next().await {
                keys.push(key.map_err(|e| e.to_string())?);
            }
            Ok::<_, String>(keys)
        })??;
        keys.sort();
        Ok(keys)
    }
}

fn valid_purge_pattern(pattern: &str) -> bool {
    let key = pattern.strip_suffix(".*").unwrap_or(pattern);
    !key.is_empty()
        && key.split('.').all(|token| {
            !token.is_empty()
                && token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | '_' | '='))
        })
}

async fn open_bucket(js: &jetstream::Context, b: Bucket) -> Result<Store, String> {
    if let Ok(store) = js.get_key_value(b.name()).await {
        return Ok(store);
    }
    js.create_key_value(b.config())
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "../tests/unit/kv.rs"]
mod unit_tests;
