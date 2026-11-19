mod common;

use std::path::Path;
use std::process::Command;

use common::{assert_fixture_unchanged, Env};

#[test]
fn other_files_next_to_task_file_are_untouched() {
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

// removed commands and flags
#[test]
fn removed_subcommands_are_usage_errors() {
    let e = Env::fixture();
    for args in [&["week"][..], &["take", "2"][..], &["close"][..]] {
        e.tm(args).usage_error();
    }
    assert_fixture_unchanged(&e);
}

#[test]
fn missing_task_file() {
    let e = Env::empty();
    let o = e.run_in(Path::new("/nonexistent/tasks.md"), "2026-10-18", &["lint"]);
    o.fail("tm: task file not found: /nonexistent/tasks.md");
}

#[test]
fn unset_tm_file_is_error() {
    for tm_file in [None, Some("")] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tm"));
        cmd.arg("lint").env("TM_TODAY", "2026-10-18");
        match tm_file {
            Some(file) => cmd.env("TM_FILE", file),
            None => cmd.env_remove("TM_FILE"),
        };
        let o = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(1), "stderr: {stderr}");
        assert!(stderr.contains("tm: TM_FILE is not set"), "{stderr}");
    }
}

#[test]
fn invalid_today_is_error() {
    let e = Env::fixture();
    let o = e.tm_on("not-a-date", &["lint"]);
    assert_eq!(o.code, 1, "stderr: {}", o.stderr);
}

#[test]
fn help_shows_flag_and_arg_names() {
    let e = Env::empty();
    let help = |args: &[&str]| e.tm(args).ok().stdout.clone();
    let h = help(&["list", "--help"]);
    assert!(h.contains("--tag <TAG>"), "{h}");
    assert!(h.contains("--due-before <DATE>"), "{h}");
    let h = help(&["start", "--help"]);
    assert!(h.contains("<REF>..."), "{h}");
}
