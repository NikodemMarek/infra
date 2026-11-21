// Each test binary uses only part of the harness.
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, PartialEq)]
pub enum Forge {
    GitLab,
    GitHub,
}

pub struct Env {
    pub dir: PathBuf,
    pub repo: PathBuf,
    pub remote: PathBuf,
    pub forge: Forge,
    pub base: String,
}

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub struct Change {
    pub change: String,
    pub branch: String,
}

impl Out {
    pub fn ok(&self) -> &Out {
        assert_eq!(
            self.code, 0,
            "expected exit 0\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        self
    }
}

fn run(cmd: &mut Command) -> String {
    let o = cmd.output().unwrap();
    assert!(
        o.status.success(),
        "{:?} failed\nstdout: {}\nstderr: {}",
        cmd,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

impl Env {
    // Repo with a pushed `main` and an empty working copy on top of it.
    pub fn new(forge: Forge) -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("jj-upload-test-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("home")).unwrap();
        let repo = dir.join("repo");
        let remote = dir.join("remote.git");

        let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake");
        for name in ["forge", "jw"] {
            let to = dir.join("bin").join(name);
            fs::copy(fake.join(name), &to).unwrap();
            fs::set_permissions(&to, fs::Permissions::from_mode(0o755)).unwrap();
        }

        fs::write(
            dir.join("jj-config.toml"),
            "[user]\nname = \"Test\"\nemail = \"test@example.com\"\n\n\
             [remotes.origin]\nauto-track-created-bookmarks = \"*\"\n\n\
             [upload]\nbookmark-prefix = \"nm/\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.join("forge")).unwrap();
        fs::write(
            dir.join("forge/kind"),
            match forge {
                Forge::GitLab => "gitlab",
                Forge::GitHub => "github",
            },
        )
        .unwrap();

        let mut e = Env {
            dir,
            repo,
            remote,
            forge,
            base: String::new(),
        };
        run(Command::new("git")
            .args(["init", "--bare", "-q", "-b", "main"])
            .arg(&e.remote)
            .envs(e.git_env()));
        run(e
            .cmd("jj")
            .args(["git", "init", "repo"])
            .current_dir(&e.dir));
        let remote = e.remote.to_string_lossy().into_owned();
        e.jj(&["git", "remote", "add", "origin", &remote]);
        e.write("base.txt", "base\n");
        e.jj(&["commit", "-m", "base"]);
        e.jj(&["bookmark", "create", "main", "-r", "@-"]);
        e.jj(&["git", "push", "--remote", "origin", "--bookmark", "main"]);
        e.base = e.remote_branch("main").unwrap();
        e
    }

    fn git_env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("GIT_AUTHOR_NAME", "Test".into()),
            ("GIT_AUTHOR_EMAIL", "test@example.com".into()),
            ("GIT_COMMITTER_NAME", "Test".into()),
            ("GIT_COMMITTER_EMAIL", "test@example.com".into()),
            ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ]
    }

    fn cmd(&self, prog: &str) -> Command {
        let mut c = Command::new(prog);
        let path = format!(
            "{}:{}",
            self.dir.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        c.current_dir(&self.repo)
            .env("PATH", path)
            .env("HOME", self.dir.join("home"))
            .env("XDG_CONFIG_HOME", self.dir.join("home/config"))
            .env("XDG_STATE_HOME", self.state_dir())
            .env("JJ_CONFIG", self.dir.join("jj-config.toml"))
            .env("JJ_EDITOR", "true")
            .env("FAKE_FORGE_DIR", self.forge_dir())
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("AGENT_SESSION")
            .envs(self.git_env());
        c
    }

    pub fn state_dir(&self) -> PathBuf {
        self.dir.join("state")
    }

    pub fn forge_dir(&self) -> PathBuf {
        self.dir.join("forge")
    }

    pub fn write(&self, rel: &str, content: &str) {
        fs::write(self.repo.join(rel), content).unwrap();
    }

    pub fn jj(&self, args: &[&str]) -> String {
        run(self
            .cmd("jj")
            .args(["--no-pager", "--color=never"])
            .args(args))
    }

    // Short change id and commit id of a revision.
    pub fn change_of(&self, rev: &str) -> String {
        self.jj(&["log", "--no-graph", "-r", rev, "-T", "change_id.short(8)"])
    }

    pub fn commit_of(&self, rev: &str) -> String {
        self.jj(&["log", "--no-graph", "-r", rev, "-T", "commit_id"])
    }

    pub fn description_of(&self, rev: &str) -> String {
        self.jj(&["log", "--no-graph", "-r", rev, "-T", "description"])
    }

    pub fn parent_commit_of(&self, rev: &str) -> String {
        self.jj(&[
            "log",
            "--no-graph",
            "-r",
            rev,
            "-T",
            "parents.map(|p| p.commit_id()).join(\",\")",
        ])
    }

    // Adds a described change touching `file` on top of the empty working copy,
    // then a new empty working copy. Returns its change id.
    pub fn add_change(&self, file: &str, message: &str) -> String {
        self.write(file, &format!("{message}\n"));
        self.jj(&["describe", "-m", message]);
        let c = self.change_of("@");
        self.jj(&["new"]);
        c
    }

    // Stack A (`feat: a`, body `Body A`) <- B (`feat: b`), empty @ on top.
    pub fn with_stack(forge: Forge) -> (Env, Change, Change) {
        let e = Env::new(forge);
        let a = e.add_change("a.txt", "feat: a\n\nBody A");
        let b = e.add_change("b.txt", "feat: b");
        let a = Change {
            branch: format!("nm/feat-a-{a}"),
            change: a,
        };
        let b = Change {
            branch: format!("nm/feat-b-{b}"),
            change: b,
        };
        (e, a, b)
    }

    pub fn upload(&self, args: &[&str]) -> Out {
        self.upload_with(args, &[])
    }

    pub fn upload_with(&self, args: &[&str], env: &[(&str, &str)]) -> Out {
        let o = self
            .cmd(env!("CARGO_BIN_EXE_jj-upload"))
            .args(args)
            .envs(env.iter().copied())
            .output()
            .unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    pub fn remote_branch(&self, name: &str) -> Option<String> {
        let o = Command::new("git")
            .arg("-C")
            .arg(&self.remote)
            .args(["rev-parse", "-q", "--verify", &format!("refs/heads/{name}")])
            .output()
            .unwrap();
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    pub fn remote_branches(&self, prefix: &str) -> Vec<String> {
        let o = run(Command::new("git").arg("-C").arg(&self.remote).args([
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("refs/heads/{prefix}"),
        ]));
        o.lines().map(String::from).collect()
    }

    // New commit on the remote main, made behind jj's back; returns its id.
    pub fn advance_remote_main(&self) -> String {
        let git = |args: &[&str]| {
            run(Command::new("git")
                .arg("-C")
                .arg(&self.remote)
                .args(args)
                .envs(self.git_env()))
            .trim()
            .to_string()
        };
        let sha = git(&["commit-tree", "main^{tree}", "-p", "main", "-m", "other"]);
        git(&["update-ref", "refs/heads/main", &sha]);
        sha
    }

    pub fn set_remote_main(&self, sha: &str) {
        run(Command::new("git").arg("-C").arg(&self.remote).args([
            "update-ref",
            "refs/heads/main",
            sha,
        ]));
    }

    fn prs_path(&self) -> PathBuf {
        self.forge_dir().join("prs.json")
    }

    // All PRs in the fake forge's state, contract-shaped, in creation order.
    pub fn prs(&self) -> Vec<Value> {
        match fs::read_to_string(self.prs_path()) {
            Ok(s) => serde_json::from_str(&s).unwrap(),
            Err(_) => Vec::new(),
        }
    }

    pub fn mr(&self, n: usize) -> Value {
        self.prs()
            .into_iter()
            .find(|p| p["number"] == n)
            .unwrap_or_else(|| panic!("no PR {n} in the fake forge"))
    }

    pub fn set_mr(&self, n: usize, v: &Value) {
        let mut all = self.prs();
        let slot = all
            .iter_mut()
            .find(|p| p["number"] == n)
            .unwrap_or_else(|| panic!("no PR {n} in the fake forge"));
        *slot = v.clone();
        fs::write(self.prs_path(), serde_json::to_string(&all).unwrap()).unwrap();
    }

    pub fn mr_count(&self) -> usize {
        self.prs().len()
    }

    // Switches a fake behaviour on: "reopen_refuse" or "upload_fail".
    pub fn fake_on(&self, flag: &str) {
        fs::write(self.forge_dir().join(flag), "").unwrap();
    }

    fn log_lines(&self, name: &str) -> Vec<Vec<String>> {
        fs::read_to_string(self.forge_dir().join(name))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    // argv of each forge call, in order.
    pub fn calls(&self) -> Vec<Vec<String>> {
        self.log_lines("calls.log")
    }

    // argv of each jw call, in order.
    pub fn jw_calls(&self) -> Vec<Vec<String>> {
        self.log_lines("jw.log")
    }

    // argv without the leading `--remote NAME`.
    pub fn args(call: &[String]) -> &[String] {
        match call {
            [flag, _, rest @ ..] if flag == "--remote" => rest,
            _ => call,
        }
    }

    // True for `pr <sub>` with the given sub (create, edit, close, reopen, ...).
    pub fn is_pr(call: &[String], sub: &str) -> bool {
        matches!(Env::args(call), [pr, s, ..] if pr == "pr" && s == sub)
    }

    pub fn is_mutating(call: &[String]) -> bool {
        ["create", "edit", "close", "reopen"]
            .iter()
            .any(|s| Env::is_pr(call, s))
            || matches!(Env::args(call), [u, ..] if u == "upload")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

// The stack block for `refs` (e.g. ["!1", "!2"]) with `marked` highlighted.
pub fn stack_block(refs: &[&str], marked: &str, forge: Forge) -> String {
    let this = match forge {
        Forge::GitLab => "this MR",
        Forge::GitHub => "this PR",
    };
    let items: Vec<String> = refs
        .iter()
        .map(|r| {
            if *r == marked {
                format!("- {r} \u{1F448} {this}")
            } else {
                format!("- {r}")
            }
        })
        .collect();
    format!(
        "<!-- jj-stack:start -->\n\n**Stack**:\n\n{}\n\n<!-- jj-stack:end -->",
        items.join("\n")
    )
}
