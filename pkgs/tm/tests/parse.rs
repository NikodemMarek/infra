use std::collections::BTreeSet;
use tm::{lint_line, parse_line, with_token, Task};

fn toks(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn tags(t: &[&str]) -> Vec<String> {
    t.iter().map(|s| s.to_string()).collect()
}

fn check(line: &str, done: bool, desc: &str, tokens: &[(&str, &str)], tg: &[&str]) {
    let t: Task = parse_line(line).unwrap_or_else(|| panic!("expected a task: {line:?}"));
    assert_eq!(t.done, done, "done for {line:?}");
    assert_eq!(t.description, desc, "description for {line:?}");
    assert_eq!(t.tokens, toks(tokens), "tokens for {line:?}");
    assert_eq!(t.tags, tags(tg), "tags for {line:?}");
}

#[test]
fn parse_trailing_est_and_tag() {
    check(
        "- [ ] stachiewicza wardrobe is mapped out  @est(4h) #organization",
        false,
        "stachiewicza wardrobe is mapped out",
        &[("est", "4h")],
        &["organization"],
    );
}

#[test]
fn parse_value_with_spaces() {
    let line = "- [ ] schema for clothes @est(1h) @project(inventory management system) @drop(2026-w41) #organization";
    check(
        line,
        false,
        "schema for clothes",
        &[
            ("est", "1h"),
            ("project", "inventory management system"),
            ("drop", "2026-w41"),
        ],
        &["organization"],
    );
    let t = parse_line(line).unwrap();
    assert_eq!(t.token("est"), Some("1h"));
    assert_eq!(t.token("project"), Some("inventory management system"));
    assert_eq!(t.token("drop"), Some("2026-w41"));
    assert_eq!(t.token("due"), None);
}

#[test]
fn parse_hash_inside_description() {
    check(
        "- [x] fix #1 bug @est(1h)",
        true,
        "fix #1 bug",
        &[("est", "1h")],
        &[],
    );
}

#[test]
fn parse_capital_x_is_done() {
    check("- [X] a", true, "a", &[], &[]);
}

#[test]
fn parse_token_in_middle_is_description() {
    check(
        "- [ ] foo @est(1h) bar #t",
        false,
        "foo @est(1h) bar",
        &[],
        &["t"],
    );
}

#[test]
fn parse_bare_at_word_is_plain() {
    check("- [ ] call @mom", false, "call @mom", &[], &[]);
}

#[test]
fn parse_token_followed_by_chars_is_plain() {
    check("- [ ] a @est(2h)x", false, "a @est(2h)x", &[], &[]);
}

#[test]
fn parse_empty_task() {
    check("- [ ]", false, "", &[], &[]);
}

#[test]
fn parse_plain_words_with_hash_and_at() {
    check(
        "- [ ] C# me@x.com # #t",
        false,
        "C# me@x.com #",
        &[],
        &["t"],
    );
}

#[test]
fn parse_non_tasks_return_none() {
    for l in [
        "  - [ ] x",
        "- [-] x",
        "* [ ] x",
        "just text",
        "-[ ] x",
        "- [ ]foo",
        "",
    ] {
        assert!(parse_line(l).is_none(), "should be None: {l:?}");
    }
}

#[test]
fn est_hours_cases() {
    let h = |l: &str| parse_line(l).unwrap().est_hours();
    assert_eq!(h("- [ ] a @est(0.5h)"), Some(0.5));
    assert_eq!(h("- [ ] a @est(2h)"), Some(2.0));
    assert_eq!(h("- [ ] a @est(3d)"), None);
    assert_eq!(h("- [ ] a"), None);
    assert_eq!(h("- [ ] a @est(2x)"), None);
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

fn lint(l: &str) -> BTreeSet<String> {
    lint_line(l).into_iter().collect()
}

fn expect(l: &str, msgs: &[&str]) {
    let want: BTreeSet<String> = msgs.iter().map(|s| s.to_string()).collect();
    assert_eq!(lint(l), want, "lint of {l:?}");
}

#[test]
fn lint_valid_line() {
    expect("- [ ] ok @est(0.5h) @due(2026-10-20) #a/b", &[]);
    assert!(lint_line("- [ ] ok @est(0.5h) @due(2026-10-20) #a/b").is_empty());
}

#[test]
fn lint_valid_est_units() {
    for v in ["1h", "3d", "2m", "1y", "1.5h"] {
        expect(&format!("- [ ] x @est({v})"), &[]);
    }
}

#[test]
fn lint_unknown_checkbox() {
    expect("- [-] x", &["unknown checkbox: [-]"]);
    expect("- [?] x", &["unknown checkbox: [?]"]);
}

#[test]
fn lint_empty_description() {
    expect("- [ ] @est(1h)", &["empty description"]);
}

#[test]
fn lint_invalid_est() {
    for v in ["2x", "0h", ".5h", "1,5h"] {
        expect(
            &format!("- [ ] x @est({v})"),
            &[&format!("invalid @est value: {v}")],
        );
    }
}

#[test]
fn lint_invalid_dates() {
    for n in ["due", "created", "done"] {
        expect(
            &format!("- [ ] x @{n}(2026-02-30)"),
            &[&format!("invalid date in @{n}: 2026-02-30")],
        );
        expect(&format!("- [ ] x @{n}(2026-02-28)"), &[]);
    }
}

#[test]
fn lint_drop_values() {
    expect(
        "- [ ] x @drop(2026-w54)",
        &["invalid @drop value: 2026-w54"],
    );
    expect(
        "- [ ] x @drop(2025-w53)",
        &["invalid @drop value: 2025-w53"],
    );
    expect("- [ ] x @drop(2026-w53)", &[]);
    expect("- [ ] x @drop(2026-w41)", &[]);
}

#[test]
fn lint_step() {
    expect("- [ ] x @proj(a) @step(0)", &["invalid @step value: 0"]);
    expect("- [ ] x @step(1)", &["@step without @proj"]);
    expect("- [ ] x @proj(a) @step(1)", &[]);
}

#[test]
fn lint_empty_proj() {
    expect("- [ ] x @proj()", &["empty @proj"]);
}

#[test]
fn lint_duplicate_token() {
    expect("- [ ] x @est(1h) @est(2h)", &["duplicate token @est"]);
}

#[test]
fn lint_unknown_token() {
    expect("- [ ] x @project(foo)", &["unknown token @project"]);
}

#[test]
fn lint_token_inside_description() {
    expect(
        "- [ ] x @est(1h) y",
        &["token inside description: @est(1h)"],
    );
}

#[test]
fn lint_invalid_tag() {
    expect("- [ ] x #a//b", &["invalid tag: #a//b"]);
    expect("- [ ] x #a/", &["invalid tag: #a/"]);
}

#[test]
fn lint_non_task_lines() {
    expect("just text", &[]);
    expect("---", &[]);
}
