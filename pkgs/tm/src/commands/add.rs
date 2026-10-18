use std::fmt::Write;

use crate::store::{Result, Store};

pub(crate) fn add(store: &mut Store, text: &str, out: &mut String) -> Result<()> {
    if text.trim().is_empty() {
        return Err("the text must not be empty".to_string());
    }
    if text.contains(['\n', '\r']) {
        return Err("text must be a single line".to_string());
    }
    if text.starts_with("- [") {
        return Err("give the text without the checkbox".to_string());
    }
    let line = store.new_task(&format!("- [ ] {text}"))?;
    let number = store.file.append(&line);
    store.save()?;
    out!(out, "{number}: {line}");
    Ok(())
}
