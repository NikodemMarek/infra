use std::env;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const MAX_HTML: u64 = 2_097_152;
const MAX_TITLE_CHARS: usize = 200;
const KEEP_SECS: u64 = 604_800;
const DEFAULT_ADDR: &str = "127.0.0.1:4388";

/// `$PLAN_REVIEW_ADDR`, `127.0.0.1:4388` when unset or empty: where the server binds and the clients connect.
pub fn addr() -> String {
    env::var("PLAN_REVIEW_ADDR")
        .ok()
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| DEFAULT_ADDR.to_string())
}

pub fn origin() -> String {
    format!("http://{}", addr())
}

pub struct Store {
    pub root: PathBuf,
}

/// `<root>/<id>/meta.json`
#[derive(Serialize, Deserialize)]
pub struct Meta {
    pub session: String,
    pub path: String,
    pub title: String,
    pub token: String,
    pub revision: u64,
    pub updated: u64,
}

/// A revision of a review, as carried by the review events.
#[derive(Serialize, Deserialize)]
pub struct ReviewRef {
    pub review: String,
    pub revision: u64,
}

/// The payload of `review.requested`.
#[derive(Serialize, Deserialize)]
pub struct Requested {
    pub review: String,
    pub revision: u64,
    pub session: String,
    pub title: String,
    pub token: String,
}

impl Store {
    pub fn new(root: PathBuf) -> Store {
        Store { root }
    }

    /// `$XDG_STATE_HOME/plan-review`, `$HOME/.local/state/plan-review` when unset or not absolute.
    pub fn from_env() -> Result<Store, String> {
        let state = match env::var("XDG_STATE_HOME") {
            Ok(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
            _ => {
                let home = env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
                PathBuf::from(home).join(".local/state")
            }
        };
        Ok(Store::new(state.join("plan-review")))
    }

    /// Writes the next revision of the review of (session, path) under the lock and prunes the old reviews.
    pub fn snapshot(
        &self,
        session: &str,
        path: &str,
        title: &str,
        html: &[u8],
    ) -> Result<Requested, String> {
        let _lock = self.lock()?;
        let reviews = self.reviews()?;
        let (id, token, revision) = match reviews
            .iter()
            .find(|(_, m)| m.session == session && m.path == path)
        {
            Some((id, m)) => (id.clone(), m.token.clone(), m.revision + 1),
            None => (random_hex(8)?, random_hex(16)?, 1),
        };
        let dir = self.root.join(&id);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        write_private(&dir.join(format!("r{revision}.html")), html)?;
        let updated = now();
        let meta = Meta {
            session: session.to_string(),
            path: path.to_string(),
            title: title.to_string(),
            token: token.clone(),
            revision,
            updated,
        };
        let tmp = dir.join("meta.json.tmp");
        let json = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
        write_private(&tmp, &json)?;
        let target = dir.join("meta.json");
        fs::rename(&tmp, &target).map_err(|e| format!("cannot write {}: {e}", target.display()))?;
        for (other, m) in &reviews {
            if *other != id && m.updated < updated.saturating_sub(KEEP_SECS) {
                let _ = fs::remove_dir_all(self.root.join(other));
            }
        }
        Ok(Requested {
            review: id,
            revision,
            session: session.to_string(),
            title: title.to_string(),
            token,
        })
    }

    /// The lines of `plan-review list`, newest first, one `\n`-terminated line per review.
    pub fn list(&self, origin: &str) -> Result<String, String> {
        let mut reviews = self.reviews()?;
        reviews.sort_by(|a, b| b.1.updated.cmp(&a.1.updated).then_with(|| a.0.cmp(&b.0)));
        Ok(reviews
            .into_iter()
            .map(|(id, m)| {
                let sent = self.feedback_path(&id, m.revision).exists();
                format!(
                    "{origin}/r/{id}?t={}\t{}\t{}\tr{}\t{}\n",
                    m.token,
                    if sent { "sent" } else { "open" },
                    m.session,
                    m.revision,
                    m.title
                )
            })
            .collect())
    }

    pub fn lock(&self) -> Result<File, String> {
        let fail = |e: io::Error| format!("cannot lock {}: {e}", self.root.display());
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)
            .map_err(fail)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.root.join(".lock"))
            .map_err(fail)?;
        lock.lock().map_err(fail)?;
        Ok(lock)
    }

    pub fn read_meta(&self, id: &str) -> Option<Meta> {
        let data = fs::read(self.root.join(id).join("meta.json")).ok()?;
        serde_json::from_slice(&data).ok()
    }

    pub fn feedback_path(&self, id: &str, n: u64) -> PathBuf {
        self.root.join(id).join(format!("feedback-r{n}.json"))
    }

    /// Every review dir with a readable meta.
    fn reviews(&self) -> Result<Vec<(String, Meta)>, String> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(format!("cannot read {}: {e}", self.root.display())),
        };
        Ok(entries
            .flatten()
            .filter_map(|e| {
                let id = e.file_name().into_string().ok().filter(|n| valid_id(n))?;
                let meta = self.read_meta(&id)?;
                Some((id, meta))
            })
            .collect())
    }
}

/// The review an event refers to: `Err` when the event is not a JSON object, `None` when its review or revision is invalid.
pub fn review_ref(event: &str) -> Result<Option<ReviewRef>, String> {
    let event: Map<String, Value> =
        serde_json::from_str(event).map_err(|_| "the event is not a JSON object")?;
    Ok(serde_json::from_value::<ReviewRef>(Value::Object(event))
        .ok()
        .filter(|r| valid_id(&r.review) && r.revision >= 1))
}

/// Collapse whitespace runs to one space, drop other control characters, trim.
pub fn oneline(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_whitespace() || !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The title as stored: one line, at most 200 characters, no trailing space.
pub fn clean_title(title: &str) -> String {
    let title: String = oneline(title).chars().take(MAX_TITLE_CHARS).collect();
    title.trim_end().to_string()
}

pub fn valid_session(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

pub fn valid_id(id: &str) -> bool {
    lower_hex(id, 16)
}

pub fn valid_token(token: &str) -> bool {
    lower_hex(token, 32)
}

fn lower_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| format!("cannot read /dev/urandom: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(data))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Publishes an event of kind with payload as JSON through `event-pub`.
pub fn event_pub<T: Serialize>(kind: &str, payload: &T) -> Result<(), String> {
    let payload = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    let mut child = Command::new("event-pub")
        .arg(kind)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => "event-pub not found".to_string(),
            _ => format!("event-pub: {e}"),
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("event-pub: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(
        match stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()) {
            Some(line) => line.to_string(),
            None => format!("event-pub failed: {}", out.status),
        },
    )
}
