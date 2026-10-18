mod common;

use common::{with_line, Env, FIXTURE};

#[test]
fn lint_clean_fixture_with_doing_lines() {
    let e = Env::fixture();
    let o = e.tm(&["lint"]);
    o.ok();
    assert_eq!(o.stdout, "");
}

#[test]
fn lint_unknown_token() {
    let e = Env::fixture();
    e.write("tasks.md", &with_line(FIXTURE, "- [ ] x @project(foo)"));
    let o = e.tm(&["lint"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stdout, "10: unknown token @project\n");
}

#[test]
fn lint_reports_week_and_drop_as_unknown_tokens() {
    let e = Env::fixture();
    e.write(
        "tasks.md",
        &with_line(
            &with_line(FIXTURE, "- [ ] x @week(2026-w42)"),
            "- [/] y @drop(2026-w41)",
        ),
    );
    let o = e.tm(&["lint"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(
        o.stdout,
        "10: unknown token @week\n11: unknown token @drop\n"
    );
}

#[test]
fn lint_unknown_checkbox() {
    let e = Env::fixture();
    e.write("tasks.md", &with_line(FIXTURE, "- [-] x"));
    let o = e.tm(&["lint"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stdout, "10: unknown checkbox: [-]\n");
}

#[test]
fn lint_missing_file_is_clean() {
    let e = Env::empty();
    let o = e.tm(&["lint"]);
    o.ok();
    assert_eq!(o.stdout, "");
}

#[test]
fn lint_reports_only_first_error_per_line() {
    let e = Env::fixture();
    e.write(
        "tasks.md",
        &with_line(FIXTURE, "- [ ] x @project(foo) @est(2x)"),
    );
    let o = e.tm(&["lint"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stdout, "10: unknown token @project\n");
}
