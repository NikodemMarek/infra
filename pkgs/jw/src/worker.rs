use crate::cmd::event_pub;
use crate::record::{Record, Records, ResultKind};

/// The transient user unit that runs the turns of headless worker name.
pub fn unit(name: &str) -> String {
    format!("jw-worker-{name}")
}

/// Writes the result of a turn of worker name (see `Records::write_result`) and, if it was written, publishes
/// `session.result` for it.
pub fn finish(
    name: &str,
    kind: ResultKind,
    text: &str,
    guard: impl FnOnce(&mut Record) -> bool,
) -> Result<Option<u64>, String> {
    let seq = Records::new().write_result(name, kind, text, guard)?;
    if let Some(seq) = seq {
        let payload = serde_json::json!({"type": "session.result", "session": name, "seq": seq});
        if let Err(e) = event_pub("session.result", &payload.to_string()) {
            eprintln!("jw: {name}: {e}");
        }
    }
    Ok(seq)
}
