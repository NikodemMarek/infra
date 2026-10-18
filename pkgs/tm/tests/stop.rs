mod common;

use common::{assert_fixture_unchanged, assert_only_lines_changed, Env, FIXTURE, WARDROBE};

#[test]
fn stop_by_substring_keeps_spacing() {
    let e = Env::fixture();
    let o = e.tm(&["stop", "wardrobe"]);
    o.ok();
    let line = "- [ ] stachiewicza wardrobe  @est(4h) #organization";
    assert_eq!(o.stdout, format!("8: {line}\n"));
    let after = e.read("tasks.md");
    assert_eq!(after, FIXTURE.replace(WARDROBE, line));
    assert_only_lines_changed(FIXTURE, &after, &[8]);
}

#[test]
fn stop_by_line_number_and_multiple() {
    let e = Env::fixture();
    let o = e.tm(&["stop", "3", "8"]);
    o.ok();
    assert_eq!(
        o.stdout,
        "3: - [ ] kubernetes cluster runs wireguard @est(4h) @due(2026-10-25) #self-hosting\n\
         8: - [ ] stachiewicza wardrobe  @est(4h) #organization\n"
    );
    assert_only_lines_changed(FIXTURE, &e.read("tasks.md"), &[3, 8]);
}

#[test]
fn stop_not_doing() {
    let e = Env::fixture();
    e.tm(&["stop", "2"]).fail("tm: not doing: line 2");
    e.tm(&["stop", "sunshine"]).fail("tm: not doing: line 4");
    assert_fixture_unchanged(&e);
}

#[test]
fn stop_already_done() {
    let e = Env::fixture();
    e.tm(&["stop", "7"]).fail("tm: already done: line 7");
    e.tm(&["stop", "5"]).fail("tm: already done: line 5");
    assert_fixture_unchanged(&e);
}

#[test]
fn stop_first_error_aborts_and_writes_nothing() {
    let e = Env::fixture();
    let o = e.tm(&["stop", "8", "2"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: not doing: line 2\n");
    assert_eq!(o.stdout, "");
    assert_fixture_unchanged(&e);
}

#[test]
fn stop_same_task_twice_is_error() {
    let e = Env::fixture();
    let o = e.tm(&["stop", "8", "wardrobe"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: same task given twice: wardrobe\n");
    assert_fixture_unchanged(&e);
}

#[test]
fn stop_substring_never_matches_done_task() {
    let e = Env::fixture();
    e.tm(&["stop", "ML"])
        .fail("tm: no open task matches \"ML\"");
    assert_fixture_unchanged(&e);
}

#[test]
fn stop_not_a_task() {
    let e = Env::fixture();
    e.tm(&["stop", "9"]).fail("tm: not a task: line 9");
    e.tm(&["stop", "99"]).fail("tm: not a task: line 99");
    assert_fixture_unchanged(&e);
}

#[test]
fn stop_last_line_without_trailing_newline() {
    let e = Env::empty();
    e.write("tasks.md", "- [ ] first\n- [/] z");
    e.tm(&["stop", "2"]).ok();
    assert_eq!(e.read("tasks.md"), "- [ ] first\n- [ ] z");
}

#[test]
fn stop_without_refs_is_usage_error() {
    let e = Env::fixture();
    e.tm(&["stop"]).usage_error();
    assert_fixture_unchanged(&e);
}

#[test]
fn stop_empty_reference_is_error() {
    let e = Env::fixture();
    e.tm(&["stop", ""]).fail("tm: empty reference");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_then_stop_restores_the_file() {
    let e = Env::fixture();
    e.tm(&["start", "2", "4"]).ok();
    e.tm(&["stop", "2", "4"]).ok();
    assert_fixture_unchanged(&e);
}
