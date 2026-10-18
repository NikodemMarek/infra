mod common;

use common::{assert_fixture_unchanged, with_line, Env, FIXTURE};

#[test]
fn add_appends_with_created() {
    let e = Env::fixture();
    let o = e.tm(&["add", "i sleep better #health/sleep"]);
    o.ok();
    let line = "- [ ] i sleep better #health/sleep @created(2026-10-18)";
    assert_eq!(o.stdout, format!("10: {line}\n"));
    assert_eq!(e.read("tasks.md"), with_line(FIXTURE, line));
}

#[test]
fn add_creates_missing_tasks_file() {
    let e = Env::empty();
    let o = e.tm(&["add", "first"]);
    o.ok();
    assert_eq!(o.stdout, "1: - [ ] first @created(2026-10-18)\n");
    assert_eq!(e.read("tasks.md"), "- [ ] first @created(2026-10-18)\n");
}

#[test]
fn add_unquoted_multiword_is_usage_error() {
    let e = Env::fixture();
    e.tm(&["add", "buy", "milk", "#home"]).usage_error();
    assert_fixture_unchanged(&e);
}

#[test]
fn add_quoted_multiword_text() {
    let e = Env::fixture();
    e.tm(&["add", "buy milk #home"]).ok();
    assert_eq!(
        e.read("tasks.md"),
        with_line(FIXTURE, "- [ ] buy milk #home @created(2026-10-18)")
    );
}

#[test]
fn add_lint_failure_writes_nothing() {
    let e = Env::fixture();
    let o = e.tm(&["add", "x @est(2x)"]);
    o.fail("invalid @est value: 2x");
    assert_fixture_unchanged(&e);
}

#[test]
fn add_keeps_existing_created() {
    let e = Env::fixture();
    e.tm(&["add", "x @created(2026-01-01)"]).ok();
    assert_eq!(
        e.read("tasks.md"),
        with_line(FIXTURE, "- [ ] x @created(2026-01-01)")
    );
}

#[test]
fn add_week_token_is_unknown() {
    let e = Env::fixture();
    e.tm(&["add", "x @week(2026-w43)"])
        .fail("tm: unknown token @week");
    assert_fixture_unchanged(&e);
}

#[test]
fn add_drop_token_is_unknown() {
    let e = Env::fixture();
    e.tm(&["add", "x @drop(2026-w41)"])
        .fail("tm: unknown token @drop");
    assert_fixture_unchanged(&e);
}

#[test]
fn add_accepts_large_est() {
    let e = Env::fixture();
    let o = e.tm(&["add", "big @est(3d)"]);
    o.ok();
    assert_eq!(o.stdout, "10: - [ ] big @est(3d) @created(2026-10-18)\n");
}

#[test]
fn add_rejects_checkbox_text() {
    let e = Env::fixture();
    e.tm(&["add", "- [ ] foo"])
        .fail("tm: give the text without the checkbox");
    assert_fixture_unchanged(&e);
}

#[test]
fn add_rejects_doing_checkbox_text() {
    let e = Env::fixture();
    e.tm(&["add", "- [/] x"])
        .fail("tm: give the text without the checkbox");
    assert_fixture_unchanged(&e);
}

#[test]
fn add_without_text_is_usage_error() {
    let e = Env::fixture();
    e.tm(&["add"]).usage_error();
    assert_fixture_unchanged(&e);
}

#[test]
fn add_adds_newline_to_unterminated_file() {
    let e = Env::empty();
    e.write("tasks.md", "# Tasks\n- [ ] a");
    let o = e.tm(&["add", "b"]);
    o.ok();
    assert_eq!(o.stdout, "3: - [ ] b @created(2026-10-18)\n");
    assert_eq!(
        e.read("tasks.md"),
        "# Tasks\n- [ ] a\n- [ ] b @created(2026-10-18)\n"
    );
}

#[test]
fn add_rejects_multiline_text() {
    for bad in ["a\nb", "a\rb"] {
        let e = Env::fixture();
        e.tm(&["add", bad]).fail("tm: text must be a single line");
        assert_fixture_unchanged(&e);
    }
}

#[test]
fn add_empty_text_fails_in_core() {
    let e = Env::fixture();
    e.tm(&["add", ""]).fail("tm: the text must not be empty");
    assert_fixture_unchanged(&e);
}

#[test]
fn add_whitespace_text_fails_in_core() {
    let e = Env::fixture();
    e.tm(&["add", "   "]).fail("tm: the text must not be empty");
    assert_fixture_unchanged(&e);
}

#[test]
fn add_reports_only_first_problem() {
    let e = Env::fixture();
    let o = e.tm(&["add", "x @est(2x) @project(a)"]);
    assert_eq!(o.code, 1, "stdout: {}", o.stdout);
    assert_eq!(o.stderr, "tm: invalid @est value: 2x\n");
    assert_fixture_unchanged(&e);
}
