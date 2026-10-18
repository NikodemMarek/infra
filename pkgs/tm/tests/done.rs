mod common;

use common::{
    assert_fixture_unchanged, assert_only_lines_changed, with_line, Env, FIXTURE, KUBE, WIREGUARD,
};

#[test]
fn done_by_substring() {
    let e = Env::fixture();
    let o = e.tm(&["done", "blog"]);
    o.ok();
    let line = "- [x] write a blog post @est(2h) #fun @done(2026-10-18)";
    assert_eq!(o.stdout, format!("6: {line}\n"));
    let after = e.read("tasks.md");
    assert_eq!(
        after,
        FIXTURE.replace("- [ ] write a blog post @est(2h) #fun", line)
    );
    assert_only_lines_changed(FIXTURE, &after, &[6]);
}

#[test]
fn done_on_doing_task() {
    let e = Env::fixture();
    let o = e.tm(&["done", "kubernetes"]);
    o.ok();
    let line = "- [x] kubernetes cluster runs wireguard @est(4h) @due(2026-10-25) #self-hosting @done(2026-10-18)";
    assert_eq!(o.stdout, format!("3: {line}\n"));
    let after = e.read("tasks.md");
    assert_eq!(after.lines().nth(2), Some(line));
    assert_only_lines_changed(FIXTURE, &after, &[3]);
}

#[test]
fn done_on_doing_task_keeps_spacing() {
    let e = Env::fixture();
    e.tm(&["done", "8"]).ok();
    assert_eq!(
        e.read("tasks.md").lines().nth(7),
        Some("- [x] stachiewicza wardrobe  @est(4h) #organization @done(2026-10-18)")
    );
}

#[test]
fn done_replaces_existing_done_token_in_place() {
    let e = Env::empty();
    e.write("tasks.md", "- [/] a @done(2026-01-01) #t\n");
    e.tm(&["done", "1"]).ok();
    assert_eq!(e.read("tasks.md"), "- [x] a @done(2026-10-18) #t\n");
}

#[test]
fn done_ambiguous() {
    let e = Env::fixture();
    let o = e.tm(&["done", "wireguard"]);
    o.fail("ambiguous \"wireguard\", 2 matches:");
    assert!(
        o.stderr.contains(&format!("\n2: {WIREGUARD}")),
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains(&format!("\n3: {KUBE}")), "{}", o.stderr);
    assert_fixture_unchanged(&e);
}

#[test]
fn done_no_match() {
    let e = Env::fixture();
    let o = e.tm(&["done", "nonexistent thing"]);
    o.fail("tm: no open task matches \"nonexistent thing\"");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_substring_never_matches_done_task() {
    let e = Env::fixture();
    e.tm(&["done", "read ML"])
        .fail("tm: no open task matches \"read ML\"");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_exact_description_wins_over_substrings() {
    let e = Env::empty();
    e.write("tasks.md", "- [ ] fix\n- [ ] fix the thing\n");
    let o = e.tm(&["done", "FIX"]);
    o.ok();
    assert_eq!(o.stdout, "1: - [x] fix @done(2026-10-18)\n");
    assert_eq!(
        e.read("tasks.md"),
        "- [x] fix @done(2026-10-18)\n- [ ] fix the thing\n"
    );
}

#[test]
fn done_by_line_number() {
    let e = Env::fixture();
    let o = e.tm(&["done", "2"]);
    o.ok();
    let line =
        "- [x] wireguard container forwards to mullvad @est(2h) #self-hosting @done(2026-10-18)";
    assert_eq!(o.stdout, format!("2: {line}\n"));
    let after = e.read("tasks.md");
    assert_only_lines_changed(FIXTURE, &after, &[2]);
    assert_eq!(after.lines().nth(1), Some(line));
}

#[test]
fn done_already_done() {
    let e = Env::fixture();
    e.tm(&["done", "5"]).fail("tm: already done: line 5");
    e.tm(&["done", "7"]).fail("tm: already done: line 7");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_not_a_task() {
    let e = Env::fixture();
    e.tm(&["done", "10"]).fail("tm: not a task: line 10");
    e.tm(&["done", "9"]).fail("tm: not a task: line 9");
    e.tm(&["done", "1"]).fail("tm: not a task: line 1");
    e.tm(&["done", "0"]).fail("tm: not a task: line 0");
    e.tm(&["done", "99"]).fail("tm: not a task: line 99");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_line_number_is_normalized() {
    let e = Env::fixture();
    e.tm(&["done", "05"]).fail("tm: already done: line 5");
    e.tm(&["done", "011"]).fail("tm: not a task: line 11");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_ambiguous_identical_descriptions() {
    let e = Env::empty();
    e.write("tasks.md", "- [ ] same\n- [ ] same\n");
    let o = e.tm(&["done", "same"]);
    o.fail("tm: ambiguous \"same\", 2 matches:");
    assert!(o.stderr.contains("1: - [ ] same"), "{}", o.stderr);
    assert!(o.stderr.contains("2: - [ ] same"), "{}", o.stderr);
    assert_eq!(e.read("tasks.md"), "- [ ] same\n- [ ] same\n");
}

#[test]
fn done_leading_space_ref_is_substring_not_line() {
    let e = Env::fixture();
    e.tm(&["done", " 2"])
        .fail("tm: no open task matches \" 2\"");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_multiple_refs() {
    let e = Env::fixture();
    let o = e.tm(&["done", "blog", "2"]);
    o.ok();
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "6: - [x] write a blog post @est(2h) #fun @done(2026-10-18)",
            "2: - [x] wireguard container forwards to mullvad @est(2h) #self-hosting @done(2026-10-18)",
        ]
    );
    assert_only_lines_changed(FIXTURE, &e.read("tasks.md"), &[2, 6]);
}

#[test]
fn done_is_all_or_nothing() {
    let e = Env::fixture();
    e.tm(&["done", "blog", "nonexistent thing"])
        .fail("no open task matches");
    assert_fixture_unchanged(&e);
    e.tm(&["done", "blog", "5"]).fail("already done: line 5");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_same_task_twice_is_error() {
    let e = Env::fixture();
    let o = e.tm(&["done", "blog", "6"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: same task given twice: 6\n");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_last_line_without_trailing_newline() {
    let e = Env::empty();
    e.write("tasks.md", "- [ ] first\n- [ ] last");
    let o = e.tm(&["done", "last"]);
    o.ok();
    assert_eq!(o.stdout, "2: - [x] last @done(2026-10-18)\n");
    assert_eq!(
        e.read("tasks.md"),
        "- [ ] first\n- [x] last @done(2026-10-18)"
    );
}

#[test]
fn done_without_refs_is_usage_error() {
    let e = Env::fixture();
    e.tm(&["done"]).usage_error();
    assert_fixture_unchanged(&e);
}

#[test]
fn empty_reference_is_error() {
    let e = Env::fixture();
    e.tm(&["done", ""]).fail("tm: empty reference");
    e.tm(&["done", "  "]).fail("tm: empty reference");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_ref_to_invalid_line_is_hard_error() {
    let e = Env::fixture();
    let content = with_line(FIXTURE, "- [ ] x @project(foo)");
    e.write("tasks.md", &content);
    let o = e.tm(&["done", "10"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: line 10: unknown token @project\n");
    assert_eq!(e.read("tasks.md"), content);
}
