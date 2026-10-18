use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const BACKLOG: &str = "# Backlog
- [ ] wireguard container forwards to mullvad @est(2h) #self-hosting
- [ ] kubernetes cluster runs wireguard @est(4h) @due(2026-10-25) #self-hosting
- [ ] sunshine on headless vm @est(3d) #self-hosting
- [x] old done thing @est(1h)
";

const W42: &str = "---
week: 2026-w42
---
- [ ] write a blog post @est(2h) #fun
- [x] read ML slides @est(2h) @due(2026-10-14) #university
- [ ] stachiewicza wardrobe  @est(4h) @drop(2026-w41) #organization
notes here
";

const W41: &str = "- [x] migrate secrets @est(4h)
- [x] clean up longhorn @est(1h)
- [ ] write a blog post @est(2h) #fun
";

const KUBE: &str =
    "- [ ] kubernetes cluster runs wireguard @est(4h) @due(2026-10-25) #self-hosting";

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
        e.write("backlog.md", BACKLOG);
        e.write("2026/w42.md", W42);
        e.write("2026/w41.md", W41);
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

fn with_line(base: &str, line: &str) -> String {
    format!("{base}{line}\n")
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

fn assert_fixture_unchanged(e: &Env) {
    assert_eq!(e.read("backlog.md"), BACKLOG);
    assert_eq!(e.read("2026/w42.md"), W42);
    assert_eq!(e.read("2026/w41.md"), W41);
}

// 1
#[test]
fn add_appends_with_created() {
    let e = Env::fixture();
    let o = e.tm(&["add", "i sleep better #health/sleep"]);
    o.ok();
    assert_eq!(o.stdout.trim_end(), "backlog.md:6");
    assert_eq!(
        e.read("backlog.md"),
        with_line(
            BACKLOG,
            "- [ ] i sleep better #health/sleep @created(2026-10-18)"
        )
    );
}

#[test]
fn add_joins_args_with_spaces() {
    let e = Env::fixture();
    e.tm(&["add", "buy", "milk", "#home"]).ok();
    assert_eq!(
        e.read("backlog.md"),
        with_line(BACKLOG, "- [ ] buy milk #home @created(2026-10-18)")
    );
}

// 2
#[test]
fn add_lint_failure_writes_nothing() {
    let e = Env::fixture();
    let o = e.tm(&["add", "x @est(2x)"]);
    o.fail("invalid @est value: 2x");
    assert_eq!(e.read("backlog.md"), BACKLOG);
}

// 3
#[test]
fn add_keeps_existing_created() {
    let e = Env::fixture();
    e.tm(&["add", "x @created(2026-01-01)"]).ok();
    assert_eq!(
        e.read("backlog.md"),
        with_line(BACKLOG, "- [ ] x @created(2026-01-01)")
    );
}

#[test]
fn add_rejects_checkbox_text() {
    let e = Env::fixture();
    e.tm(&["add", "- [ ] foo"])
        .fail("tm: give the text without the checkbox");
    assert_eq!(e.read("backlog.md"), BACKLOG);
}

#[test]
fn add_without_text_is_usage_error() {
    let e = Env::fixture();
    let o = e.tm(&["add"]);
    assert_eq!(o.code, 2, "stderr: {}", o.stderr);
    assert_eq!(e.read("backlog.md"), BACKLOG);
}

#[test]
fn add_adds_newline_to_unterminated_file() {
    let e = Env::empty();
    e.write("backlog.md", "# Backlog\n- [ ] a");
    let o = e.tm(&["add", "b"]);
    o.ok();
    assert_eq!(o.stdout.trim_end(), "backlog.md:3");
    assert_eq!(
        e.read("backlog.md"),
        "# Backlog\n- [ ] a\n- [ ] b @created(2026-10-18)\n"
    );
}

// 4
#[test]
fn raw_then_promote() {
    let e = Env::fixture();
    let o = e.tm(&["raw", "call the dentist"]);
    o.ok();
    assert_eq!(o.stdout.trim_end(), "raw.md:1");
    assert_eq!(e.read("raw.md"), "- [ ] call the dentist\n");

    let o = e.tm(&["promote", "dentist", "dentist booked @est(0.5h) #health"]);
    o.ok();
    assert_eq!(o.stdout.trim_end(), "backlog.md:6");
    assert_eq!(e.read("raw.md"), "");
    assert_eq!(
        e.read("backlog.md"),
        with_line(
            BACKLOG,
            "- [ ] dentist booked @est(0.5h) #health @created(2026-10-18)"
        )
    );
}

#[test]
fn promote_without_text_keeps_original_line() {
    let e = Env::fixture();
    e.write("raw.md", "- [ ] first\n- [ ] call the dentist #health\n");
    let o = e.tm(&["promote", "dentist"]);
    o.ok();
    assert_eq!(o.stdout.trim_end(), "backlog.md:6");
    assert_eq!(e.read("raw.md"), "- [ ] first\n");
    assert_eq!(
        e.read("backlog.md"),
        with_line(
            BACKLOG,
            "- [ ] call the dentist #health @created(2026-10-18)"
        )
    );
}

#[test]
fn promote_outside_raw_is_rejected() {
    let e = Env::fixture();
    e.write("raw.md", "- [ ] something\n");
    let o = e.tm(&["promote", "backlog.md:2"]);
    o.fail("tm: backlog.md:2 is not in raw.md");
    assert_eq!(e.read("backlog.md"), BACKLOG);
    assert_eq!(e.read("raw.md"), "- [ ] something\n");
}

#[test]
fn raw_does_not_lint_or_add_created() {
    let e = Env::empty();
    e.write("raw.md", "- [ ] a\n");
    let o = e.tm(&["raw", "x @est(2x)"]);
    o.ok();
    assert_eq!(o.stdout.trim_end(), "raw.md:2");
    assert_eq!(e.read("raw.md"), "- [ ] a\n- [ ] x @est(2x)\n");
}

// 5
#[test]
fn done_by_substring() {
    let e = Env::fixture();
    let o = e.tm(&["done", "blog"]);
    o.ok();
    assert_eq!(o.stdout.trim_end(), "2026/w42.md:4");
    let want = W42.replace(
        "- [ ] write a blog post @est(2h) #fun\n",
        "- [x] write a blog post @est(2h) #fun @done(2026-10-18)\n",
    );
    assert_eq!(e.read("2026/w42.md"), want);
    assert_eq!(e.read("2026/w41.md"), W41);
    assert_eq!(e.read("backlog.md"), BACKLOG);
}

// 6
#[test]
fn done_ambiguous() {
    let e = Env::fixture();
    let o = e.tm(&["done", "wireguard"]);
    o.fail("ambiguous \"wireguard\", 2 matches:");
    assert!(o.stderr.contains("backlog.md:2:"), "{}", o.stderr);
    assert!(o.stderr.contains("backlog.md:3:"), "{}", o.stderr);
    assert_fixture_unchanged(&e);
}

#[test]
fn done_no_match() {
    let e = Env::fixture();
    e.tm(&["done", "nonexistent thing"])
        .fail("tm: no open task matches \"nonexistent thing\" in");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_exact_description_wins_over_substrings() {
    let e = Env::empty();
    e.write("backlog.md", "- [ ] fix\n- [ ] fix the thing\n");
    let o = e.tm(&["done", "FIX"]);
    o.ok();
    assert_eq!(o.stdout.trim_end(), "backlog.md:1");
    assert_eq!(
        e.read("backlog.md"),
        "- [x] fix @done(2026-10-18)\n- [ ] fix the thing\n"
    );
}

// 7
#[test]
fn done_by_file_line_any_task_file() {
    let e = Env::fixture();
    let o = e.tm(&["done", "2026/w41.md:3"]);
    o.ok();
    assert_eq!(o.stdout.trim_end(), "2026/w41.md:3");
    assert_eq!(
        e.read("2026/w41.md"),
        "- [x] migrate secrets @est(4h)\n- [x] clean up longhorn @est(1h)\n- [x] write a blog post @est(2h) #fun @done(2026-10-18)\n"
    );
}

#[test]
fn done_already_done() {
    let e = Env::fixture();
    e.tm(&["done", "backlog.md:5"])
        .fail("tm: already done: backlog.md:5");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_not_a_task() {
    let e = Env::fixture();
    e.tm(&["done", "2026/w42.md:2"])
        .fail("tm: not a task: 2026/w42.md:2");
    assert_fixture_unchanged(&e);
}

#[test]
fn done_multiple_refs() {
    let e = Env::fixture();
    let o = e.tm(&["done", "blog", "backlog.md:2"]);
    o.ok();
    assert_eq!(o.stdout, "2026/w42.md:4\nbacklog.md:2\n");
    assert!(e.read("backlog.md").contains(
        "- [x] wireguard container forwards to mullvad @est(2h) #self-hosting @done(2026-10-18)\n"
    ));
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
    let o = e.tm(&["done", "blog", "2026/w42.md:4"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert!(o.stderr.contains("same task given twice"), "{}", o.stderr);
    assert_fixture_unchanged(&e);
}

// 8
#[test]
fn take_creates_next_week_file_on_sunday() {
    let e = Env::fixture();
    let o = e.tm(&["take", "kubernetes"]);
    o.ok();
    assert_eq!(
        o.stdout.lines().next(),
        Some("2026/w43.md:1"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.lines().any(|l| l.starts_with("2026-w43: planned")),
        "{}",
        o.stdout
    );
    assert_eq!(e.read("2026/w43.md"), format!("{KUBE}\n"));
    assert_eq!(
        e.read("backlog.md"),
        BACKLOG.replace(&format!("{KUBE}\n"), "")
    );
    assert_eq!(e.read("backlog.md").lines().count(), 4);
    assert_eq!(e.read("2026/w42.md"), W42);
}

#[test]
fn take_targets_current_week_before_sunday() {
    let e = Env::fixture();
    let o = e.tm_on("2026-10-15", &["take", "kubernetes"]);
    o.ok();
    assert_eq!(
        o.stdout.lines().next(),
        Some("2026/w42.md:8"),
        "{}",
        o.stdout
    );
    assert_eq!(e.read("2026/w42.md"), with_line(W42, KUBE));
    assert!(!e.exists("2026/w43.md"));
}

// 9
#[test]
fn take_refuses_big_tasks() {
    for args in [
        &["take", "sunshine"][..],
        &["take", "kubernetes", "sunshine"][..],
    ] {
        let e = Env::fixture();
        let o = e.tm(args);
        o.fail("tm: too big, split it first: backlog.md:4 @est(3d)");
        assert_fixture_unchanged(&e);
        assert!(!e.exists("2026/w43.md"));
    }
}

// 10
#[test]
fn take_with_explicit_week() {
    let e = Env::fixture();
    let o = e.tm(&["take", "--week", "current", "mullvad"]);
    o.ok();
    assert_eq!(
        o.stdout.lines().next(),
        Some("2026/w42.md:8"),
        "{}",
        o.stdout
    );
    assert_eq!(
        e.read("2026/w42.md"),
        with_line(
            W42,
            "- [ ] wireguard container forwards to mullvad @est(2h) #self-hosting"
        )
    );
}

#[test]
fn take_multiple_in_given_order() {
    let e = Env::fixture();
    let o = e.tm(&["take", "mullvad", "kubernetes"]);
    o.ok();
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert_eq!(
        &lines[..2],
        &["2026/w43.md:1", "2026/w43.md:2"],
        "{}",
        o.stdout
    );
    assert_eq!(
        e.read("2026/w43.md"),
        format!("- [ ] wireguard container forwards to mullvad @est(2h) #self-hosting\n{KUBE}\n")
    );
}

// 11
#[test]
fn close_copies_open_tasks_with_drop() {
    let e = Env::fixture();
    let o = e.tm(&["close"]);
    o.ok();
    assert_eq!(o.stdout, "backlog.md:6\nbacklog.md:7\n");
    let want = format!(
        "{BACKLOG}- [ ] write a blog post @est(2h) #fun @drop(2026-w42)\n- [ ] stachiewicza wardrobe  @est(4h) @drop(2026-w42) #organization\n"
    );
    assert_eq!(e.read("backlog.md"), want);
    assert_eq!(e.read("2026/w42.md"), W42);

    let o = e.tm(&["close"]);
    o.ok();
    assert_eq!(e.read("backlog.md"), want);
    assert_eq!(e.read("2026/w42.md"), W42);
}

// 12
#[test]
fn close_defaults_to_previous_week_midweek() {
    let e = Env::fixture();
    let o = e.tm_on("2026-10-15", &["close"]);
    o.ok();
    assert_eq!(
        e.read("backlog.md"),
        with_line(
            BACKLOG,
            "- [ ] write a blog post @est(2h) #fun @drop(2026-w41)"
        )
    );
    assert_eq!(e.read("2026/w41.md"), W41);
}

#[test]
fn close_unfinished_week_fails() {
    let e = Env::fixture();
    e.tm_on("2026-10-15", &["close", "2026-w42"])
        .fail("tm: week 2026-w42 is not finished (ends 2026-10-18)");
    assert_eq!(e.read("backlog.md"), BACKLOG);
}

#[test]
fn close_nothing_to_close() {
    let e = Env::fixture();
    e.write("2026/w41.md", "- [x] a\n");
    let o = e.tm_on("2026-10-15", &["close"]);
    o.ok();
    assert!(o.stdout.contains("nothing to close in"), "{}", o.stdout);
    assert_eq!(e.read("backlog.md"), BACKLOG);
}

// 13
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
    assert_eq!(v["file"], "2026/w42.md");
    assert_eq!(v["exists"], true);
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
fn week_other_estimates_and_no_estimate() {
    let e = Env::empty();
    e.write(
        "2026/w42.md",
        "- [ ] a @est(3d)\n- [ ] b\n- [ ] c @est(1h)\n",
    );
    let v = e.tm(&["week", "--json"]).ok().json();
    assert_eq!(v["other_estimates"], json!(["3d"]));
    assert_eq!(f(&v["no_estimate"]), 1.0);
    assert_eq!(f(&v["planned_hours"]), 1.0);
    assert!(v["past_avg_done_hours"].is_null());
    assert_eq!(v["past_weeks"], json!([]));
}

// 14
#[test]
fn week_past_and_next() {
    let e = Env::fixture();
    let v = e.tm(&["week", "2026-w41", "--json"]).ok().json();
    assert_eq!(v["week"], "2026-w41");
    assert_eq!(v["status"], "past");

    let v = e.tm(&["week", "next", "--json"]).ok().json();
    assert_eq!(v["week"], "2026-w43");
    assert_eq!(v["exists"], false);
    assert_eq!(f(&v["tasks"]), 0.0);
    assert_eq!(f(&v["planned_hours"]), 0.0);
    assert_eq!(v["status"], "future");
}

// 15
#[test]
fn week_iso_year_edges() {
    let e = Env::fixture();
    let v = e.tm_on("2027-01-01", &["week", "--json"]).ok().json();
    assert_eq!(v["week"], "2026-w53");
    assert_eq!(v["start"], "2026-12-28");
    assert_eq!(v["end"], "2027-01-03");
    assert_eq!(v["file"], "2026/w53.md");

    let v = e.tm_on("2025-12-30", &["week", "--json"]).ok().json();
    assert_eq!(v["week"], "2026-w01");
    assert_eq!(v["file"], "2026/w01.md");
}

// 16
#[test]
fn list_json_backlog_open() {
    let e = Env::fixture();
    let v = e
        .tm(&["list", "--in", "backlog", "--open", "--json"])
        .ok()
        .json();
    let items = v.as_array().expect("array");
    assert_eq!(items.len(), 3);
    let it = items.iter().find(|i| f(&i["line"]) == 3.0).expect("line 3");
    assert_eq!(it["file"], "backlog.md");
    assert_eq!(it["done"], false);
    assert_eq!(it["description"], "kubernetes cluster runs wireguard");
    assert_eq!(it["tags"], json!(["self-hosting"]));
    assert_eq!(it["tokens"], json!({"est": "4h", "due": "2026-10-25"}));
    assert_eq!(f(&it["est_hours"]), 4.0);
    assert_eq!(it["text"], KUBE);
    let none = items.iter().find(|i| f(&i["line"]) == 4.0).unwrap();
    assert!(none["est_hours"].is_null());
}

// 17
#[test]
fn list_tag_filters() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--tag", "self"]);
    o.ok();
    assert_eq!(o.stdout, "");
    let o = e.tm(&["list", "--tag", "self-hosting"]);
    o.ok();
    assert_eq!(o.stdout.lines().count(), 3, "{}", o.stdout);
}

#[test]
fn list_tag_matches_subtags_and_requires_all() {
    let e = Env::empty();
    e.write(
        "backlog.md",
        "- [ ] a #health/sleep\n- [ ] b #health #fun\n- [ ] c #healthy\n",
    );
    let o = e.tm(&["list", "--tag", "health"]);
    o.ok();
    assert_eq!(
        o.stdout,
        "backlog.md:1: - [ ] a #health/sleep\nbacklog.md:2: - [ ] b #health #fun\n"
    );
    let o = e.tm(&["list", "--tag", "health", "--tag", "fun"]);
    o.ok();
    assert_eq!(o.stdout, "backlog.md:2: - [ ] b #health #fun\n");
}

#[test]
fn list_due_before() {
    let e = Env::fixture();
    let o = e.tm(&["list", "--due-before", "2026-10-20"]);
    o.ok();
    assert_eq!(
        o.stdout,
        "2026/w42.md:5: - [x] read ML slides @est(2h) @due(2026-10-14) #university\n"
    );
}

#[test]
fn list_default_order_and_text_format() {
    let e = Env::fixture();
    e.write("raw.md", "- [ ] raw thing\n");
    let o = e.tm(&["list", "--open"]);
    o.ok();
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert_eq!(lines[0], "raw.md:1: - [ ] raw thing");
    assert_eq!(
        lines[1],
        "backlog.md:2: - [ ] wireguard container forwards to mullvad @est(2h) #self-hosting"
    );
    let pos = |s: &str| lines.iter().position(|l| l.starts_with(s)).unwrap();
    assert!(pos("2026/w41.md:3:") < pos("2026/w42.md:4:"));
    assert!(pos("backlog.md:4:") < pos("2026/w41.md:3:"));
    assert_eq!(lines.len(), 7);
}

#[test]
fn list_query_in_done_proj_and_empty_json() {
    let e = Env::fixture();
    let o = e.tm(&["list", "BLOG"]);
    o.ok();
    assert_eq!(o.stdout.lines().count(), 2, "{}", o.stdout);

    let o = e.tm(&["list", "--in", "2026-w41", "--done"]);
    o.ok();
    assert_eq!(o.stdout.lines().count(), 2, "{}", o.stdout);

    e.write("backlog.md", "- [ ] a @proj(x)\n- [ ] b @proj(xy)\n");
    let o = e.tm(&["list", "--proj", "x"]);
    o.ok();
    assert_eq!(o.stdout, "backlog.md:1: - [ ] a @proj(x)\n");

    let o = e.tm(&["list", "zzzz", "--json"]);
    o.ok();
    assert_eq!(o.json(), json!([]));
    let o = e.tm(&["list", "zzzz"]);
    o.ok();
    assert_eq!(o.stdout, "");
}

// 18
#[test]
fn lint_clean_then_unknown_token() {
    let e = Env::fixture();
    let o = e.tm(&["lint"]);
    o.ok();
    assert_eq!(o.stdout, "");

    e.write("2026/w41.md", &with_line(W41, "- [ ] x @project(foo)"));
    let o = e.tm(&["lint"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_eq!(o.stdout, "2026/w41.md:4: unknown token @project\n");
}

#[test]
fn lint_excludes_raw() {
    let e = Env::fixture();
    e.write("raw.md", "- [ ] x @project(foo)\n");
    e.tm(&["lint"]).ok();
}

// 19
#[test]
fn missing_tasks_dir() {
    let e = Env::empty();
    let o = e.run_in(Path::new("/nonexistent"), "2026-10-18", &["lint"]);
    o.fail("tm: tasks dir not found: /nonexistent");
}

// 20
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

// amendments: single line text
#[test]
fn add_raw_promote_reject_multiline_text() {
    for bad in ["a\nb", "a\rb"] {
        let e = Env::fixture();
        e.write("raw.md", "- [ ] item\n");
        e.tm(&["add", bad]).fail("tm: text must be a single line");
        e.tm(&["raw", bad]).fail("tm: text must be a single line");
        e.tm(&["promote", "item", bad])
            .fail("tm: text must be a single line");
        assert_fixture_unchanged(&e);
        assert_eq!(e.read("raw.md"), "- [ ] item\n");
    }
}

#[test]
fn empty_reference_is_error() {
    let e = Env::fixture();
    e.write("raw.md", "- [ ] item\n");
    e.tm(&["done", ""]).fail("tm: empty reference");
    e.tm(&["take", " "]).fail("tm: empty reference");
    e.tm(&["promote", ""]).fail("tm: empty reference");
    assert_fixture_unchanged(&e);
    assert_eq!(e.read("raw.md"), "- [ ] item\n");
    assert!(!e.exists("2026/w43.md"));
}

#[test]
fn take_and_promote_refuse_done_task() {
    let e = Env::fixture();
    e.tm(&["take", "backlog.md:5"])
        .fail("tm: already done: backlog.md:5");
    assert_fixture_unchanged(&e);
    assert!(!e.exists("2026/w43.md"));

    e.write("raw.md", "- [x] finished\n");
    e.tm(&["promote", "raw.md:1"])
        .fail("tm: already done: raw.md:1");
    assert_eq!(e.read("raw.md"), "- [x] finished\n");
    assert_eq!(e.read("backlog.md"), BACKLOG);
}

#[test]
fn list_json_closed_pipe_does_not_panic() {
    let e = Env::empty();
    let mut b = String::new();
    for i in 0..20000 {
        b.push_str(&format!("- [ ] task number {i} @est(1h) #tag\n"));
    }
    e.write("backlog.md", &b);
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

// review gaps
#[test]
fn done_last_line_without_trailing_newline() {
    let e = Env::empty();
    e.write("2026/w42.md", "- [ ] first\n- [ ] last");
    let o = e.tm(&["done", "last"]);
    o.ok();
    assert_eq!(o.stdout.trim_end(), "2026/w42.md:2");
    assert_eq!(
        e.read("2026/w42.md"),
        "- [ ] first\n- [x] last @done(2026-10-18)"
    );
}

#[test]
fn take_middle_backlog_line() {
    let e = Env::fixture();
    e.tm(&["take", "mullvad"]).ok();
    assert_eq!(
        e.read("backlog.md"),
        "# Backlog
- [ ] kubernetes cluster runs wireguard @est(4h) @due(2026-10-25) #self-hosting
- [ ] sunshine on headless vm @est(3d) #self-hosting
- [x] old done thing @est(1h)
"
    );
    assert_eq!(
        e.read("2026/w43.md"),
        "- [ ] wireguard container forwards to mullvad @est(2h) #self-hosting\n"
    );
}

#[test]
fn take_last_backlog_line_without_trailing_newline() {
    let e = Env::empty();
    e.write("backlog.md", "# Backlog\n- [ ] a\n- [ ] last @est(1h)");
    let o = e.tm(&["take", "last"]);
    o.ok();
    assert_eq!(o.stdout.lines().next(), Some("2026/w43.md:1"));
    assert_eq!(e.read("backlog.md"), "# Backlog\n- [ ] a\n");
    assert_eq!(e.read("2026/w43.md"), "- [ ] last @est(1h)\n");
}

#[test]
fn take_same_task_twice_is_error() {
    let e = Env::fixture();
    let o = e.tm(&["take", "mullvad", "backlog.md:2"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_fixture_unchanged(&e);
    assert!(!e.exists("2026/w43.md"));
}

#[test]
fn take_partial_failure_writes_nothing() {
    let e = Env::fixture();
    let o = e.tm(&["take", "kubernetes", "nope"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
    assert_fixture_unchanged(&e);
    assert!(!e.exists("2026/w43.md"));
}

#[test]
fn take_size_boundaries() {
    for est in ["1d", "40h"] {
        let e = Env::empty();
        let line = format!("- [ ] t @est({est})");
        e.write("backlog.md", &format!("{line}\n"));
        e.tm(&["take", "t"]).ok();
        assert_eq!(e.read("2026/w43.md"), format!("{line}\n"), "{est}");
        assert_eq!(e.read("backlog.md"), "", "{est}");
    }
    for est in ["1.5d", "2m", "1y"] {
        let e = Env::empty();
        let content = format!("- [ ] t @est({est})\n");
        e.write("backlog.md", &content);
        e.tm(&["take", "t"]).fail("too big, split it first");
        assert_eq!(e.read("backlog.md"), content, "{est}");
        assert!(!e.exists("2026/w43.md"), "{est}");
    }
}

#[test]
fn promote_lint_failure_writes_nothing() {
    let e = Env::fixture();
    e.write("raw.md", "- [ ] a\n");
    e.tm(&["promote", "a", "x @est(2x)"])
        .fail("invalid @est value: 2x");
    assert_eq!(e.read("raw.md"), "- [ ] a\n");
    assert_eq!(e.read("backlog.md"), BACKLOG);
}

#[test]
fn add_empty_text_is_usage_error() {
    let e = Env::fixture();
    let o = e.tm(&["add", ""]);
    assert_eq!(o.code, 2, "stderr: {}", o.stderr);
    assert_eq!(e.read("backlog.md"), BACKLOG);
}
