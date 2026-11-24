use std::collections::BTreeMap;
use std::env;
use std::fs::{self, DirBuilder, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::procs::home;

const RECORD_MAX: u64 = 65536;
const RECORD_DAYS: u64 = 7;
const LOCK_WAIT: Duration = Duration::from_secs(5);
const RESULT_TEXT_MAX: usize = 262144;
/// A result file: the text with every character escaped as \u00XX, plus the other fields.
const RESULT_FILE_MAX: u64 = RESULT_TEXT_MAX as u64 * 6 + 4096;
/// The record kind of a first mate (`jw mate`): `jw reap` never stops it and pruning keeps its record.
pub const MATE_KIND: &str = "mate";

/// What the poller knows of one MR: the commit whose failed pipeline it reported, and the keys of the comments it has
/// seen (empty: baselined).
#[derive(Default, Serialize, Clone, Debug, PartialEq)]
pub struct MrMark {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_sha: Option<String>,
    pub seen: Vec<String>,
}

/// How a headless worker's turn ended.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ResultKind {
    Result,
    Died,
}

/// The result of one turn of a headless worker, a numbered file kept until the supervisor acks it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TurnResult {
    pub seq: u64,
    pub kind: ResultKind,
    pub text: String,
    pub at: f64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

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
    /// The session this one reports to; none: the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reports_to: Option<String>,
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
    /// The poller's mark per MR url.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub mr_marks: BTreeMap<String, MrMark>,
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
    /// A headless worker: each turn is one `claude -p` run, ADR-083.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub headless: bool,
    /// result_seq when the open turn started; a turn is open iff this equals Some(result_seq).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_base: Option<u64>,
    /// How many results the session has written.
    #[serde(skip_serializing_if = "is_zero")]
    pub result_seq: u64,
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
        let mr_marks: BTreeMap<String, MrMark> = map
            .get("mr_marks")
            .and_then(Value::as_object)
            .map(|marks| {
                marks
                    .iter()
                    .filter_map(|(url, mark)| {
                        let mark = mark.as_object()?;
                        let seen: Vec<String> = mark
                            .get("seen")
                            .and_then(Value::as_array)
                            .map(|v| {
                                v.iter()
                                    .filter_map(|k| k.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let failed_sha = mark
                            .get("failed_sha")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        Some((url.clone(), MrMark { failed_sha, seen }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Record {
            kind: text("kind"),
            workspace: text("workspace"),
            title: text("title"),
            description: text("description"),
            reports_to: text("reports_to"),
            url: text("url"),
            mrs: texts("mrs"),
            merged_mrs: texts("merged_mrs"),
            worktrees: texts("worktrees"),
            mr_marks,
            status: text("status"),
            status_at: number("status_at"),
            waiting_for: text("waiting_for"),
            cache_expires_at: integer("cache_expires_at"),
            notified: number("notified"),
            reaped: number("reaped"),
            headless: map
                .get("headless")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            turn_base: map.get("turn_base").and_then(Value::as_u64),
            result_seq: map
                .get("result_seq")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        })
    }

    /// Whether the session outlives its process: a mate or a headless worker.
    pub fn long_lived(&self) -> bool {
        self.kind.as_deref() == Some(MATE_KIND) || self.headless
    }

    /// Takes the fields other has set.
    pub fn merge(&mut self, other: Record) {
        self.kind = other.kind.or(self.kind.take());
        self.workspace = other.workspace.or(self.workspace.take());
        self.title = other.title.or(self.title.take());
        self.description = other.description.or(self.description.take());
        self.reports_to = other.reports_to.or(self.reports_to.take());
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
        self.turn_base = None;
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

/// The content of the file at path, None if it is missing or not a plain file of at most max bytes.
fn read_capped(path: &Path, max: u64) -> Option<Vec<u8>> {
    // O_NONBLOCK: opening a FIFO must not wait for a writer
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut data = Vec::new();
    file.take(max + 1).read_to_end(&mut data).ok()?;
    (data.len() as u64 <= max).then_some(data)
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
        let data = read_capped(&self.dir.join(format!("{name}.json")), RECORD_MAX)?;
        Record::from_value(serde_json::from_slice(&data).ok()?)
    }

    /// The directory of the messages queued for headless worker name.
    pub fn queue_dir(&self, name: &str) -> PathBuf {
        self.dir.join("queue").join(name)
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

    /// Deletes the records untouched for RECORD_DAYS, except those of a mate or headless worker that is not reaped,
    /// with the results and queue of a deleted record.
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
            let name = entry
                .file_name()
                .to_str()
                .and_then(|f| f.strip_suffix(".json"))
                .filter(|name| valid_name(name))
                .map(str::to_string);
            let kept = name
                .as_deref()
                .and_then(|name| self.read(name))
                .is_some_and(|r| r.long_lived() && r.reaped.is_none());
            if old && !kept {
                let _ = fs::remove_file(&path);
                if let Some(name) = name {
                    let _ = fs::remove_dir_all(self.dir.join("results").join(&name));
                    let _ = fs::remove_dir_all(self.dir.join("queue").join(&name));
                }
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

    /// Under the records lock: if guard (which may change the record) returns true, writes the next numbered result of
    /// session name, saves the record with the turn closed and returns the number; false: nothing is written.
    pub fn write_result(
        &self,
        name: &str,
        kind: ResultKind,
        text: &str,
        guard: impl FnOnce(&mut Record) -> bool,
    ) -> Result<Option<u64>, String> {
        let mut written = Ok(None);
        self.update(name, |r| {
            if !guard(r) {
                return false;
            }
            let seq = self.last_seq(name, r) + 1;
            written = self.store_result(name, seq, kind, text).map(|()| Some(seq));
            if written.is_ok() {
                r.result_seq = seq;
                r.turn_base = None;
            }
            written.is_ok()
        })?;
        written
    }

    fn store_result(
        &self,
        name: &str,
        seq: u64,
        kind: ResultKind,
        text: &str,
    ) -> Result<(), String> {
        let text = if text.chars().count() > RESULT_TEXT_MAX {
            let head: String = text.chars().take(RESULT_TEXT_MAX).collect();
            format!("{head}\n[truncated]")
        } else {
            text.to_string()
        };
        let result = TurnResult {
            seq,
            kind,
            text,
            at: now(),
        };
        let dir = self.dir.join("results").join(name);
        let tmp = dir.join(format!(".{seq}.tmp"));
        let stored = DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .and_then(|()| {
                let json = serde_json::to_string(&result).map_err(io::Error::from)?;
                let mut file = OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&tmp)?;
                file.write_all(json.as_bytes())?;
                file.sync_all()?;
                fs::rename(&tmp, dir.join(format!("{seq}.json")))
            });
        stored.map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("cannot write the result of {name}: {e}")
        })
    }

    /// The numbers of the unacked results of session name, ascending: the numeric `<n>.json` file names are the
    /// source of truth.
    pub fn result_seqs(&self, name: &str) -> Vec<u64> {
        if !valid_name(name) {
            return vec![];
        }
        let Ok(entries) = fs::read_dir(self.dir.join("results").join(name)) else {
            return vec![];
        };
        let mut seqs: Vec<u64> = entries
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_suffix(".json")?
                    .parse::<u64>()
                    .ok()
            })
            .collect();
        seqs.sort_unstable();
        seqs
    }

    /// The highest result number of session name: a result file renamed into place before its record was saved
    /// counts.
    fn last_seq(&self, name: &str, record: &Record) -> u64 {
        let on_disk = self.result_seqs(name).last().copied().unwrap_or_default();
        record.result_seq.max(on_disk)
    }

    /// The unacked results of session name, by number; an unreadable file is reported on stderr and skipped.
    pub fn results(&self, name: &str) -> Vec<TurnResult> {
        let dir = self.dir.join("results").join(name);
        self.result_seqs(name)
            .into_iter()
            .filter_map(|seq| {
                let result = read_capped(&dir.join(format!("{seq}.json")), RESULT_FILE_MAX)
                    .and_then(|data| serde_json::from_slice(&data).ok());
                if result.is_none() {
                    eprintln!("jw: {name}: unreadable result {seq}");
                }
                result
            })
            .collect()
    }

    /// Deletes the results of session name up to seq; a seq beyond the last result fails.
    pub fn ack(&self, name: &str, seq: u64) -> Result<(), String> {
        let record = self
            .read(name)
            .ok_or_else(|| format!("no session record {name}"))?;
        if seq > self.last_seq(name, &record) {
            return Err(format!("no result {seq} of {name}"));
        }
        let dir = self.dir.join("results").join(name);
        for n in self.result_seqs(name).into_iter().filter(|&n| n <= seq) {
            fs::remove_file(dir.join(format!("{n}.json")))
                .map_err(|e| format!("cannot delete the result of {name}: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/record.rs"]
mod unit_tests;
