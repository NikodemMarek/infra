use std::env;
use std::fs::{self, DirBuilder, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use crate::procs::home;

const RECORD_MAX: u64 = 65536;
const RECORD_DAYS: u64 = 7;
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// The record of an agent session, ADR-056/063. Records written by older versions are read field by field: a field of
/// the wrong type is ignored, not the record.
#[derive(Default, Serialize)]
pub struct Record {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mrs: Vec<String>,
    /// The MRs of mrs the session was told are merged.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub merged_mrs: Vec<String>,
    /// Workspaces the session created (`jw session worktree`); `jw reap` drops them once it is done.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub worktrees: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notified: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaped: Option<f64>,
}

impl Record {
    fn from_value(value: Value) -> Option<Record> {
        let Value::Object(map) = value else {
            return None;
        };
        let text = |key: &str| map.get(key).and_then(Value::as_str).map(str::to_string);
        let number = |key: &str| map.get(key).and_then(Value::as_f64);
        let texts = |key: &str| -> Vec<String> {
            map.get(key)
                .and_then(Value::as_array)
                .map(|v| {
                    v.iter()
                        .filter_map(|u| u.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let integer = |key: &str| {
            let value = map.get(key)?;
            value.as_i64().or_else(|| value.as_f64().map(|f| f as i64))
        };
        Some(Record {
            kind: text("kind"),
            workspace: text("workspace"),
            title: text("title"),
            description: text("description"),
            url: text("url"),
            mrs: texts("mrs"),
            merged_mrs: texts("merged_mrs"),
            worktrees: texts("worktrees"),
            status: text("status"),
            status_at: number("status_at"),
            waiting_for: text("waiting_for"),
            cache_expires_at: integer("cache_expires_at"),
            notified: number("notified"),
            reaped: number("reaped"),
        })
    }

    /// Takes the fields other has set.
    pub fn merge(&mut self, other: Record) {
        self.kind = other.kind.or(self.kind.take());
        self.workspace = other.workspace.or(self.workspace.take());
        self.title = other.title.or(self.title.take());
        self.description = other.description.or(self.description.take());
        self.url = other.url.or(self.url.take());
    }

    /// Drops what says whether the session is finished and reaped; a new session start does.
    pub fn clear_marks(&mut self) {
        self.status = None;
        self.status_at = None;
        self.waiting_for = None;
        self.cache_expires_at = None;
        self.notified = None;
        self.reaped = None;
    }
}

const WRITE_FAILED: &str = "cannot write the session record: ";

/// The message of a failed record update for callers that only warn: every kind of error says the record was not written.
pub fn write_error(e: &str) -> String {
    if e.starts_with(WRITE_FAILED) {
        e.to_string()
    } else {
        format!("{WRITE_FAILED}{e}")
    }
}

pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The session records, $XDG_STATE_HOME/agent-sessions/<name>.json.
pub struct Records {
    dir: PathBuf,
}

impl Records {
    pub fn new() -> Records {
        let state = env::var("XDG_STATE_HOME")
            .ok()
            .filter(|s| Path::new(s).is_absolute())
            .unwrap_or_else(|| format!("{}/.local/state", home()));
        Records {
            dir: PathBuf::from(state).join("agent-sessions"),
        }
    }

    /// The record of session name, None if it is missing or not a plain, small JSON object file.
    pub fn read(&self, name: &str) -> Option<Record> {
        if !valid_name(name) {
            return None;
        }
        // O_NONBLOCK: opening a FIFO must not wait for a writer
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.dir.join(format!("{name}.json")))
            .ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut data = Vec::new();
        file.take(RECORD_MAX + 1).read_to_end(&mut data).ok()?;
        if data.len() as u64 > RECORD_MAX {
            return None;
        }
        Record::from_value(serde_json::from_slice(&data).ok()?)
    }

    /// The records, sorted by name.
    pub fn load_all(&self) -> Vec<(String, Record)> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return vec![];
        };
        let mut files: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        files.sort();
        files
            .iter()
            .filter_map(|f| {
                let name = f.strip_suffix(".json")?;
                Some((name.to_string(), self.read(name)?))
            })
            .collect()
    }

    /// Under the records lock calls change on the record of session name (empty if there is none) and writes it
    /// unless that returns false; returns the record.
    pub fn update(
        &self,
        name: &str,
        change: impl FnOnce(&mut Record) -> bool,
    ) -> Result<Record, String> {
        if !valid_name(name) {
            return Err(format!("invalid session name: '{name}'"));
        }
        let fail = |e: io::Error| format!("{WRITE_FAILED}{e}");
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(fail)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.dir.join(".lock"))
            .map_err(fail)?;
        // polling: a writer that died holding the lock cannot hang the callers (agents, hooks) for good
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err("session records are busy".to_string());
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                Err(TryLockError::Error(e)) => return Err(fail(e)),
            }
        }
        let path = self.dir.join(format!("{name}.json"));
        let existed = fs::symlink_metadata(&path).is_ok();
        let mut record = self.read(name).unwrap_or_default();
        if !change(&mut record) {
            return Ok(record);
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let tmp = self.dir.join(format!(".tmp-{}-{nanos}", process::id()));
        let written = serde_json::to_string(&record)
            .map_err(io::Error::from)
            .and_then(|json| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&tmp)?
                    .write_all(json.as_bytes())?;
                fs::rename(&tmp, &path)
            });
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(fail(e));
        }
        if !existed {
            self.prune();
        }
        Ok(record)
    }

    /// Deletes the records untouched for RECORD_DAYS.
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let cutoff = SystemTime::now() - Duration::from_secs(RECORD_DAYS * 86400);
        for entry in entries.flatten() {
            if entry.file_name() == ".lock" {
                continue;
            }
            let path = entry.path();
            let old = fs::symlink_metadata(&path)
                .is_ok_and(|m| m.is_file() && m.modified().is_ok_and(|t| t < cutoff));
            if old {
                let _ = fs::remove_file(&path);
            }
        }
    }

    /// Records the state of session name and when its prompt cache goes cold (None: unknown, removed); a done session
    /// keeps its status, and an unchanged record is not rewritten.
    pub fn set_status(
        &self,
        name: &str,
        state: &str,
        waiting_for: Option<String>,
        cache_expires_at: Option<i64>,
    ) -> Result<(), String> {
        self.update(name, |r| {
            let mut changed = false;
            let unchanged = r.status.as_deref() == Some(state) && r.waiting_for == waiting_for;
            if r.status.as_deref() != Some("done") && !unchanged {
                r.status = Some(state.to_string());
                r.status_at = Some(now());
                r.waiting_for = waiting_for;
                changed = true;
            }
            if r.cache_expires_at != cache_expires_at {
                r.cache_expires_at = cache_expires_at;
                changed = true;
            }
            changed
        })
        .map(|_| ())
    }
}
