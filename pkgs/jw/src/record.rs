use std::collections::BTreeMap;
use std::process;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::kv::{nats_url, Bucket, Kv, KvError};

const RECORD_MAX: usize = 65536;
const QUEUE_MESSAGE_MAX: usize = 786432;
const RESULT_MAX: usize = 786432;
const RECORD_DAYS: u64 = 7;
const BUSY_WAIT: Duration = Duration::from_secs(5);
const BUSY: &str = "session records are busy";
const RESULT_TEXT_MAX: usize = 262144;
const TRUNCATED: &str = "\n[truncated]";
/// TRUNCATED as the JSON of a string escapes it.
const TRUNCATED_JSON_LEN: usize = TRUNCATED.len() + 1;
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

/// The result of one turn of a headless worker, a numbered entry kept until the supervisor acks it.
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
#[derive(Default, Serialize, Clone)]
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
    /// Workspaces the session created (`jw hook create`); `jw reap` drops them once it is done.
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
    /// Claude runs without bypass mode: a headless turn denies what needs a prompt, an interactive session asks.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ask_permissions: bool,
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
            ask_permissions: map
                .get("ask_permissions")
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

/// A name that is a KV key whose `<name>.*` subjects are exactly the entries of this session.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        && !name.contains("..")
        && !name.ends_with('.')
}

fn check_name(name: &str) -> Result<(), String> {
    if valid_name(name) {
        Ok(())
    } else {
        Err(format!("invalid session name: '{name}'"))
    }
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The record in a stored value, None if it is too large or not a JSON object.
fn parse_record(data: &[u8]) -> Option<Record> {
    if data.len() > RECORD_MAX {
        return None;
    }
    serde_json::from_slice(data)
        .ok()
        .and_then(Record::from_value)
}

fn result_key(name: &str, seq: u64) -> String {
    format!("{name}.{seq}")
}

fn turn_result(seq: u64, kind: ResultKind, text: &str) -> Result<TurnResult, serde_json::Error> {
    let mut result = TurnResult {
        seq,
        kind,
        text: String::new(),
        at: now(),
    };
    let room = RESULT_MAX - serde_json::to_vec(&result)?.len();
    result.text = clip_result_text(text, room);
    Ok(result)
}

/// The bytes a character takes in a JSON string.
fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if c < ' ' => 6,
        c => c.len_utf8(),
    }
}

/// text cut to RESULT_TEXT_MAX characters and to room bytes of JSON, marked when it was cut.
fn clip_result_text(text: &str, room: usize) -> String {
    let fits = text.chars().count() <= RESULT_TEXT_MAX
        && text.chars().map(json_len).sum::<usize>() <= room;
    if fits {
        return text.to_string();
    }
    let room = room - TRUNCATED_JSON_LEN;
    let limit = text
        .char_indices()
        .nth(RESULT_TEXT_MAX)
        .map_or(text.len(), |(i, _)| i);
    let mut used = 0;
    let end = text[..limit]
        .char_indices()
        .find_map(|(i, c)| {
            used += json_len(c);
            (used > room).then_some(i)
        })
        .unwrap_or(limit);
    format!("{}{TRUNCATED}", &text[..end])
}

/// The outcome of one attempt to write a result.
enum Attempt {
    Done(Option<u64>),
    /// The record or the result number changed meanwhile.
    Retry,
}

/// The connection of this process, shared by every `Records::open`.
static SHARED_KV: Mutex<Option<Arc<Kv>>> = Mutex::new(None);

/// The session records and results, and the queues of the headless workers, in NATS KV.
pub struct Records {
    kv: Arc<Kv>,
}

impl Records {
    pub fn open() -> Result<Records, String> {
        let mut shared = SHARED_KV.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(kv) = shared.as_ref() {
            return Ok(Records { kv: Arc::clone(kv) });
        }
        let records = Records::connect(&nats_url())?;
        *shared = Some(Arc::clone(&records.kv));
        Ok(records)
    }

    /// Records on a fresh connection to the NATS server at url.
    pub fn connect(url: &str) -> Result<Records, String> {
        Ok(Records {
            kv: Arc::new(Kv::connect(url)?),
        })
    }

    /// The record of session name, None if it is missing or not a small JSON object.
    pub fn read(&self, name: &str) -> Result<Option<Record>, String> {
        if !valid_name(name) {
            return Ok(None);
        }
        Ok(self
            .kv
            .get(Bucket::Sessions, name)?
            .and_then(|e| parse_record(&e.value)))
    }

    /// The records, sorted by name.
    pub fn load_all(&self) -> Result<Vec<(String, Record)>, String> {
        let mut records = vec![];
        for key in self.kv.keys(Bucket::Sessions)? {
            if !valid_name(&key) {
                continue;
            }
            if let Some(record) = self.read(&key)? {
                records.push((key, record));
            }
        }
        Ok(records)
    }

    /// Writes record as the value of session name: created if revision is None, else replacing that revision.
    fn save(&self, name: &str, record: &Record, revision: Option<u64>) -> Result<u64, KvError> {
        let json = serde_json::to_vec(record).map_err(|e| KvError::Other(e.to_string()))?;
        if json.len() > RECORD_MAX {
            return Err(KvError::Other(format!(
                "{WRITE_FAILED}it is larger than {RECORD_MAX} bytes"
            )));
        }
        match revision {
            Some(revision) => self.kv.update(Bucket::Sessions, name, &json, revision),
            None => self.kv.create(Bucket::Sessions, name, &json),
        }
    }

    /// Calls change on the record of session name (empty if there is none) and writes it unless that returns false;
    /// returns the record. A concurrent write reruns change on the new record, for up to BUSY_WAIT, so change must have
    /// no side effects beyond what it captures.
    pub fn update(
        &self,
        name: &str,
        mut change: impl FnMut(&mut Record) -> bool,
    ) -> Result<Record, String> {
        check_name(name)?;
        let deadline = Instant::now() + BUSY_WAIT;
        loop {
            let entry = self.kv.get(Bucket::Sessions, name)?;
            let mut record = entry
                .as_ref()
                .and_then(|e| parse_record(&e.value))
                .unwrap_or_default();
            if !change(&mut record) {
                return Ok(record);
            }
            match self.save(name, &record, entry.map(|e| e.revision)) {
                Ok(_) => return Ok(record),
                Err(KvError::Conflict) if Instant::now() >= deadline => {
                    return Err(BUSY.to_string())
                }
                Err(KvError::Conflict) => {}
                Err(KvError::Other(e)) => return Err(write_error(&e)),
            }
        }
    }

    /// Deletes the records untouched for RECORD_DAYS before now, except those of a mate or headless worker that is not
    /// reaped, with the results and queue of a deleted record; calls on_pruned with each name once it is deleted.
    pub fn prune(&self, now: f64, mut on_pruned: impl FnMut(&str)) -> Result<(), String> {
        let cutoff = now - (RECORD_DAYS * 86400) as f64;
        for key in self.kv.keys(Bucket::Sessions)? {
            if !valid_name(&key) {
                continue;
            }
            let Some(entry) = self.kv.get(Bucket::Sessions, &key)? else {
                continue;
            };
            let kept =
                parse_record(&entry.value).is_some_and(|r| r.long_lived() && r.reaped.is_none());
            if entry.at >= cutoff || kept {
                continue;
            }
            match self.kv.delete(Bucket::Sessions, &key, entry.revision) {
                Ok(()) => {
                    self.purge_entries(&key);
                    on_pruned(&key);
                }
                Err(KvError::Conflict) => {}
                Err(KvError::Other(e)) => return Err(e),
            }
        }
        Ok(())
    }

    fn purge_entries(&self, name: &str) {
        for bucket in [Bucket::Results, Bucket::Queue] {
            if let Err(e) = self.kv.purge(bucket, &format!("{name}.*")) {
                eprintln!("jw: {name}: {e}");
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
                r.waiting_for = waiting_for.clone();
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

    /// If guard (which may change the record) returns true, writes the next numbered result of session name, saves the
    /// record with the turn closed and returns the number; false: nothing is written. The result is created first and
    /// deleted again if the record changed meanwhile (a result left by an earlier attempt is reused, and deleted only
    /// if guard returns false); a concurrent write reruns guard for up to BUSY_WAIT, so guard
    /// must have no side effects beyond what it captures.
    pub fn write_result(
        &self,
        name: &str,
        kind: ResultKind,
        text: &str,
        mut guard: impl FnMut(&mut Record) -> bool,
    ) -> Result<Option<u64>, String> {
        check_name(name)?;
        let deadline = Instant::now() + BUSY_WAIT;
        loop {
            if let Attempt::Done(seq) = self.attempt_result(name, kind, text, &mut guard)? {
                return Ok(seq);
            }
            if Instant::now() >= deadline {
                return Err(BUSY.to_string());
            }
        }
    }

    fn attempt_result(
        &self,
        name: &str,
        kind: ResultKind,
        text: &str,
        guard: &mut impl FnMut(&mut Record) -> bool,
    ) -> Result<Attempt, String> {
        let cannot = |e: &dyn std::fmt::Display| format!("cannot write the result of {name}: {e}");
        let entry = self.kv.get(Bucket::Sessions, name)?;
        let mut record = entry
            .as_ref()
            .and_then(|e| parse_record(&e.value))
            .unwrap_or_default();
        let reused = self.written_result(name, &record, kind, text)?;
        if !guard(&mut record) {
            if let Some(seq) = reused {
                let _ = self.kv.purge(Bucket::Results, &result_key(name, seq));
            }
            return Ok(Attempt::Done(None));
        }
        let seq = match reused {
            Some(seq) => seq,
            None => self.last_seq(name, &record)? + 1,
        };
        let key = result_key(name, seq);
        if reused.is_none() {
            let json = serde_json::to_vec(&turn_result(seq, kind, text).map_err(|e| cannot(&e))?)
                .map_err(|e| cannot(&e))?;
            match self.kv.create(Bucket::Results, &key, &json) {
                Ok(_) => {}
                Err(KvError::Conflict) => return Ok(Attempt::Retry),
                Err(e) => return Err(cannot(&e)),
            }
        }
        record.result_seq = seq;
        record.turn_base = None;
        let drop_created = || {
            if reused.is_none() {
                let _ = self.kv.purge(Bucket::Results, &key);
            }
        };
        match self.save(name, &record, entry.map(|e| e.revision)) {
            Ok(_) => Ok(Attempt::Done(Some(seq))),
            Err(KvError::Conflict) => {
                drop_created();
                Ok(Attempt::Retry)
            }
            // a lost reply: if the record points at our result, the write landed
            Err(KvError::Other(e)) => match self.read(name) {
                Ok(Some(saved)) if saved.result_seq == seq => Ok(Attempt::Done(Some(seq))),
                Ok(_) => {
                    drop_created();
                    Err(write_error(&e))
                }
                Err(_) => Err(write_error(&e)),
            },
        }
    }

    /// The highest result of session name above the saved result_seq if it already holds this kind and text: an
    /// earlier create whose reply was lost, which the retry reuses instead of writing a duplicate.
    fn written_result(
        &self,
        name: &str,
        record: &Record,
        kind: ResultKind,
        text: &str,
    ) -> Result<Option<u64>, String> {
        let Some(seq) = self
            .result_seqs(name)?
            .last()
            .copied()
            .filter(|&seq| seq > record.result_seq)
        else {
            return Ok(None);
        };
        let Some(entry) = self.kv.get(Bucket::Results, &result_key(name, seq))? else {
            return Ok(None);
        };
        let wanted = turn_result(seq, kind, text).map_err(|e| e.to_string())?;
        Ok(serde_json::from_slice::<TurnResult>(&entry.value)
            .ok()
            .filter(|old| old.kind == wanted.kind && old.text == wanted.text)
            .map(|_| seq))
    }

    /// The numbers of the unacked results of every session, ascending: the numeric result keys are the source of truth.
    pub fn unacked(&self) -> Result<BTreeMap<String, Vec<u64>>, String> {
        let mut unacked: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for key in self.kv.keys(Bucket::Results)? {
            let Some((name, seq)) = key.rsplit_once('.') else {
                continue;
            };
            let Some(seq) = seq.parse().ok().filter(|&seq| key == result_key(name, seq)) else {
                continue;
            };
            unacked.entry(name.to_string()).or_default().push(seq);
        }
        unacked.values_mut().for_each(|seqs| seqs.sort_unstable());
        Ok(unacked)
    }

    /// The numbers of the unacked results of session name, ascending.
    pub fn result_seqs(&self, name: &str) -> Result<Vec<u64>, String> {
        Ok(self.unacked()?.remove(name).unwrap_or_default())
    }

    /// The highest result number of session name: a result created before its record was saved counts.
    fn last_seq(&self, name: &str, record: &Record) -> Result<u64, String> {
        let stored = self.result_seqs(name)?.last().copied().unwrap_or_default();
        Ok(record.result_seq.max(stored))
    }

    /// The unacked results of session name, by number; an unreadable one is reported on stderr and skipped.
    pub fn results(&self, name: &str) -> Result<Vec<TurnResult>, String> {
        let mut results = vec![];
        for seq in self.result_seqs(name)? {
            let Some(entry) = self.kv.get(Bucket::Results, &result_key(name, seq))? else {
                continue;
            };
            match serde_json::from_slice(&entry.value) {
                Ok(result) => results.push(result),
                Err(_) => eprintln!("jw: {name}: unreadable result {seq}"),
            }
        }
        Ok(results)
    }

    /// Deletes the results of session name up to seq; a seq beyond the last result fails.
    pub fn ack(&self, name: &str, seq: u64) -> Result<(), String> {
        let record = self
            .read(name)?
            .ok_or_else(|| format!("no session record {name}"))?;
        if seq > self.last_seq(name, &record)? {
            return Err(format!("no result {seq} of {name}"));
        }
        for n in self.result_seqs(name)?.into_iter().filter(|&n| n <= seq) {
            self.kv
                .purge(Bucket::Results, &result_key(name, n))
                .map_err(|e| format!("cannot delete the result of {name}: {e}"))?;
        }
        Ok(())
    }
}

/// A message queued for a headless worker; revision is that of its queue entry.
pub struct Queued {
    pub id: String,
    pub text: String,
    pub revision: u64,
}

fn queue_key(name: &str, id: &str) -> String {
    format!("{name}.{id}")
}

fn queue_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos:020}-{}", process::id())
}

/// The queue of a worker: an entry is created holding the message, a turn claims it by rewriting it at the revision it
/// listed and deletes it after the turn; a sender that gives up cancels it by blanking it at its revision. Either
/// rewrite invalidates the other's revision, so exactly one of them wins.
impl Records {
    /// Queues text for worker name; returns the id and revision of the entry.
    pub fn enqueue(&self, name: &str, text: &str) -> Result<(String, u64), String> {
        let fail = |e: &str| format!("cannot queue the message for {name}: {e}");
        if text.is_empty() {
            return Err(fail("empty message"));
        }
        if text.len() > QUEUE_MESSAGE_MAX {
            return Err(fail(&format!(
                "it is larger than {QUEUE_MESSAGE_MAX} bytes"
            )));
        }
        loop {
            let id = queue_id();
            match self
                .kv
                .create(Bucket::Queue, &queue_key(name, &id), text.as_bytes())
            {
                Ok(revision) => return Ok((id, revision)),
                Err(KvError::Conflict) => {}
                Err(e) => return Err(fail(&e.to_string())),
            }
        }
    }

    /// The messages queued for worker name by id; cancelled entries are left out.
    pub fn queued(&self, name: &str) -> Result<Vec<Queued>, String> {
        let prefix = format!("{name}.");
        let mut queued = vec![];
        for key in self.kv.keys(Bucket::Queue)? {
            let Some(id) = key.strip_prefix(&prefix).filter(|id| !id.contains('.')) else {
                continue;
            };
            let Some(entry) = self.kv.get(Bucket::Queue, &key)? else {
                continue;
            };
            if !entry.value.is_empty() {
                queued.push(Queued {
                    id: id.to_string(),
                    text: String::from_utf8_lossy(&entry.value).into_owned(),
                    revision: entry.revision,
                });
            }
        }
        Ok(queued)
    }

    /// Takes the message q for a turn; false if it was cancelled or claimed meanwhile.
    pub fn claim(&self, name: &str, q: &Queued) -> Result<bool, String> {
        let key = queue_key(name, &q.id);
        match self
            .kv
            .update(Bucket::Queue, &key, q.text.as_bytes(), q.revision)
        {
            Ok(_) => Ok(true),
            Err(KvError::Conflict) => Ok(false),
            Err(KvError::Other(e)) => Err(e),
        }
    }

    /// Withdraws the message id enqueued at revision; false if a turn claimed it.
    pub fn cancel(&self, name: &str, id: &str, revision: u64) -> Result<bool, String> {
        let key = queue_key(name, id);
        match self.kv.update(Bucket::Queue, &key, b"", revision) {
            Ok(_) => {
                let _ = self.kv.purge(Bucket::Queue, &key);
                Ok(true)
            }
            Err(KvError::Conflict) => Ok(false),
            Err(KvError::Other(e)) => Err(e),
        }
    }

    /// Deletes the message id of worker name.
    pub fn dequeue(&self, name: &str, id: &str) -> Result<(), String> {
        self.kv.purge(Bucket::Queue, &queue_key(name, id))
    }
}

#[cfg(test)]
#[path = "../tests/unit/record.rs"]
mod unit_tests;
