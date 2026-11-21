use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

struct Fake(PathBuf);

impl Fake {
    fn new(kind: &str) -> Fake {
        let dir =
            std::env::temp_dir().join(format!("fake-forge-test-{}-{kind}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kind"), kind).unwrap();
        Fake(dir)
    }

    fn output(&self, args: &[&str]) -> std::process::Output {
        let bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake/forge");
        Command::new("sh")
            .arg(bin)
            .args(args)
            .env("FAKE_FORGE_DIR", &self.0)
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> (i32, String) {
        let o = self.output(args);
        (
            o.status.code().unwrap(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
        )
    }

    fn json(&self, args: &[&str]) -> Value {
        let (code, out) = self.run(args);
        assert_eq!(code, 0, "{args:?}");
        serde_json::from_str(&out).unwrap()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn fake_forge_behaves_like_the_contract() {
    let f = Fake::new("gitlab");
    assert_eq!(
        f.json(&["--remote", "origin", "repo"]),
        json!({"forge": "gitlab", "host": "gitlab.example.com", "project": "g/p"})
    );
    let pr = f.json(&[
        "pr", "create", "--head", "h", "--base", "main", "--title", "T", "--body", "B", "--draft",
    ]);
    assert_eq!(pr["ref"], "!1");
    assert_eq!(pr["draft"], true);
    f.json(&[
        "pr", "create", "--head", "h", "--base", "main", "--title", "T2",
    ]);
    assert_eq!(
        f.json(&["pr", "list", "--head", "h", "--limit", "1"])[0]["number"],
        2
    );
    assert_eq!(f.json(&["pr", "list"]).as_array().unwrap().len(), 2);
    assert_eq!(f.json(&["pr", "edit", "1", "--title", "X"])["title"], "X");
    assert_eq!(f.json(&["pr", "close", "1"])["state"], "closed");
    assert_eq!(f.json(&["pr", "reopen", "1"])["state"], "open");
    std::fs::write(f.0.join("reopen_refuse"), "").unwrap();
    assert_eq!(f.run(&["pr", "reopen", "1"]).0, 1);
    assert_eq!(f.run(&["pr", "view", "9"]).0, 1);
    assert_eq!(f.run(&["bogus"]).0, 99);
    assert_eq!(f.run(&["pr", "view", "1", "--nope"]).0, 99);
    let file = f.0.join("a.png");
    std::fs::write(&file, "x").unwrap();
    let file = file.to_str().unwrap();
    assert_eq!(
        f.json(&["upload", file])["url"],
        "/uploads/0123456789abcdef/a.png"
    );
    std::fs::write(f.0.join("upload_fail"), "").unwrap();
    assert_eq!(f.run(&["upload", file]).0, 1);
}

#[test]
fn fake_forge_github_kind() {
    let f = Fake::new("github");
    assert_eq!(f.json(&["repo"])["forge"], "github");
    assert_eq!(
        f.json(&["pr", "create", "--head", "h", "--base", "main", "--title", "T"])["ref"],
        "#1"
    );
    let file = f.0.join("a.png");
    std::fs::write(&file, "x").unwrap();
    let o = f.output(&["upload", file.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("github has no file upload API"), "{err}");
}
