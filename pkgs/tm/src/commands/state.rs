use std::fmt::Write;

use crate::parse::with_token;
use crate::store::{Result, Store};
use crate::task::Status;

pub(crate) fn start(store: &mut Store, refs: &[String], out: &mut String) -> Result<()> {
    set_state(
        store,
        refs,
        out,
        |s| (s == Status::Doing).then_some("already doing"),
        |text| format!("- [/]{}", &text[5..]),
    )
}

pub(crate) fn stop(store: &mut Store, refs: &[String], out: &mut String) -> Result<()> {
    set_state(
        store,
        refs,
        out,
        |s| (s == Status::Todo).then_some("not doing"),
        |text| format!("- [ ]{}", &text[5..]),
    )
}

pub(crate) fn done(store: &mut Store, refs: &[String], out: &mut String) -> Result<()> {
    let today = store.today.to_string();
    set_state(
        store,
        refs,
        out,
        |_| None,
        |text| with_token(&format!("- [x]{}", &text[5..]), "done", &today),
    )
}

/// Applies `edit` to every referenced line and writes once; `check` is
/// explained at `resolve_all`.
fn set_state(
    store: &mut Store,
    refs: &[String],
    out: &mut String,
    check: impl Fn(Status) -> Option<&'static str>,
    edit: impl Fn(&str) -> String,
) -> Result<()> {
    if refs.is_empty() {
        return Err("refs must not be empty".to_string());
    }
    let found = store.resolve_all(refs, check)?;
    for &l in &found {
        let line = edit(store.file.text(l));
        store.file.set(l, &line);
    }
    store.save()?;
    print_lines(store, &found, out);
    Ok(())
}

fn print_lines(store: &Store, lines: &[usize], out: &mut String) {
    for &l in lines {
        out!(out, "{}: {}", l + 1, store.file.text(l));
    }
}
