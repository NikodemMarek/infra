use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const FIXTURE: &str = "# Tasks
- [ ] wireguard container forwards to mullvad @est(2h) #self-hosting
- [ ] kubernetes cluster runs wireguard @est(4h) @due(2026-10-25) #self-hosting
- [ ] sunshine on headless vm @est(3d) #self-hosting
- [x] old done thing @est(1h)
- [x] migrate secrets @est(4h) @week(2026-w41)
- [x] clean up longhorn @est(1h) @week(2026-w41)
- [ ] write a blog post @est(2h) #fun @drop(2026-w41) @week(2026-w42)
- [x] read ML slides @est(2h) @due(2026-10-14) #university @week(2026-w42)
- [ ] stachiewicza wardrobe  @est(4h) @drop(2026-w41) #organization @week(2026-w42)
notes here
";

const KUBE: &str =
    "- [ ] kubernetes cluster runs wireguard @est(4h) @due(2026-10-25) #self-hosting";

const BLOG: &str = "- [ ] write a blog post @est(2h) #fun @drop(2026-w41) @week(2026-w42)";

const WARDROBE: &str =
    "- [ ] stachiewicza wardrobe  @est(4h) @drop(2026-w41) #organization @week(2026-w42)";

struct Env {
    dir: PathBuf,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Env {
    fn empty() -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("tm-test-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Env { dir }
    }

    fn fixture() -> Env {
        let e = Env::empty();
        e.write("tasks.md", FIXTURE);
        e
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.dir.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
    }

    fn exists(&self, rel: &str) -> bool {
        self.dir.join(rel).exists()
    }

    fn run_in(&self, dir: &Path, today: &str, args: &[&str]) -> Out {
        let o = Command::new(env!("CARGO_BIN_EXE_tm"))
            .args(args)
            .env("TM_DIR", dir)
            .env("TM_TODAY", today)
            .output()
            .unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn tm(&self, args: &[&str]) -> Out {
        self.run_in(&self.dir, "2026-10-18", args)
    }

    fn tm_on(&self, today: &str, args: &[&str]) -> Out {
        self.run_in(&self.dir, today, args)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Out {
    fn ok(&self) -> &Out {
        assert_eq!(
            self.code, 0,
            "expected exit 0\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        self
    }

    fn fail(&self, stderr_has: &str) -> &Out {
        assert_eq!(
            self.code, 1,
            "expected exit 1\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        assert!(
            self.stderr.contains(stderr_has),
            "stderr should contain {stderr_has:?}, got: {}",
            self.stderr
        );
        self
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("bad json ({e}): {}\nstderr: {}", self.stdout, self.stderr))
    }
}

#[test]
fn help_shows_flag_and_arg_names() {
    let e = Env::empty();
    let help = |args: &[&str]| e.tm(args).ok().stdout.clone();
    let h = help(&["list", "--help"]);
    assert!(h.contains("--tag <TAG>"), "{h}");
    assert!(h.contains("--due-before <DATE>"), "{h}");
    let h = help(&["week", "--help"]);
    assert!(h.contains("[WEEK]"), "{h}");
    let h = help(&["take", "--help"]);
    assert!(h.contains("<REF>..."), "{h}");
    assert!(h.contains("--week <WEEK>"), "{h}");
}

fn with_line(base: &str, line: &str) -> String {
    format!("{base}{line}\n")
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

fn assert_fixture_unchanged(e: &Env) {
    assert_eq!(e.read("tasks.md"), FIXTURE);
}

// Every line except the given 1-based ones must be byte-identical.
fn assert_only_lines_changed(before: &str, after: &str, changed: &[usize]) {
    let b: Vec<&str> = before.split_inclusive('\n').collect();
    let a: Vec<&str> = after.split_inclusive('\n').collect();
    assert_eq!(b.len(), a.len(), "line count changed:\n{after}");
    for (i, (x, y)) in b.iter().zip(&a).enumerate() {
        if !changed.contains(&(i + 1)) {
            assert_eq!(x, y, "line {} changed", i + 1);
        }
    }
}

// add
#[test]
fn add_appends_with_created() {
    let e = Env::fixture();
    let o = e.tm(&["add", "i sleep better #health/sleep"]);
    o.ok();
    let line = "- [ ] i sleep better #health/sleep @created(2026-10-18)";
    assert_eq!(o.stdout, format!("12: {line}\n"));
    assert_eq!(e.read("tasks.md"), with_line(FIXTURE, line));
}

#[test]
fn add_creates_missing_tasks_file() {
    let e = Env::empty();
    let o = e.tm(&["add", "first"]);
    o.ok();
    assert_eq!(o.stdout, "1: - [ ] first @created(2026-10-18)\n");
    assert_eq!(e.read("tasks.md"), "- [ ] first @created(2026-10-18)\n");
    assert!(!e.exists(".tasks.md.tm-tmp"));
}

#[test]
fn add_unquoted_multiword_is_usage_error() {
    let e = Env::fixture();
    let o = e.tm(&["add", "buy", "milk", "#home"]);
    assert_eq!(o.code, 2, "stderr: {}", o.stderr);
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
fn add_with_week_token() {
    let e = Env::fixture();
    let o = e.tm(&["add", "plan it @est(1h) @week(2026-w43)"]);
    o.ok();
    let line = "- [ ] plan it @est(1h) @week(2026-w43) @created(2026-10-18)";
    assert_eq!(o.stdout, format!("12: {line}\n"));
    assert_eq!(e.read("tasks.md"), with_line(FIXTURE, line));
}

#[test]
fn add_invalid_week_writes_nothing() {
    let e = Env::fixture();
    e.tm(&["add", "x @week(2026-w54)"])
        .fail("invalid @week value: 2026-w54");
    assert_fixture_unchanged(&e);
}

#[test]
fn add_rejects_checkbox_text() {
    let e = Env::fixture();
    e.tm(&["add", "- [ ] foo"])
        .fail("tm: give the text without the checkbox");
    assert_fixture_unchanged(&e);
}

#[test]
fn add_without_text_is_usage_error() {
    let e = Env::fixture();
    let o = e.tm(&["add"]);
    assert_eq!(o.code, 2, "stderr: {}", o.stderr);
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

#[test]
fn other_files_in_tm_dir_are_ignored() {
    let e = Env::fixture();
    e.write("backlog.md", "- [ ] stray @project(x)\n");
    e.write("2026/w41.md", "- [ ] old @project(y)\n");
    e.tm(&["lint"]).ok();
    e.tm(&["add", "z"]).ok();
    assert_eq!(e.read("backlog.md"), "- [ ] stray @project(x)\n");
    assert_eq!(e.read("2026/w41.md"), "- [ ] old @project(y)\n");
    let o = e.tm(&["list", "stray"]);
    o.ok();
    assert_eq!(o.stdout, "");
}

// done
#[test]
fn done_by_substring() {
    let e = Env::fixture();
    let o = e.tm(&["done", "blog"]);
    o.ok();
    let line = format!("{} @done(2026-10-18)", BLOG.replace("- [ ]", "- [x]"));
    assert_eq!(o.stdout, format!("8: {line}\n"));
    assert_eq!(
        line,
        "- [x] write a blog post @est(2h) #fun @drop(2026-w41) @week(2026-w42) @done(2026-10-18)"
    );
    let after = e.read("tasks.md");
    assert_eq!(after, FIXTURE.replace(BLOG, &line));
    assert_only_lines_changed(FIXTURE, &after, &[8]);
}

#[test]
fn done_ambiguous() {
    let e = Env::fixture();
    let o = e.tm(&["done", "wireguard"]);
    o.fail("ambiguous \"wireguard\", 2 matches:");
    assert!(o.stderr.contains("\n2: - [ ] wireguard"), "{}", o.stderr);
    assert!(o.stderr.contains(&format!("\n3: {KUBE}")), "{}", o.stderr);
    assert_fixture_unchanged(&e);
}

#[test]
fn done_no_match() {
    let e = Env::fixture();
    let o = e.tm(&["done", "nonexistent thing"]);
    o.fail("tm: no open task matches \"nonexistent thing\"");
    assert!(!o.stderr.contains(" in "), "{}", o.stderr);
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
fn done_leaves_week_token_alone() {
    let e = Env::fixture();
    e.tm(&["done", "10"]).ok();
    assert!(e
        .read("tasks.md")
        .contains("#organization @week(2026-w42) @done(2026-10-18)\n"));
}

#[test]
fn done_already_done() {
    let e = Env::fixture();
    e.tm(&["done", "5"]).fail("tm: already done: line 5");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_not_a_task() {
    let e = Env::fixture();
    e.tm(&["done", "11"]).fail("tm: not a task: line 11");
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
fn take_last_line_without_trailing_newline() {
    let e = Env::empty();
    e.write("tasks.md", "- [ ] first @est(1h)\n- [ ] last @est(1h)");
    e.tm(&["take", "last"]).ok();
    assert_eq!(
        e.read("tasks.md"),
        "- [ ] first @est(1h)\n- [ ] last @est(1h) @week(2026-w43)"
    );
}

#[test]
fn close_last_line_without_trailing_newline() {
    let e = Env::empty();
    e.write(
        "tasks.md",
        "- [x] first @week(2026-w42)\n- [ ] last @week(2026-w42)",
    );
    e.tm(&["close"]).ok();
    assert_eq!(
        e.read("tasks.md"),
        "- [x] first @week(2026-w42)\n- [ ] last @drop(2026-w42)"
    );
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
    assert_eq!(lines.len(), 2, "{}", o.stdout);
    assert!(
        lines[0].starts_with("8: - [x] write a blog post"),
        "{}",
        o.stdout
    );
    assert_eq!(
        lines[1],
        "2: - [x] wireguard container forwards to mullvad @est(2h) #self-hosting @done(2026-10-18)"
    );
    assert_only_lines_changed(FIXTURE, &e.read("tasks.md"), &[2, 8]);
}

#[test]
fn done_is_all_or_nothing() {
    let e = Env::fixture();
    e.tm(&["done", "blog", "nonexistent thing"])
        .fail("no open task matches");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_same_task_twice_is_error() {
    let e = Env::fixture();
    let o = e.tm(&["done", "blog", "8"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: same task given twice: 8\n");
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
fn empty_reference_is_error() {
    let e = Env::fixture();
    e.tm(&["done", ""]).fail("tm: empty reference");
    e.tm(&["take", " "]).fail("tm: empty reference");
    assert_fixture_unchanged(&e);
}

// take
#[test]
fn take_defaults_to_next_week_on_sunday() {
    let e = Env::fixture();
    let o = e.tm(&["take", "kubernetes"]);
    o.ok();
    assert_eq!(
        o.stdout,
        format!("3: {KUBE} @week(2026-w43)\n2026-w43: planned 4h, past average done 5h\n")
    );
    let after = e.read("tasks.md");
    assert_eq!(
        after,
        FIXTURE.replace(KUBE, &format!("{KUBE} @week(2026-w43)"))
    );
    assert_only_lines_changed(FIXTURE, &after, &[3]);
}

#[test]
fn take_targets_current_week_before_sunday() {
    let e = Env::fixture();
    let o = e.tm_on("2026-10-15", &["take", "kubernetes"]);
    o.ok();
    assert_eq!(
        o.stdout,
        format!("3: {KUBE} @week(2026-w42)\n2026-w42: planned 12h, past average done 5h\n")
    );
    assert_only_lines_changed(FIXTURE, &e.read("tasks.md"), &[3]);
}

#[test]
fn take_refuses_big_tasks() {
    for args in [
        &["take", "sunshine"][..],
        &["take", "kubernetes", "sunshine"][..],
    ] {
        let e = Env::fixture();
        let o = e.tm(args);
        o.fail("tm: too big, split it first: line 4 @est(3d)");
        assert_fixture_unchanged(&e);
    }
}

#[test]
fn take_with_explicit_week() {
    let e = Env::fixture();
    let o = e.tm(&["take", "--week", "current", "mullvad"]);
    o.ok();
    let line =
        "- [ ] wireguard container forwards to mullvad @est(2h) #self-hosting @week(2026-w42)";
    assert_eq!(
        o.stdout,
        format!("2: {line}\n2026-w42: planned 10h, past average done 5h\n")
    );
    assert_only_lines_changed(FIXTURE, &e.read("tasks.md"), &[2]);

    let e = Env::fixture();
    let o = e.tm(&["take", "--week", "2026-w44", "mullvad"]);
    o.ok();
    assert!(o.stdout.contains("@week(2026-w44)"), "{}", o.stdout);
    assert!(
        o.stdout
            .ends_with("2026-w44: planned 2h, past average done 5h\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn take_invalid_week_is_checked_before_refs() {
    let e = Env::fixture();
    e.tm(&["take", "--week", "bogus", "nonexistent thing"])
        .fail("tm: invalid week: bogus");
    assert_fixture_unchanged(&e);
}

#[test]
fn take_multiple_in_given_order() {
    let e = Env::fixture();
    let o = e.tm(&["take", "mullvad", "kubernetes"]);
    o.ok();
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "2: - [ ] wireguard container forwards to mullvad @est(2h) #self-hosting @week(2026-w43)",
            &format!("3: {KUBE} @week(2026-w43)"),
            "2026-w43: planned 6h, past average done 5h",
        ]
    );
    assert_only_lines_changed(FIXTURE, &e.read("tasks.md"), &[2, 3]);
}

#[test]
fn take_past_average_is_na_without_past_weeks() {
    let e = Env::empty();
    e.write("tasks.md", "- [ ] a @est(1h)\n");
    let o = e.tm(&["take", "a"]);
    o.ok();
    assert_eq!(
        o.stdout,
        "1: - [ ] a @est(1h) @week(2026-w43)\n2026-w43: planned 1h, past average done n/a\n"
    );
}

#[test]
fn take_replans_task_from_another_week() {
    let e = Env::fixture();
    let o = e.tm(&["take", "--week", "next", "blog"]);
    o.ok();
    let line = "- [ ] write a blog post @est(2h) #fun @drop(2026-w41) @week(2026-w43)";
    assert_eq!(
        o.stdout,
        format!("8: {line}\n2026-w43: planned 2h, past average done 5h\n")
    );
    let after = e.read("tasks.md");
    assert_eq!(after, FIXTURE.replace(BLOG, line));
    assert_only_lines_changed(FIXTURE, &after, &[8]);
}

#[test]
fn take_into_same_week_leaves_line_unchanged_but_prints_it() {
    let e = Env::fixture();
    let o = e.tm(&["take", "--week", "current", "wardrobe"]);
    o.ok();
    assert_eq!(
        o.stdout,
        format!("10: {WARDROBE}\n2026-w42: planned 8h, past average done 5h\n")
    );
    assert_fixture_unchanged(&e);
}

#[test]
fn take_refuses_done_task() {
    let e = Env::fixture();
    e.tm(&["take", "5"]).fail("tm: already done: line 5");
    e.tm(&["take", "read ML"])
        .fail("tm: no open task matches \"read ML\"");
    assert_fixture_unchanged(&e);
}

#[test]
fn take_same_task_twice_is_error() {
    let e = Env::fixture();
    let o = e.tm(&["take", "mullvad", "2"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert!(
        o.stderr.contains("same task given twice: 2"),
        "{}",
        o.stderr
    );
    assert_fixture_unchanged(&e);
}

#[test]
fn take_partial_failure_writes_nothing() {
    let e = Env::fixture();
    let o = e.tm(&["take", "kubernetes", "nope"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_fixture_unchanged(&e);
}

#[test]
fn take_size_boundaries() {
    for est in ["1d", "40h"] {
        let e = Env::empty();
        e.write("tasks.md", &format!("- [ ] t @est({est})\n"));
        e.tm(&["take", "t"]).ok();
        assert_eq!(
            e.read("tasks.md"),
            format!("- [ ] t @est({est}) @week(2026-w43)\n"),
            "{est}"
        );
    }
    for est in ["1.5d", "2m", "1y"] {
        let e = Env::empty();
        let content = format!("- [ ] t @est({est})\n");
        e.write("tasks.md", &content);
        e.tm(&["take", "t"]).fail("too big, split it first");
        assert_eq!(e.read("tasks.md"), content, "{est}");
    }
}

// close
#[test]
fn close_returns_open_tasks_with_drop() {
    let e = Env::fixture();
    let o = e.tm(&["close"]);
    o.ok();
    let l8 = "- [ ] write a blog post @est(2h) #fun @drop(2026-w42)";
    let l10 = "- [ ] stachiewicza wardrobe  @est(4h) @drop(2026-w42) #organization";
    assert_eq!(o.stdout, format!("8: {l8}\n10: {l10}\n"));
    let after = e.read("tasks.md");
    assert_eq!(after, FIXTURE.replace(BLOG, l8).replace(WARDROBE, l10));
    assert_only_lines_changed(FIXTURE, &after, &[8, 10]);
    // the done task keeps its @week
    assert!(after
        .contains("- [x] read ML slides @est(2h) @due(2026-10-14) #university @week(2026-w42)\n"));
}

#[test]
fn close_then_nothing_to_close() {
    let e = Env::fixture();
    e.tm(&["close"]).ok();
    let closed = e.read("tasks.md");
    let o = e.tm(&["close"]);
    o.ok();
    assert_eq!(o.stdout, "nothing to close in 2026-w42\n");
    assert_eq!(e.read("tasks.md"), closed);
}

#[test]
fn close_appends_drop_when_task_has_none() {
    let e = Env::fixture();
    let before = with_line(FIXTURE, "- [ ] no drop yet @est(1h) #t @week(2026-w42)");
    e.write("tasks.md", &before);
    let o = e.tm(&["close"]);
    o.ok();
    assert!(
        o.stdout
            .ends_with("12: - [ ] no drop yet @est(1h) #t @drop(2026-w42)\n"),
        "{}",
        o.stdout
    );
    assert_eq!(
        e.read("tasks.md").lines().nth(11),
        Some("- [ ] no drop yet @est(1h) #t @drop(2026-w42)")
    );
    assert_only_lines_changed(&before, &e.read("tasks.md"), &[8, 10, 12]);
}

#[test]
fn close_defaults_to_previous_week_midweek() {
    let e = Env::fixture();
    let before = with_line(FIXTURE, "- [ ] late @est(1h) @week(2026-w41)");
    e.write("tasks.md", &before);
    let o = e.tm_on("2026-10-15", &["close"]);
    o.ok();
    assert_eq!(o.stdout, "12: - [ ] late @est(1h) @drop(2026-w41)\n");
    assert_only_lines_changed(&before, &e.read("tasks.md"), &[12]);
}

#[test]
fn close_unfinished_week_fails() {
    let e = Env::fixture();
    e.tm_on("2026-10-15", &["close", "2026-w42"])
        .fail("tm: week 2026-w42 is not finished (ends 2026-10-18)");
    assert_fixture_unchanged(&e);
}

#[test]
fn close_nothing_to_close() {
    let e = Env::fixture();
    let o = e.tm_on("2026-10-15", &["close"]);
    o.ok();
    assert_eq!(o.stdout, "nothing to close in 2026-w41\n");
    assert_fixture_unchanged(&e);
}

#[test]
fn close_skips_invalid_open_task() {
    let e = Env::fixture();
    let before = with_line(FIXTURE, "- [ ] y @project(b) @week(2026-w42)");
    e.write("tasks.md", &before);
    let o = e.tm(&["close"]);
    o.ok();
    assert_eq!(o.stderr, "tm: skipping line 12: unknown token @project\n");
    assert_only_lines_changed(&before, &e.read("tasks.md"), &[8, 10]);
}

// week
#[test]
fn week_json_current() {
    let e = Env::fixture();
    let o = e.tm(&["week", "--json"]);
    o.ok();
    let v = o.json();
    assert_eq!(v["week"], "2026-w42");
    assert_eq!(v["start"], "2026-10-12");
    assert_eq!(v["end"], "2026-10-18");
    assert_eq!(v["status"], "current");
    assert!(v.get("file").is_none());
    assert!(v.get("exists").is_none());
    assert_eq!(f(&v["tasks"]), 3.0);
    assert_eq!(f(&v["done"]), 1.0);
    assert_eq!(f(&v["open"]), 2.0);
    assert_eq!(f(&v["planned_hours"]), 8.0);
    assert_eq!(f(&v["done_hours"]), 2.0);
    assert_eq!(f(&v["open_hours"]), 6.0);
    assert_eq!(v["other_estimates"], json!([]));
    assert_eq!(f(&v["no_estimate"]), 0.0);
    assert_eq!(v["past_weeks"], json!(["2026-w41"]));
    assert_eq!(f(&v["past_avg_done_hours"]), 5.0);
}

#[test]
fn week_text_first_line_has_no_file() {
    let e = Env::fixture();
    let o = e.tm(&["week"]);
    o.ok();
    assert_eq!(
        o.stdout.lines().next(),
        Some("2026-w42 (2026-10-12..2026-10-18, current)"),
        "{}",
        o.stdout
    );
}

#[test]
fn week_other_estimates_and_no_estimate() {
    let e = Env::empty();
    e.write(
        "tasks.md",
        "- [ ] a @est(3d) @week(2026-w42)\n- [ ] b @week(2026-w42)\n- [ ] c @est(1h) @week(2026-w42)\n",
    );
    let v = e.tm(&["week", "--json"]).ok().json();
    assert_eq!(v["other_estimates"], json!(["3d"]));
    assert_eq!(f(&v["no_estimate"]), 1.0);
    assert_eq!(f(&v["planned_hours"]), 1.0);
    assert!(v["past_avg_done_hours"].is_null());
    assert_eq!(v["past_weeks"], json!([]));
}

#[test]
fn week_past_and_next() {
    let e = Env::fixture();
    let v = e.tm(&["week", "2026-w41", "--json"]).ok().json();
    assert_eq!(v["week"], "2026-w41");
    assert_eq!(v["status"], "past");

    let v = e.tm(&["week", "next", "--json"]).ok().json();
    assert_eq!(v["week"], "2026-w43");
    assert_eq!(f(&v["tasks"]), 0.0);
    assert_eq!(f(&v["planned_hours"]), 0.0);
    assert_eq!(v["status"], "future");
}

#[test]
fn week_iso_year_edges() {
    let e = Env::fixture();
    let v = e.tm_on("2027-01-01", &["week", "--json"]).ok().json();
    assert_eq!(v["week"], "2026-w53");
    assert_eq!(v["start"], "2026-12-28");
    assert_eq!(v["end"], "2027-01-03");
    assert!(v.get("file").is_none());

    let v = e.tm_on("2025-12-30", &["week", "--json"]).ok().json();
    assert_eq!(v["week"], "2026-w01");
}

#[test]
fn week_past_average_after_close() {
    let e = Env::fixture();
    e.tm_on("2026-10-19", &["close"]).ok();
    let v = e.tm_on("2026-10-19", &["week", "--json"]).ok().json();
    assert_eq!(v["week"], "2026-w43");
    assert_eq!(v["past_weeks"], json!(["2026-w41", "2026-w42"]));
    assert_eq!(f(&v["past_avg_done_hours"]), 3.5);
}

#[test]
fn week_past_weeks_include_drop_only_weeks() {
    let e = Env::empty();
    e.write(
        "tasks.md",
        "- [ ] a @est(1h) @drop(2026-w40)\n- [x] b @est(4h) @week(2026-w41)\n- [x] c @est(1h) @week(2026-w41)\n",
    );
    let v = e.tm(&["week", "--json"]).ok().json();
    assert_eq!(v["past_weeks"], json!(["2026-w40", "2026-w41"]));
    assert_eq!(f(&v["past_avg_done_hours"]), 2.5);
}

#[test]
fn week_only_last_four_past_weeks_count() {
    let e = Env::empty();
    e.write(
        "tasks.md",
        "- [x] a @est(1h) @week(2026-w36)\n- [x] b @est(2h) @week(2026-w37)\n- [x] c @est(3h) @week(2026-w38)\n- [x] d @est(4h) @week(2026-w39)\n- [x] e @est(5h) @week(2026-w40)\n",
    );
    let v = e.tm(&["week", "--json"]).ok().json();
    assert_eq!(
        v["past_weeks"],
        json!(["2026-w37", "2026-w38", "2026-w39", "2026-w40"])
    );
    assert_eq!(f(&v["past_avg_done_hours"]), 3.5);
}

#[test]
fn week_warns_once_per_invalid_line() {
    let e = Env::fixture();
    e.write("tasks.md", &with_line(FIXTURE, "- [ ] x @project(foo)"));
    let skip = "tm: skipping line 12: unknown token @project\n";
    let o = e.tm(&["week", "--json"]);
    o.ok();
    let v = o.json();
    assert_eq!(v["past_weeks"], json!(["2026-w41"]));
    assert_eq!(f(&v["past_avg_done_hours"]), 5.0);
    assert_eq!(o.stderr, skip);

    let o = e.tm(&["week", "2026-w42", "--json"]);
    o.ok();
    assert_eq!(f(&o.json()["tasks"]), 3.0);
    assert_eq!(o.stderr, skip);
}

// list
#[test]
fn list_json_backlog_open() {
    let e = Env::fixture();
    let v = e
        .tm(&["list", "--in", "backlog", "--open", "--json"])
        .ok()
        .json();
    let items = v.as_array().expect("array");
    let lines: Vec<f64> = items.iter().map(|i| f(&i["line"])).collect();
    assert_eq!(lines, [2.0, 3.0, 4.0]);
    let it = &items[1];
    assert!(it.get("file").is_none());
    assert_eq!(it["done"], false);
    assert_eq!(it["description"], "kubernetes cluster runs wireguard");
    assert_eq!(it["tags"], json!(["self-hosting"]));
    assert_eq!(it["est"], "4h");
    assert_eq!(it["due"], "2026-10-25");
    for k in ["created", "done_on", "drop", "week", "proj", "step"] {
        assert_eq!(it.get(k), Some(&Value::Null), "{k}");
    }
    assert!(it.get("tokens").is_none());
    assert_eq!(f(&it["est_hours"]), 4.0);
    assert_eq!(it["text"], KUBE);
    assert!(items[2]["est_hours"].is_null());

    let mut keys: Vec<&str> = it.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    keys.sort();
    let mut want = vec![
        "line",
        "done",
        "description",
        "tags",
        "est",
        "est_hours",
        "due",
        "created",
        "done_on",
        "drop",
        "week",
        "proj",
        "step",
        "text",
    ];
    want.sort();
    assert_eq!(keys, want);
}

#[test]
fn list_json_week_key() {
    let e = Env::fixture();
    let v = e.tm(&["list", "--in", "2026-w42", "--json"]).ok().json();
    let items = v.as_array().expect("array");
    let lines: Vec<f64> = items.iter().map(|i| f(&i["line"])).collect();
    assert_eq!(lines, [8.0, 9.0, 10.0]);
    assert_eq!(items[0]["week"], "2026-w42");
    assert_eq!(items[0]["drop"], "2026-w41");
    assert_eq!(items[0]["text"], BLOG);
    assert_eq!(items[0]["tags"], json!(["fun"]));
    assert_eq!(items[1]["done"], true);
    assert_eq!(items[1]["due"], "2026-10-14");
    assert_eq!(items[2]["tags"], json!(["organization"]));
    assert_eq!(f(&items[2]["est_hours"]), 4.0);
    for k in ["created", "done_on", "proj", "step"] {
        assert_eq!(items[2].get(k), Some(&Value::Null), "{k}");
    }
}

#[test]
fn list_in_scopes() {
    let e = Env::fixture();
    let lines = |args: &[&str]| -> Vec<String> {
        let v = e.tm(args).ok().json();
        v.as_array()
            .unwrap()
            .iter()
            .map(|i| f(&i["line"]).to_string())
            .collect()
    };
    assert_eq!(
        lines(&["list", "--in", "backlog", "--json"]),
        ["2", "3", "4", "5"]
    );
    assert_eq!(
        lines(&["list", "--in", "2026-w41", "--done", "--json"]),
        ["6", "7"]
    );
    assert_eq!(
        lines(&["list", "--in", "current", "--json"]),
        ["8", "9", "10"]
    );
    assert_eq!(
        lines(&["list", "--in", "next", "--json"]),
        Vec::<String>::new()
    );
    assert_eq!(lines(&["list", "--json"]).len(), 9);
}

#[test]
fn list_in_invalid_is_error() {
    let e = Env::fixture();
    e.tm(&["list", "--in", "raw"]).fail("tm: invalid week: raw");
}

#[test]
fn list_tag_filters() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--tag", "self"]);
    o.ok();
    assert_eq!(o.stdout, "");
    let o = e.tm(&["list", "--tag", "self-hosting"]);
    o.ok();
    let nums: Vec<&str> = o
        .stdout
        .lines()
        .map(|l| l.split(':').next().unwrap())
        .collect();
    assert_eq!(nums, ["2", "3", "4"], "{}", o.stdout);
}

#[test]
fn list_tag_matches_subtags_and_requires_all() {
    let e = Env::empty();
    e.write(
        "tasks.md",
        "- [ ] a #health/sleep\n- [ ] b #health #fun\n- [ ] c #healthy\n",
    );
    let o = e.tm(&["list", "--tag", "health"]);
    o.ok();
    assert_eq!(
        o.stdout,
        "1: - [ ] a #health/sleep\n2: - [ ] b #health #fun\n"
    );
    let o = e.tm(&["list", "--tag", "health", "--tag", "fun"]);
    o.ok();
    assert_eq!(o.stdout, "2: - [ ] b #health #fun\n");
}

#[test]
fn list_due_before() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--due-before", "2026-10-20"]);
    o.ok();
    assert_eq!(
        o.stdout,
        "9: - [x] read ML slides @est(2h) @due(2026-10-14) #university @week(2026-w42)\n"
    );
}

#[test]
fn list_default_order_and_text_format() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--open"]);
    o.ok();
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "2: - [ ] wireguard container forwards to mullvad @est(2h) #self-hosting",
            &format!("3: {KUBE}"),
            "4: - [ ] sunshine on headless vm @est(3d) #self-hosting",
            &format!("8: {BLOG}"),
            &format!("10: {WARDROBE}"),
        ]
    );
    let all = e.tm(&["list"]).ok().stdout.clone();
    let nums: Vec<&str> = all.lines().map(|l| l.split(':').next().unwrap()).collect();
    assert_eq!(nums, ["2", "3", "4", "5", "6", "7", "8", "9", "10"]);
}

#[test]
fn list_query_in_done_proj_and_empty_json() {
    let e = Env::fixture();
    let o = e.tm(&["list", "BLOG"]);
    o.ok();
    assert_eq!(o.stdout, format!("8: {BLOG}\n"));

    let o = e.tm(&["list", "--in", "backlog", "--open"]);
    o.ok();
    assert_eq!(o.stdout.lines().count(), 3, "{}", o.stdout);

    e.write("tasks.md", "- [ ] a @proj(x)\n- [ ] b @proj(xy)\n");
    let o = e.tm(&["list", "--proj", "x"]);
    o.ok();
    assert_eq!(o.stdout, "1: - [ ] a @proj(x)\n");

    let o = e.tm(&["list", "zzzz", "--json"]);
    o.ok();
    assert_eq!(o.json(), json!([]));
    let o = e.tm(&["list", "zzzz"]);
    o.ok();
    assert_eq!(o.stdout, "");
}

#[test]
fn list_json_typed_shape() {
    let e = Env::empty();
    e.write(
        "tasks.md",
        "- [x] a @est(0.5h) @created(2026-10-01) @done(2026-10-18) @proj(p q) @step(2) @week(2026-w42) #t\n",
    );
    let v = e.tm(&["list", "--json"]).ok().json();
    let it = &v.as_array().expect("array")[0];
    assert_eq!(it["est"], "0.5h");
    assert_eq!(f(&it["est_hours"]), 0.5);
    assert_eq!(it["created"], "2026-10-01");
    assert_eq!(it["done_on"], "2026-10-18");
    assert_eq!(it["proj"], "p q");
    assert_eq!(it["step"], json!(2));
    assert_eq!(it["week"], "2026-w42");
    assert_eq!(it["tags"], json!(["t"]));
}

#[test]
fn list_skips_invalid_line_with_warning() {
    let e = Env::fixture();
    e.write("tasks.md", &with_line(FIXTURE, "- [ ] x @project(foo)"));
    let o = e.tm(&["list"]);
    o.ok();
    assert!(!o.stdout.contains("12:"), "{}", o.stdout);
    assert_eq!(o.stderr, "tm: skipping line 12: unknown token @project\n");
}

#[test]
fn list_json_closed_pipe_does_not_panic() {
    let e = Env::empty();
    let mut b = String::new();
    for i in 0..20000 {
        b.push_str(&format!("- [ ] task number {i} @est(1h) #tag\n"));
    }
    e.write("tasks.md", &b);
    let mut child = Command::new(env!("CARGO_BIN_EXE_tm"))
        .args(["list", "--json"])
        .env("TM_DIR", &e.dir)
        .env("TM_TODAY", "2026-10-18")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let o = child.wait_with_output().unwrap();
    assert_ne!(o.status.code(), Some(101));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!err.contains("panicked"), "{err}");
}

// lint
#[test]
fn lint_clean_then_unknown_token() {
    let e = Env::fixture();
    let o = e.tm(&["lint"]);
    o.ok();
    assert_eq!(o.stdout, "");

    e.write("tasks.md", &with_line(FIXTURE, "- [ ] x @project(foo)"));
    let o = e.tm(&["lint"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stdout, "12: unknown token @project\n");
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
    assert_eq!(o.stdout, "12: unknown token @project\n");
}

#[test]
fn lint_reports_invalid_week() {
    let e = Env::fixture();
    e.write("tasks.md", &with_line(FIXTURE, "- [ ] x @week(2026-w54)"));
    let o = e.tm(&["lint"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stdout, "12: invalid @week value: 2026-w54\n");
}

// refs to invalid lines, warnings
#[test]
fn done_ref_to_invalid_line_is_hard_error() {
    let e = Env::fixture();
    let content = with_line(FIXTURE, "- [ ] x @project(foo)");
    e.write("tasks.md", &content);
    let o = e.tm(&["done", "12"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stderr, "tm: line 12: unknown token @project\n");
    assert_eq!(e.read("tasks.md"), content);
}

#[test]
fn take_skips_invalid_line() {
    let e = Env::fixture();
    let content = with_line(FIXTURE, "- [ ] fix gutter @project(a)");
    e.write("tasks.md", &content);
    let skip = "tm: skipping line 12: unknown token @project\n";

    let o = e.tm(&["take", "gutter"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(
        o.stderr,
        format!("{skip}tm: no open task matches \"gutter\"\n")
    );
    assert_eq!(e.read("tasks.md"), content);

    let o = e.tm(&["take", "kubernetes"]);
    o.ok();
    assert!(o.stderr.contains(skip), "{}", o.stderr);
    assert_only_lines_changed(&content, &e.read("tasks.md"), &[3]);
}

// misc
#[test]
fn missing_tasks_dir() {
    let e = Env::empty();
    let o = e.run_in(Path::new("/nonexistent"), "2026-10-18", &["lint"]);
    o.fail("tm: tasks dir not found: /nonexistent");
}

#[test]
fn invalid_week() {
    let e = Env::fixture();
    e.tm(&["week", "bogus"]).fail("tm: invalid week: bogus");
}

#[test]
fn invalid_today_is_error() {
    let e = Env::fixture();
    let o = e.tm_on("not-a-date", &["lint"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
}

#[test]
fn json_flag_before_subcommand() {
    let e = Env::fixture();
    let before = e.tm(&["--json", "list"]).ok().json();
    let after = e.tm(&["list", "--json"]).ok().json();
    assert_eq!(before, after);
    assert!(before.is_array());
    let w = e.tm(&["--json", "week"]).ok().json();
    assert!(w.get("past_weeks").is_some(), "{w}");
}
