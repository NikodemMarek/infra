#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use serde_json::Value;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const FAKES: &[(&str, &str)] = &[
    ("shpool", include_str!("../fake/shpool")),
    ("direnv", include_str!("../fake/direnv")),
    ("claude", include_str!("../fake/claude")),
    ("jj-stack", include_str!("../fake/jj-stack")),
    ("forge", include_str!("../fake/forge")),
    ("event-pub", include_str!("../fake/event-pub")),
    ("gitlab-context", include_str!("../fake/gitlab-context")),
    (
        "claude-jj-workspace",
        include_str!("../fake/claude-jj-workspace"),
    ),
];

pub struct Env {
    pub dir: PathBuf,
    pub home: PathBuf,
    pub root: PathBuf,
    pub state: PathBuf,
    pub runtime: PathBuf,
    pub logs: PathBuf,
    bin: PathBuf,
    jj_config: PathBuf,
}

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Env {
    pub fn new() -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = fs::canonicalize(std::env::temp_dir()).unwrap();
        let dir = base.join(format!("jw-test-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        let e = Env {
            home: dir.join("home"),
            root: dir.join("home/projects"),
            state: dir.join("state"),
            runtime: dir.join("runtime"),
            logs: dir.join("logs"),
            bin: dir.join("bin"),
            jj_config: dir.join("jj-config.toml"),
            dir,
        };
        for d in [&e.root, &e.state, &e.runtime, &e.logs, &e.bin] {
            fs::create_dir_all(d).unwrap();
        }
        fs::create_dir_all(e.dir.join("config")).unwrap();
        fs::create_dir_all(e.dir.join("claude")).unwrap();
        for (name, body) in FAKES {
            let p = e.bin.join(name);
            fs::write(&p, body).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(
            &e.jj_config,
            format!(
                "[user]\nname = \"Test\"\nemail = \"test@example.com\"\n\n\
                 [revset-aliases]\n\"trunk()\" = \"main\"\n\n\
                 [aliases]\nstack = [\"util\", \"exec\", \"--\", \"sh\", \"{}\"]\n",
                e.bin.join("jj-stack").display()
            ),
        )
        .unwrap();
        e
    }

    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>, cwd: &Path) -> Command {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut c = Command::new(program);
        c.current_dir(cwd)
            .env_clear()
            .env("PATH", path)
            .env("HOME", &self.home)
            .env("JW_PROJECTS_ROOT", &self.root)
            .env("XDG_STATE_HOME", &self.state)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("CLAUDE_CONFIG_DIR", self.dir.join("claude"))
            .env("JJ_CONFIG", &self.jj_config)
            .env("FAKE_LOG_DIR", &self.logs)
            .env("SHPOOL_STATE", self.dir.join("shpool-state"));
        c
    }

    pub fn jw(&self, cwd: &Path, args: &[&str]) -> Out {
        self.jw_env(cwd, &[], args)
    }

    pub fn jw_env(&self, cwd: &Path, envs: &[(&str, &str)], args: &[&str]) -> Out {
        let mut c = self.command(env!("CARGO_BIN_EXE_jw"), cwd);
        c.args(args).envs(envs.iter().copied());
        let o = c.output().unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    pub fn jw_stdin(&self, cwd: &Path, stdin: &str, args: &[&str]) -> Out {
        use std::io::Write;
        use std::process::Stdio;
        let mut c = self.command(env!("CARGO_BIN_EXE_jw"), cwd);
        let mut child = c
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    pub fn jj(&self, cwd: &Path, args: &[&str]) -> String {
        let o = self
            .command("jj", cwd)
            .arg("--no-pager")
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "jj {args:?} failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    /// One repo, `repo`, under the projects root: a commit on bookmark `main`, an empty working copy on top.
    pub fn repo(&self) -> PathBuf {
        let home = self.root.join("repo");
        self.jj(&self.root, &["git", "init", "repo"]);
        fs::write(home.join("file"), "content\n").unwrap();
        self.jj(&home, &["commit", "-m", "init"]);
        self.jj(&home, &["bookmark", "create", "main", "-r", "@-"]);
        home
    }

    /// A jj workspace `repo.agents/<name>` on main.
    pub fn workspace(&self, name: &str) -> PathBuf {
        let d = self.root.join("repo.agents").join(name);
        fs::create_dir_all(d.parent().unwrap()).unwrap();
        self.jj(
            &self.root.join("repo"),
            &[
                "workspace",
                "add",
                "--name",
                name,
                "-r",
                "main",
                d.to_str().unwrap(),
            ],
        );
        d
    }

    pub fn record_path(&self, name: &str) -> PathBuf {
        self.state
            .join("agent-sessions")
            .join(format!("{name}.json"))
    }

    pub fn record(&self, name: &str) -> Value {
        let p = self.record_path(name);
        let text = fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad record json ({e}): {text}"))
    }

    pub fn write_record(&self, name: &str, json: &str) {
        let p = self.record_path(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, json).unwrap();
    }

    pub fn log(&self, name: &str) -> String {
        let p = self.logs.join(name);
        fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    }
}

/// Make `d` look untouched for two hours (prune skips recently modified workspaces).
pub fn age(d: &Path) {
    let old = SystemTime::now() - Duration::from_secs(2 * 3600);
    for p in [d.to_path_buf(), d.join(".jj/working_copy")] {
        fs::File::open(&p).unwrap().set_modified(old).unwrap();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
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

    pub fn fail(&self, stderr_has: &str) -> &Out {
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

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("bad json ({e}): {}\nstderr: {}", self.stdout, self.stderr))
    }

    pub fn stderr_lines(&self) -> Vec<&str> {
        self.stderr.lines().collect()
    }
}
