mod common;

use common::{
    assert_fixture_unchanged, assert_only_lines_changed, nums, with_line, Env, FIXTURE, KUBE,
    SUNSHINE, WIREGUARD,
};

#[test]
fn start_by_substring() {
    let e = Env::fixture();
    let o = e.tm(&["start", "sunshine"]);
    o.ok();
    let line = "- [/] sunshine on headless vm @est(3d) #self-hosting";
    assert_eq!(o.stdout, format!("4: {line}\n"));
    let after = e.read("tasks.md");
    assert_eq!(after, FIXTURE.replace(SUNSHINE, line));
    assert_only_lines_changed(FIXTURE, &after, &[4]);
}

#[test]
fn start_multiple_refs_in_given_order() {
    let e = Env::fixture();
    let o = e.tm(&["start", "2", "4"]);
    o.ok();
    assert_eq!(
        o.stdout,
        "2: - [/] wireguard container forwards to mullvad @est(2h) #self-hosting\n\
         4: - [/] sunshine on headless vm @est(3d) #self-hosting\n"
    );
    assert_only_lines_changed(FIXTURE, &e.read("tasks.md"), &[2, 4]);

    let e = Env::fixture();
    let o = e.tm(&["start", "4", "2"]);
    o.ok();
    assert_eq!(nums(&o.stdout), ["4", "2"], "{}", o.stdout);
}

#[test]
fn start_only_changes_the_checkbox() {
    let e = Env::empty();
    e.write("tasks.md", "- [ ] a  @est(1h) #t @due(2026-10-20)\n");
    e.tm(&["start", "1"]).ok();
    assert_eq!(
        e.read("tasks.md"),
        "- [/] a  @est(1h) #t @due(2026-10-20)\n"
    );
}

#[test]
fn start_already_doing() {
    let e = Env::fixture();
    e.tm(&["start", "kubernetes"])
        .fail("tm: already doing: line 3");
    assert_fixture_unchanged(&e);
    e.tm(&["start", "3"]).fail("tm: already doing: line 3");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_ambiguous_includes_doing_task() {
    let e = Env::fixture();
    let o = e.tm(&["start", "wireguard"]);
    o.fail("tm: ambiguous \"wireguard\", 2 matches:");
    assert!(
        o.stderr.contains(&format!("\n2: {WIREGUARD}")),
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains(&format!("\n3: {KUBE}")), "{}", o.stderr);
    assert_fixture_unchanged(&e);
}

#[test]
fn start_substring_matches_doing_task() {
    let e = Env::fixture();
    e.tm(&["start", "wardrobe"])
        .fail("tm: already doing: line 8");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_already_done() {
    let e = Env::fixture();
    e.tm(&["start", "5"]).fail("tm: already done: line 5");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_capital_x_is_already_done() {
    let e = Env::empty();
    e.write("tasks.md", "- [X] a\n");
    e.tm(&["start", "1"]).fail("tm: already done: line 1");
    assert_eq!(e.read("tasks.md"), "- [X] a\n");
}

#[test]
fn start_substring_never_matches_done_task() {
    let e = Env::fixture();
    e.tm(&["start", "read ML"])
        .fail("tm: no open task matches \"read ML\"");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_not_a_task() {
    let e = Env::fixture();
    e.tm(&["start", "9"]).fail("tm: not a task: line 9");
    e.tm(&["start", "1"]).fail("tm: not a task: line 1");
    e.tm(&["start", "0"]).fail("tm: not a task: line 0");
    e.tm(&["start", "99"]).fail("tm: not a task: line 99");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_same_task_twice_is_error() {
    let e = Env::fixture();
    let o = e.tm(&["start", "4", "4"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: same task given twice: 4\n");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_same_task_twice_reports_later_ref_as_given() {
    let e = Env::fixture();
    let o = e.tm(&["start", "sunshine", "4"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: same task given twice: 4\n");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_first_error_aborts_and_writes_nothing() {
    let e = Env::fixture();
    let o = e.tm(&["start", "4", "3"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: already doing: line 3\n");
    assert_eq!(o.stdout, "");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_state_check_of_first_ref_comes_before_later_refs() {
    let e = Env::fixture();
    let o = e.tm(&["start", "3", "3"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: already doing: line 3\n");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_unresolvable_later_ref_writes_nothing() {
    let e = Env::fixture();
    e.tm(&["start", "4", "nonexistent thing"])
        .fail("no open task matches");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_last_line_without_trailing_newline() {
    let e = Env::empty();
    e.write("tasks.md", "- [ ] first\n- [ ] z");
    e.tm(&["start", "2"]).ok();
    assert_eq!(e.read("tasks.md"), "- [ ] first\n- [/] z");

    let e = Env::empty();
    e.write("tasks.md", "- [ ] z");
    e.tm(&["start", "1"]).ok();
    assert_eq!(e.read("tasks.md"), "- [/] z");
}

#[test]
fn start_without_refs_is_usage_error() {
    let e = Env::fixture();
    e.tm(&["start"]).usage_error();
    assert_fixture_unchanged(&e);
}

#[test]
fn start_empty_reference_is_error() {
    let e = Env::fixture();
    e.tm(&["start", ""]).fail("tm: empty reference");
    e.tm(&["start", " "]).fail("tm: empty reference");
    assert_fixture_unchanged(&e);
}

#[test]
fn start_ref_to_invalid_line_is_hard_error() {
    let e = Env::fixture();
    let content = with_line(FIXTURE, "- [ ] x @project(foo)");
    e.write("tasks.md", &content);
    let o = e.tm(&["start", "10"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: line 10: unknown token @project\n");
    assert_eq!(e.read("tasks.md"), content);
}

#[test]
fn start_skips_invalid_line_for_substring_refs() {
    let e = Env::fixture();
    let content = with_line(FIXTURE, "- [ ] fix gutter @project(a)");
    e.write("tasks.md", &content);
    let skip = "tm: skipping line 10: unknown token @project\n";

    let o = e.tm(&["start", "gutter"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(
        o.stderr,
        format!("{skip}tm: no open task matches \"gutter\"\n")
    );
    assert_eq!(e.read("tasks.md"), content);

    let o = e.tm(&["start", "sunshine"]);
    o.ok();
    assert!(o.stderr.contains(skip), "{}", o.stderr);
    assert_only_lines_changed(&content, &e.read("tasks.md"), &[4]);
}
