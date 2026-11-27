#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

// Checked in as executables: writing them at runtime races other threads' fork+exec (ETXTBSY).
pub fn fake_bin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake")
}

/// A running `plan-review serve`, killed on drop.
pub struct Server {
    child: Child,
    pub addr: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A 127.0.0.1 address nothing listens on (for the moment).
pub fn free_addr() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    format!("127.0.0.1:{}", l.local_addr().unwrap().port())
}

pub struct Env {
    pub dir: PathBuf,
    /// `host:port` of the server started by `Env::new`
    pub addr: String,
    server: Option<Server>,
    /// cwd of the commands, holds the plan files
    pub work: PathBuf,
    /// `$XDG_STATE_HOME`
    pub state: PathBuf,
    pub logs: PathBuf,
}

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.server.take();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Env {
    pub fn new() -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = fs::canonicalize(std::env::temp_dir()).unwrap();
        let dir = base.join(format!("plan-review-test-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        let mut e = Env {
            work: dir.join("work"),
            state: dir.join("state"),
            logs: dir.join("logs"),
            dir,
            addr: String::new(),
            server: None,
        };
        for d in [&e.work, &e.state, &e.logs, &e.dir.join("home")] {
            fs::create_dir_all(d).unwrap();
        }
        let server = e.start_server(&[], &[]);
        e.addr = server.addr.clone();
        e.server = Some(server);
        e
    }

    /// `http://<addr>` of the server started by `Env::new`
    pub fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Starts `plan-review serve` on a free port with the default test environment
    /// (minus `unset`, plus `envs`) and waits until it accepts connections.
    pub fn start_server(&self, unset: &[&str], envs: &[(&str, &str)]) -> Server {
        for _ in 0..3 {
            let addr = free_addr();
            let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_plan-review"));
            c.current_dir(&self.work)
                .env_clear()
                .env("PATH", fake_bin())
                .env("HOME", self.dir.join("home"))
                .env("XDG_STATE_HOME", &self.state)
                .env("FAKE_LOG_DIR", &self.logs)
                .env("PLAN_REVIEW_ADDR", &addr)
                .arg("serve")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            for k in unset {
                c.env_remove(k);
            }
            c.envs(envs.iter().copied());
            let mut server = Server {
                child: c.spawn().unwrap(),
                addr,
            };
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(3) {
                if server.child.try_wait().unwrap().is_some() {
                    break;
                }
                if TcpStream::connect(&server.addr).is_ok() {
                    return server;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        panic!("plan-review serve did not start");
    }

    /// `<state>/plan-review`
    pub fn root(&self) -> PathBuf {
        self.state.join("plan-review")
    }

    pub fn run(&self, cwd: &Path, envs: &[(&str, &str)], stdin: &str, args: &[&str]) -> Out {
        self.run_without(&[], cwd, envs, stdin, args)
    }

    /// Like `run`, but the named variables are removed from the default environment first.
    pub fn run_without(
        &self,
        unset: &[&str],
        cwd: &Path,
        envs: &[(&str, &str)],
        stdin: &str,
        args: &[&str],
    ) -> Out {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_plan-review"));
        c.current_dir(cwd)
            .env_clear()
            .env("PATH", fake_bin())
            .env("HOME", self.dir.join("home"))
            .env("XDG_STATE_HOME", &self.state)
            .env("FAKE_LOG_DIR", &self.logs)
            .env("PLAN_REVIEW_ADDR", &self.addr)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for k in unset {
            c.env_remove(k);
        }
        c.envs(envs.iter().copied());
        let mut child = c.spawn().unwrap();
        // the binary may exit without reading stdin
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let start = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if start.elapsed() > Duration::from_secs(20) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("plan-review {args:?} did not exit within 20 s (blocked?)");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let o = child.wait_with_output().unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    pub fn publish_as(&self, session: &str, file: &str, title: &str) -> Out {
        self.run(
            &self.work,
            &[("AGENT_SESSION", session)],
            "",
            &["publish", file, title],
        )
    }

    pub fn publish(&self, file: &str, title: &str) -> Out {
        self.publish_as("sess-1", file, title)
    }

    pub fn write_plan(&self, name: &str, content: &str) {
        fs::write(self.work.join(name), content).unwrap();
    }

    pub fn intake(&self, stdin: &str) -> Out {
        self.run(&self.work, &[], stdin, &["intake"])
    }

    pub fn list(&self) -> Out {
        self.run(&self.work, &[], "", &["list"])
    }

    /// Review ids (directory names) under the root.
    pub fn ids(&self) -> Vec<String> {
        let mut v: Vec<String> = match fs::read_dir(self.root()) {
            Ok(rd) => rd
                .map(|e| e.unwrap())
                .filter(|e| e.file_type().unwrap().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => vec![],
        };
        v.sort();
        v
    }

    pub fn meta(&self, id: &str) -> Value {
        let p = self.root().join(id).join("meta.json");
        serde_json::from_str(
            &fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
        )
        .unwrap()
    }

    pub fn set_meta(&self, id: &str, key: &str, value: Value) {
        let mut m = self.meta(id);
        m[key] = value;
        fs::write(
            self.root().join(id).join("meta.json"),
            serde_json::to_string(&m).unwrap(),
        )
        .unwrap();
    }

    pub fn log(&self, name: &str) -> String {
        fs::read_to_string(self.logs.join(name)).unwrap_or_default()
    }

    pub fn set_flag(&self, name: &str, content: &str) {
        fs::write(self.logs.join(name), content).unwrap();
    }

    pub fn clear_flag(&self, name: &str) {
        let _ = fs::remove_file(self.logs.join(name));
    }

    /// Args of each invocation recorded by the notify-send / xdg-open fakes.
    pub fn calls(&self, log: &str) -> Vec<Vec<String>> {
        let mut calls = vec![];
        let mut cur = vec![];
        for line in self.log(log).lines() {
            if line == "--end--" {
                calls.push(std::mem::take(&mut cur));
            } else if let Some(a) = line.strip_prefix("arg=") {
                cur.push(a.to_string());
            }
        }
        calls
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
