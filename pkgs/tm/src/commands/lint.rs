use std::fmt::Write;

use crate::store::{Result, Store};

/// Returns whether any line has problems.
pub(crate) fn lint(store: &Store, out: &mut String) -> Result<bool> {
    let problems: Vec<String> = (0..store.file.lines.len())
        .filter_map(|l| store.file.task(l).err().map(|e| format!("{}: {e}", l + 1)))
        .collect();
    for p in &problems {
        out!(out, "{p}");
    }
    Ok(!problems.is_empty())
}
