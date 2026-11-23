use super::*;

#[test]
fn slug_polish_diacritics() {
    assert_eq!(slug("Zażółć gęślą jaźń!"), "zazolc-gesla-jazn");
}

#[test]
fn slug_plain_text() {
    assert_eq!(slug("Hello, World!"), "hello-world");
}

#[test]
fn slug_only_punctuation_is_empty() {
    assert_eq!(slug("!!!"), "");
    assert_eq!(slug(""), "");
}

#[test]
fn slug_capital_l_stroke() {
    assert_eq!(slug("Łódź"), "lodz");
}

#[test]
fn slug_collapses_runs_and_trims() {
    assert_eq!(slug("  --a   b__c--  "), "a-b-c");
}

#[test]
fn slug_drops_non_ascii_without_separator() {
    assert_eq!(slug("a日本b"), "ab");
}

#[test]
fn slug_keeps_digits() {
    assert_eq!(slug("Fix bug #42"), "fix-bug-42");
}

#[test]
fn slug_truncates_to_40_chars() {
    assert_eq!(slug(&"a".repeat(60)), "a".repeat(40));
}

#[test]
fn slug_trims_dash_left_by_truncation() {
    let title = format!("{} b", "a".repeat(39));
    assert_eq!(slug(&title), "a".repeat(39));
}

use std::os::unix::fs::symlink;

fn sandbox(name: &str) -> PathBuf {
    let root = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("jw-test-{}-delete-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn fixture_entry(root: &Path, agents: &Path, dir: &Path) -> (Registry, Entry) {
    let reg = Registry {
        dir: root.join("registry"),
    };
    fs::create_dir_all(&reg.dir).unwrap();
    let entry = Entry {
        name: "agent-0123abcd".to_string(),
        root: root.join("nonexistent-root").to_string_lossy().into_owned(),
        dir: dir.to_string_lossy().into_owned(),
        agents: agents.to_string_lossy().into_owned(),
        base: "x".to_string(),
        kind: "agent".to_string(),
        touched: 0.0,
        ws: None,
        warned: false,
    };
    reg.save(&entry).unwrap();
    (reg, entry)
}

fn assert_refused(reg: &Registry, entry: &Entry, kept: &Path) {
    let err = delete_workspace(reg, entry).unwrap_err();
    assert!(err.starts_with("refusing to delete "), "{err}");
    assert!(kept.exists());
    assert!(reg.path(&entry.name).exists());
}

#[test]
fn delete_refuses_dir_outside_agents_directory() {
    let root = sandbox("outside");
    let dir = root.join("elsewhere/a-0123abcd");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("f"), "x").unwrap();
    let agents = root.join("repo.agents");
    fs::create_dir_all(&agents).unwrap();
    let (reg, entry) = fixture_entry(&root, &agents, &dir);
    assert_refused(&reg, &entry, &dir.join("f"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn delete_refuses_agents_without_suffix() {
    let root = sandbox("suffix");
    let agents = root.join("repo-agents");
    let dir = agents.join("a-0123abcd");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("f"), "x").unwrap();
    let (reg, entry) = fixture_entry(&root, &agents, &dir);
    assert_refused(&reg, &entry, &dir.join("f"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn delete_refuses_symlink_to_directory_outside() {
    let root = sandbox("symlink");
    let agents = root.join("repo.agents");
    fs::create_dir_all(&agents).unwrap();
    let outside = root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("precious"), "x").unwrap();
    let link = agents.join("link");
    symlink(&outside, &link).unwrap();
    let (reg, entry) = fixture_entry(&root, &agents, &link);
    assert_refused(&reg, &entry, &outside.join("precious"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn delete_refuses_invalid_basename() {
    let root = sandbox("basename");
    let agents = root.join("repo.agents");
    let dir = agents.join("bad name");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("f"), "x").unwrap();
    let (reg, entry) = fixture_entry(&root, &agents, &dir);
    assert_refused(&reg, &entry, &dir.join("f"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn delete_dangling_symlink_removes_only_the_link() {
    let root = sandbox("dangling");
    let agents = root.join("repo.agents");
    fs::create_dir_all(&agents).unwrap();
    let keep = agents.join("a-feed0001");
    fs::create_dir_all(&keep).unwrap();
    let link = agents.join("a-0123abcd");
    symlink(root.join("nowhere"), &link).unwrap();
    let (reg, entry) = fixture_entry(&root, &agents, &link);

    delete_workspace(&reg, &entry).unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
    assert!(keep.is_dir());
    assert!(!reg.path(&entry.name).exists());
    fs::remove_dir_all(&root).unwrap();
}
