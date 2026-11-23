use super::*;
use std::os::unix::fs::symlink;

fn fixture(name: &str) -> PathBuf {
    let root = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("jw-test-{}-realpath-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn realpath_existing_dir_through_symlinked_parent() {
    let root = fixture("existing");
    fs::create_dir_all(root.join("real/child")).unwrap();
    symlink(root.join("real"), root.join("link")).unwrap();
    assert_eq!(realpath(root.join("link/child")), root.join("real/child"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn realpath_existing_symlink_resolves_to_target() {
    let root = fixture("symlink");
    fs::create_dir_all(root.join("target")).unwrap();
    symlink(root.join("target"), root.join("link")).unwrap();
    assert_eq!(realpath(root.join("link")), root.join("target"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn realpath_missing_leaf_keeps_canonical_parent() {
    let root = fixture("missing");
    fs::create_dir_all(root.join("real")).unwrap();
    symlink(root.join("real"), root.join("link")).unwrap();
    assert_eq!(realpath(root.join("link/gone")), root.join("real/gone"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn realpath_dangling_symlink_keeps_link_own_path() {
    let root = fixture("dangling");
    fs::create_dir_all(root.join("real")).unwrap();
    symlink(root.join("real"), root.join("plink")).unwrap();
    symlink(root.join("nowhere"), root.join("real/dead")).unwrap();
    assert_eq!(realpath(root.join("plink/dead")), root.join("real/dead"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn realpath_nothing_resolvable_is_unchanged() {
    let p = PathBuf::from("/nonexistent-jw-test/a/b");
    assert_eq!(realpath(&p), p);
}
