use chrono::NaiveDate;
use tm::{parse_line, with_token, Est, Task, Unit, Week};

fn tags(t: &[&str]) -> Vec<String> {
    t.iter().map(|s| s.to_string()).collect()
}

fn ok(line: &str) -> Task {
    parse_line(line)
        .unwrap_or_else(|e| panic!("unexpected error for {line:?}: {e}"))
        .unwrap_or_else(|| panic!("expected a task: {line:?}"))
}

fn err(line: &str) -> String {
    parse_line(line).unwrap_err().to_string()
}

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn est(amount: f64, unit: Unit) -> Option<Est> {
    Some(Est { amount, unit })
}

fn check(line: &str, done: bool, desc: &str, tg: &[&str]) -> Task {
    let t = ok(line);
    assert_eq!(t.done, done, "done for {line:?}");
    assert_eq!(t.description, desc, "description for {line:?}");
    assert_eq!(t.tags, tags(tg), "tags for {line:?}");
    t
}

#[test]
fn parse_trailing_est_and_tag() {
    let t = check(
        "- [ ] stachiewicza wardrobe is mapped out  @est(4h) #organization",
        false,
        "stachiewicza wardrobe is mapped out",
        &["organization"],
    );
    assert_eq!(t.est, est(4.0, Unit::Hours));
}

#[test]
fn parse_value_with_spaces() {
    let t = check(
        "- [ ] schema for clothes @est(1h) @proj(inventory management system) @drop(2026-w41) #organization",
        false,
        "schema for clothes",
        &["organization"],
    );
    assert_eq!(t.proj.as_deref(), Some("inventory management system"));
    assert_eq!(
        t.drop,
        Some(Week {
            year: 2026,
            week: 41
        })
    );
    assert_eq!(t.est, est(1.0, Unit::Hours));
    assert_eq!(t.due, None);
}

#[test]
fn parse_hash_inside_description() {
    let t = check("- [x] fix #1 bug @est(1h)", true, "fix #1 bug", &[]);
    assert_eq!(t.est, est(1.0, Unit::Hours));
}

#[test]
fn parse_capital_x_is_done() {
    check("- [X] a", true, "a", &[]);
}

#[test]
fn parse_token_in_middle_is_error() {
    assert_eq!(
        err("- [ ] foo @est(1h) bar #t"),
        "token inside description: @est(1h)"
    );
}

#[test]
fn parse_token_with_spaces_in_middle_is_quoted_whole() {
    assert_eq!(
        err("- [ ] a @proj(x y) b"),
        "token inside description: @proj(x y)"
    );
}

#[test]
fn parse_bare_at_word_is_plain() {
    check("- [ ] call @mom", false, "call @mom", &[]);
}

#[test]
fn parse_token_followed_by_chars_is_error() {
    assert_eq!(
        err("- [ ] a @est(2h)x"),
        "token inside description: @est(2h)x"
    );
}

#[test]
fn parse_empty_task() {
    assert_eq!(err("- [ ]"), "empty description");
}

#[test]
fn parse_plain_words_with_hash_and_at() {
    check("- [ ] C# me@x.com # #t", false, "C# me@x.com #", &["t"]);
}

#[test]
fn parse_non_tasks_return_none() {
    for l in [
        "  - [ ] x",
        "* [ ] x",
        "just text",
        "-[ ] x",
        "- [ ]foo",
        "- [link](url)",
        "---",
        "",
    ] {
        assert_eq!(parse_line(l), Ok(None), "should be Ok(None): {l:?}");
    }
}

#[test]
fn parse_unknown_checkbox_is_error() {
    assert_eq!(err("- [-] x"), "unknown checkbox: [-]");
}

#[test]
fn est_hours_cases() {
    let h = |l: &str| ok(l).est_hours();
    assert_eq!(h("- [ ] a @est(0.5h)"), Some(0.5));
    assert_eq!(h("- [ ] a @est(2h)"), Some(2.0));
    assert_eq!(h("- [ ] a @est(3d)"), None);
    assert_eq!(h("- [ ] a"), None);
    assert_eq!(err("- [ ] a @est(2x)"), "invalid @est value: 2x");
}

#[test]
fn typed_values() {
    let t = ok("- [ ] ok @est(0.5h) @due(2026-10-20) @created(2026-10-01) #a/b");
    assert_eq!(t.est, est(0.5, Unit::Hours));
    assert_eq!(t.est.unwrap().to_string(), "0.5h");
    assert_eq!(t.due, Some(d("2026-10-20")));
    assert_eq!(t.created, Some(d("2026-10-01")));
    assert_eq!(t.tags, tags(&["a/b"]));
}

#[test]
fn typed_done_on() {
    let t = ok("- [x] a @done(2026-10-18)");
    assert_eq!(t.done_on, Some(d("2026-10-18")));
}

#[test]
fn typed_step() {
    let t = ok("- [ ] x @proj(a) @step(3)");
    assert_eq!(t.step, Some(3));
    assert_eq!(t.proj.as_deref(), Some("a"));
}

#[test]
fn typed_est_units() {
    for (v, unit) in [
        ("3d", Unit::Days),
        ("2m", Unit::Months),
        ("1y", Unit::Years),
    ] {
        let t = ok(&format!("- [ ] x @est({v})"));
        assert_eq!(t.est.unwrap().unit, unit, "unit for {v}");
        assert_eq!(t.est.unwrap().to_string(), v);
        assert_eq!(t.est_hours(), None, "est_hours for {v}");
    }
}

#[test]
fn typed_est_display_normalised() {
    let t = ok("- [ ] x @est(1.50h)");
    assert_eq!(t.est.unwrap().to_string(), "1.5h");
}

#[test]
fn step_overflow_is_error() {
    assert_eq!(
        err("- [ ] x @proj(a) @step(4294967296)"),
        "invalid @step value: 4294967296"
    );
}

#[test]
fn whitespace_only_proj_is_error() {
    assert_eq!(err("- [ ] x @proj(  )"), "empty @proj");
}

#[test]
fn first_error_wins() {
    for (l, msg) in [
        ("- [ ] @project(x) @est(2x)", "empty description"),
        (
            "- [ ] a @est(1h) b @project(x)",
            "token inside description: @est(1h)",
        ),
        ("- [ ] x @project(a) @est(2x)", "unknown token @project"),
        ("- [ ] x @est(2x) @project(a)", "invalid @est value: 2x"),
        ("- [ ] x @est(1h) @est(2x)", "duplicate token @est"),
        ("- [ ] x #a//b @project(c)", "invalid tag: #a//b"),
        ("- [ ] x @step(0)", "invalid @step value: 0"),
    ] {
        assert_eq!(err(l), msg, "error for {l:?}");
    }
}

#[test]
fn with_token_append() {
    assert_eq!(
        with_token("- [ ] a #t", "done", "2026-10-18"),
        "- [ ] a #t @done(2026-10-18)"
    );
}

#[test]
fn with_token_replace_in_place() {
    assert_eq!(
        with_token("- [ ] a @drop(2026-w41) #t", "drop", "2026-w42"),
        "- [ ] a @drop(2026-w42) #t"
    );
}

#[test]
fn with_token_trims_trailing_whitespace() {
    assert_eq!(
        with_token("- [ ] a  ", "done", "2026-10-18"),
        "- [ ] a @done(2026-10-18)"
    );
}

#[test]
fn with_token_ignores_description_tokens() {
    assert_eq!(
        with_token("- [ ] a @done(x) b", "done", "2026-10-18"),
        "- [ ] a @done(x) b @done(2026-10-18)"
    );
}

#[test]
fn with_token_keeps_rest_byte_identical() {
    assert_eq!(
        with_token("- [ ] wardrobe  @est(4h) #o", "created", "2026-10-18"),
        "- [ ] wardrobe  @est(4h) #o @created(2026-10-18)"
    );
}

#[test]
fn lint_valid_line() {
    ok("- [ ] ok @est(0.5h) @due(2026-10-20) #a/b");
}

#[test]
fn lint_valid_est_units() {
    for v in ["1h", "3d", "2m", "1y", "1.5h"] {
        ok(&format!("- [ ] x @est({v})"));
    }
}

#[test]
fn lint_unknown_checkbox() {
    assert_eq!(err("- [-] x"), "unknown checkbox: [-]");
    assert_eq!(err("- [?] x"), "unknown checkbox: [?]");
}

#[test]
fn lint_empty_description() {
    assert_eq!(err("- [ ] @est(1h)"), "empty description");
}

#[test]
fn lint_invalid_est() {
    for v in ["2x", "0h", ".5h", "1,5h"] {
        assert_eq!(
            err(&format!("- [ ] x @est({v})")),
            format!("invalid @est value: {v}")
        );
    }
}

#[test]
fn lint_invalid_dates() {
    for n in ["due", "created", "done"] {
        assert_eq!(
            err(&format!("- [ ] x @{n}(2026-02-30)")),
            format!("invalid date in @{n}: 2026-02-30")
        );
        ok(&format!("- [ ] x @{n}(2026-02-28)"));
    }
}

#[test]
fn lint_drop_values() {
    assert_eq!(
        err("- [ ] x @drop(2026-w54)"),
        "invalid @drop value: 2026-w54"
    );
    assert_eq!(
        err("- [ ] x @drop(2025-w53)"),
        "invalid @drop value: 2025-w53"
    );
    ok("- [ ] x @drop(2026-w53)");
    ok("- [ ] x @drop(2026-w41)");
}

#[test]
fn lint_step() {
    assert_eq!(err("- [ ] x @proj(a) @step(0)"), "invalid @step value: 0");
    assert_eq!(err("- [ ] x @step(1)"), "@step without @proj");
    ok("- [ ] x @proj(a) @step(1)");
}

#[test]
fn lint_empty_proj() {
    assert_eq!(err("- [ ] x @proj()"), "empty @proj");
}

#[test]
fn lint_duplicate_token() {
    assert_eq!(err("- [ ] x @est(1h) @est(2h)"), "duplicate token @est");
}

#[test]
fn lint_unknown_token() {
    assert_eq!(err("- [ ] x @project(foo)"), "unknown token @project");
}

#[test]
fn lint_token_inside_description() {
    assert_eq!(
        err("- [ ] x @est(1h) y"),
        "token inside description: @est(1h)"
    );
}

#[test]
fn lint_invalid_tag() {
    assert_eq!(err("- [ ] x #a//b"), "invalid tag: #a//b");
    assert_eq!(err("- [ ] x #a/"), "invalid tag: #a/");
}

#[test]
fn lint_non_task_lines() {
    assert_eq!(parse_line("just text"), Ok(None));
    assert_eq!(parse_line("---"), Ok(None));
}
