//! Answers a request by writing one line to its reply FIFO. The path comes from an event, which
//! any local process can post, so it is only trusted after the checks in `write_reply`.
//! The line may be a password: it is written, never formatted, logged or stored.

use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

/// Name prefix of the directory that may hold the reply FIFO of an event type: a password can
/// never go to a hook FIFO nor a hook decision to askpass. None = the type is not answerable.
pub fn reply_prefix(kind: &str) -> Option<&'static str> {
    match kind {
        "sudo.password_request" => Some("claude-askpass-"),
        "claude.permission_request" | "claude.question" => Some("claude-hook-"),
        _ => None,
    }
}

#[derive(Debug)]
pub enum ReplyError {
    /// The target is not a reply FIFO of ours; nothing was written.
    Invalid(String),
    /// Nobody is waiting any more (directory gone or no reader on the FIFO).
    Expired,
    Io(io::Error),
}

impl fmt::Display for ReplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplyError::Invalid(why) => write!(f, "refusing to answer: {why}"),
            ReplyError::Expired => write!(f, "expired: nothing is waiting for the answer"),
            ReplyError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ReplyError {}

impl From<io::Error> for ReplyError {
    fn from(e: io::Error) -> Self {
        ReplyError::Io(e)
    }
}

fn invalid(why: &str) -> ReplyError {
    ReplyError::Invalid(why.to_string())
}

/// Writes `line` + newline to `reply`, which must be `<runtime>/<prefix>*/reply`:
/// a FIFO owned by us, closed to group and others, in a real directory owned by us.
pub fn write_reply(
    runtime: &Path,
    prefix: &str,
    reply: &Path,
    line: &str,
) -> Result<(), ReplyError> {
    if reply.file_name() != Some(OsStr::new("reply")) {
        return Err(invalid("target is not named reply"));
    }
    let dir = reply
        .parent()
        .ok_or_else(|| invalid("target has no directory"))?;
    if dir.parent() != Some(runtime.canonicalize()?.as_path()) {
        return Err(invalid("target is outside the runtime directory"));
    }
    let name = dir.file_name().and_then(OsStr::to_str).unwrap_or("");
    if !name.starts_with(prefix) {
        return Err(invalid("target directory is not a known reply directory"));
    }

    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_dir() && m.uid() == euid => {}
        Ok(_) => return Err(invalid("target directory is not a directory of ours")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(ReplyError::Expired),
        Err(e) => return Err(e.into()),
    }

    // O_NONBLOCK: without a reader the open fails (ENXIO) instead of hanging.
    // O_NOFOLLOW: the final component must not be a symlink.
    let mut file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(reply)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ENXIO) | Some(libc::ENOENT) => ReplyError::Expired,
            Some(libc::ELOOP) => invalid("target is a symlink"),
            _ => ReplyError::Io(e),
        })?;

    // Check what was actually opened, not what the path pointed to a moment ago.
    let meta = file.metadata()?;
    if !meta.file_type().is_fifo() {
        return Err(invalid("target is not a FIFO"));
    }
    if meta.uid() != euid {
        return Err(invalid("target is owned by another user"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(invalid("target is accessible to group or others"));
    }

    match file
        .write_all(line.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
    {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Err(ReplyError::Expired),
        other => other.map_err(ReplyError::Io),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::PathBuf;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// Fresh canonical runtime dir under the temp dir.
    fn runtime(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("agent-inbox-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn mkfifo(path: &Path, mode: u32) {
        let c = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), mode as libc::mode_t) }, 0);
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// `<runtime>/<dir>/reply` as a FIFO with `mode`; returns the reply path.
    fn fifo_in(runtime: &Path, dir: &str, mode: u32) -> PathBuf {
        let dir = runtime.join(dir);
        fs::create_dir(&dir).unwrap();
        let reply = dir.join("reply");
        mkfifo(&reply, mode);
        reply
    }

    /// Reader end that does not block on open (what askpass's `head` is, once it has opened).
    fn reader(reply: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(reply)
            .unwrap()
    }

    /// Everything written to `r` until EOF. With many tests running in parallel the read end can
    /// still see a write end (EAGAIN) briefly after `write_reply` closed it, so retry for a while.
    fn read_all(r: &mut File) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut got = Vec::new();
        loop {
            match r.read_to_end(&mut got) {
                Ok(_) => return String::from_utf8(got).unwrap(),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("read failed: {e}"),
            }
        }
    }

    fn is_invalid(r: Result<(), ReplyError>) -> bool {
        matches!(r, Err(ReplyError::Invalid(_)))
    }

    const ASKPASS: &str = "claude-askpass-";
    const HOOK: &str = "claude-hook-";

    #[test]
    fn prefix_is_bound_to_the_event_type() {
        assert_eq!(reply_prefix("sudo.password_request"), Some(ASKPASS));
        assert_eq!(reply_prefix("claude.permission_request"), Some(HOOK));
        assert_eq!(reply_prefix("claude.question"), Some(HOOK));
        assert_eq!(reply_prefix("claude.notification"), None);
        assert_eq!(reply_prefix(""), None);
    }

    #[test]
    fn writes_line_to_reader() {
        let rt = runtime("happy");
        let reply = fifo_in(&rt, "claude-askpass-abc123", 0o600);
        let mut r = reader(&reply);
        write_reply(&rt, ASKPASS, &reply, "pw").unwrap();
        assert_eq!(read_all(&mut r), "pw\n");
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn writes_decision_to_hook_dir() {
        let rt = runtime("hook");
        let reply = fifo_in(&rt, "claude-hook-abc123", 0o600);
        let mut r = reader(&reply);
        write_reply(&rt, HOOK, &reply, r#"{"behavior":"allow"}"#).unwrap();
        assert_eq!(read_all(&mut r), "{\"behavior\":\"allow\"}\n");
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn prefix_of_another_kind_is_invalid() {
        let rt = runtime("kinds");
        let ask = fifo_in(&rt, "claude-askpass-abc123", 0o600);
        let hook = fifo_in(&rt, "claude-hook-abc123", 0o600);
        let mut ask_r = reader(&ask);
        let mut hook_r = reader(&hook);
        assert!(is_invalid(write_reply(&rt, HOOK, &ask, "pw")));
        assert!(is_invalid(write_reply(&rt, ASKPASS, &hook, "{}")));
        assert_eq!(read_all(&mut ask_r), "");
        assert_eq!(read_all(&mut hook_r), "");
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn no_reader_is_expired() {
        let rt = runtime("noreader");
        let reply = fifo_in(&rt, "claude-askpass-abc123", 0o600);
        assert!(matches!(
            write_reply(&rt, ASKPASS, &reply, "pw"),
            Err(ReplyError::Expired)
        ));
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn missing_dir_is_expired() {
        let rt = runtime("missing");
        let reply = rt.join("claude-askpass-gone").join("reply");
        assert!(matches!(
            write_reply(&rt, ASKPASS, &reply, "pw"),
            Err(ReplyError::Expired)
        ));
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn regular_file_is_invalid() {
        let rt = runtime("regular");
        let dir = rt.join("claude-askpass-abc123");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("reply"), "").unwrap();
        assert!(is_invalid(write_reply(
            &rt,
            ASKPASS,
            &dir.join("reply"),
            "pw"
        )));
        assert_eq!(fs::read(dir.join("reply")).unwrap(), b"");
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn symlink_to_fifo_is_invalid() {
        let rt = runtime("symlink");
        let real = fifo_in(&rt, "claude-askpass-abc123", 0o600);
        let dir = rt.join("claude-askpass-link");
        fs::create_dir(&dir).unwrap();
        symlink(&real, dir.join("reply")).unwrap();
        let _r = reader(&real);
        assert!(is_invalid(write_reply(
            &rt,
            ASKPASS,
            &dir.join("reply"),
            "pw"
        )));
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn open_to_others_is_invalid() {
        let rt = runtime("mode");
        let reply = fifo_in(&rt, "claude-askpass-abc123", 0o644);
        let mut r = reader(&reply);
        assert!(is_invalid(write_reply(&rt, ASKPASS, &reply, "pw")));
        assert_eq!(read_all(&mut r), "");
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn unknown_prefix_is_invalid() {
        let rt = runtime("prefix");
        let reply = fifo_in(&rt, "evil-abc123", 0o600);
        let _r = reader(&reply);
        assert!(is_invalid(write_reply(&rt, ASKPASS, &reply, "pw")));
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn path_outside_runtime_is_invalid() {
        let rt = runtime("outside");
        let run = rt.join("run");
        fs::create_dir(&run).unwrap();
        let reply = fifo_in(&rt, "claude-askpass-abc123", 0o600);
        let _r = reader(&reply);
        assert!(is_invalid(write_reply(&run, ASKPASS, &reply, "pw")));
        let dotdot = run.join("..").join("claude-askpass-abc123").join("reply");
        assert!(is_invalid(write_reply(&run, ASKPASS, &dotdot, "pw")));
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn symlinked_directory_is_invalid() {
        let rt = runtime("dirlink");
        let real = fifo_in(&rt, "real", 0o600);
        let _r = reader(&real);
        symlink(rt.join("real"), rt.join("claude-askpass-link")).unwrap();
        let reply = rt.join("claude-askpass-link").join("reply");
        assert!(is_invalid(write_reply(&rt, ASKPASS, &reply, "pw")));
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn wrong_file_name_is_invalid() {
        let rt = runtime("name");
        let reply = fifo_in(&rt, "claude-askpass-abc123", 0o600);
        let other = reply.with_file_name("other");
        fs::rename(&reply, &other).unwrap();
        assert!(is_invalid(write_reply(&rt, ASKPASS, &other, "pw")));
        fs::remove_dir_all(&rt).unwrap();
    }
}
