#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Map, Value};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const JJ_USER: &str = "[user]\nname = \"Test\"\nemail = \"test@example.com\"\n";

pub struct Env {
    pub dir: PathBuf,
    pub cwd: PathBuf,
    cred: Vec<String>,
    empty_token: bool,
    routes: Map<String, Value>,
}

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn fails(&self) -> &Out {
        assert_eq!(
            self.code, 1,
            "expected exit 1\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        assert!(
            self.stderr.starts_with("forge: "),
            "stderr should start with `forge: `: {}",
            self.stderr
        );
        assert!(self.stdout.is_empty(), "stdout: {}", self.stdout);
        self
    }

    pub fn usage_error(&self) -> &Out {
        assert_eq!(
            self.code, 2,
            "expected exit 2\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        self
    }

    // Exit 0 with exactly one compact JSON value followed by a single newline.
    pub fn json(&self) -> Value {
        assert_eq!(
            self.code, 0,
            "expected exit 0\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        assert!(self.stdout.ends_with('\n'), "no trailing newline");
        let body = &self.stdout[..self.stdout.len() - 1];
        assert!(!body.contains('\n'), "more than one line: {}", self.stdout);
        let v: Value = serde_json::from_str(body)
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", self.stdout));
        assert_eq!(
            serde_json::to_string(&v).unwrap().len(),
            body.len(),
            "not compact: {body}"
        );
        v
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
    fn base() -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("forge-test-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("home")).unwrap();
        fs::create_dir_all(dir.join("fake")).unwrap();
        fs::create_dir_all(dir.join("cwd")).unwrap();
        fs::write(dir.join("jj-config.toml"), JJ_USER).unwrap();
        let e = Env {
            cwd: dir.join("cwd"),
            dir,
            cred: vec![],
            empty_token: false,
            routes: Map::new(),
        };
        e.write_routes();
        e
    }

    // A directory that is not inside any jj repo.
    pub fn outside() -> Env {
        Env::base()
    }

    // A jj repo with the given remotes and extra jj config (TOML).
    pub fn in_repo(remotes: &[(&str, &str)], config: &str) -> Env {
        let mut e = Env::base();
        run(e
            .cmd("jj")
            .args(["git", "init", "repo"])
            .current_dir(&e.dir));
        e.cwd = e.dir.join("repo");
        let path = run(e.cmd("jj").args(["config", "path", "--repo"]));
        fs::write(path.trim(), config).unwrap();
        for (name, url) in remotes {
            run(e.cmd("jj").args(["git", "remote", "add", name, url]));
        }
        e
    }

    // Hosts for which gh/glab report a credential.
    pub fn cred(mut self, hosts: &[&str]) -> Env {
        self.cred = hosts.iter().map(|h| h.to_string()).collect();
        self
    }

    // Credentialed hosts whose `gh auth token` prints nothing.
    pub fn empty_token(mut self) -> Env {
        self.empty_token = true;
        self
    }

    fn write_routes(&self) {
        fs::write(
            self.dir.join("fake/routes.json"),
            serde_json::to_string(&self.routes).unwrap(),
        )
        .unwrap();
    }

    // Serve `response` for `"<METHOD> <path>"`.
    pub fn route(&mut self, key: &str, response: Value) {
        self.routes.insert(key.to_string(), response);
        self.write_routes();
    }

    pub fn route_error(&mut self, key: &str, message: &str) {
        self.route(key, serde_json::json!({ "__error": message }));
    }

    fn cmd(&self, prog: &str) -> Command {
        let mut c = Command::new(prog);
        let path = format!(
            "{}:{}",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fake/bin")
                .display(),
            std::env::var("PATH").unwrap_or_default()
        );
        c.current_dir(&self.cwd)
            .env("PATH", path)
            .env("HOME", self.dir.join("home"))
            .env("XDG_CONFIG_HOME", self.dir.join("home/config"))
            .env("JJ_CONFIG", self.dir.join("jj-config.toml"))
            .env("JJ_EDITOR", "true")
            .env("FAKE_DIR", self.dir.join("fake"))
            .env("FAKE_CRED", self.cred.join(" "))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        if self.empty_token {
            c.env("FAKE_EMPTY_TOKEN", "1");
        }
        c
    }

    pub fn forge(&self, args: &[&str]) -> Out {
        let o = self
            .cmd(env!("CARGO_BIN_EXE_forge"))
            .args(args)
            .output()
            .unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    pub fn forge_stdin(&self, args: &[&str], input: &str) -> Out {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = self
            .cmd(env!("CARGO_BIN_EXE_forge"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let data = input.as_bytes().to_vec();
        let t = std::thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
        let o = child.wait_with_output().unwrap();
        let _ = t.join();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    // Sequenced responses for one route key: the n-th call gets the n-th item, the last repeats.
    pub fn route_seq(&mut self, key: &str, items: Vec<Value>) {
        self.route(key, serde_json::json!({ "__seq": items }));
    }

    fn log(&self, name: &str) -> Vec<Value> {
        fs::read_to_string(self.dir.join("fake").join(name))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    // Each gh/glab `api` call: {tool, host, method, path, body, headers}.
    // `%3A` in the path is normalized to `:`.
    pub fn api_calls(&self) -> Vec<Value> {
        self.log("calls.log")
    }

    // Each gh/glab `auth` call: {tool, sub, host}.
    pub fn auth_calls(&self) -> Vec<Value> {
        self.log("auth.log")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub fn merge(mut base: Value, over: Value) -> Value {
    let b = base.as_object_mut().unwrap();
    for (k, v) in over.as_object().unwrap() {
        b.insert(k.clone(), v.clone());
    }
    base
}
