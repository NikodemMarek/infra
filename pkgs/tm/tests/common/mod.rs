// Each test binary uses only part of the harness.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub const FIXTURE: &str = "# Tasks
- [ ] wireguard container forwards to mullvad @est(2h) #self-hosting
- [/] kubernetes cluster runs wireguard @est(4h) @due(2026-10-25) #self-hosting
- [ ] sunshine on headless vm @est(3d) #self-hosting
- [x] old done thing @est(1h)
- [ ] write a blog post @est(2h) #fun
- [x] read ML slides @est(2h) @due(2026-10-14) #university @done(2026-10-14)
- [/] stachiewicza wardrobe  @est(4h) #organization
notes here
";

pub const WIREGUARD: &str = "- [ ] wireguard container forwards to mullvad @est(2h) #self-hosting";

pub const KUBE: &str =
    "- [/] kubernetes cluster runs wireguard @est(4h) @due(2026-10-25) #self-hosting";

pub const SUNSHINE: &str = "- [ ] sunshine on headless vm @est(3d) #self-hosting";

pub const WARDROBE: &str = "- [/] stachiewicza wardrobe  @est(4h) #organization";

pub struct Env {
    pub dir: PathBuf,
}

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Env {
    pub fn empty() -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("tm-test-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Env { dir }
    }

    pub fn fixture() -> Env {
        let e = Env::empty();
        e.write("tasks.md", FIXTURE);
        e
    }

    pub fn write(&self, rel: &str, content: &str) {
        let p = self.dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.dir.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.dir.join(rel).exists()
    }

    pub fn run_in(&self, dir: &Path, today: &str, args: &[&str]) -> Out {
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

    pub fn tm(&self, args: &[&str]) -> Out {
        self.run_in(&self.dir, "2026-10-18", args)
    }

    pub fn tm_on(&self, today: &str, args: &[&str]) -> Out {
        self.run_in(&self.dir, today, args)
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

    pub fn usage_error(&self) -> &Out {
        assert_eq!(
            self.code, 2,
            "expected exit 2\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        self
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("bad json ({e}): {}\nstderr: {}", self.stdout, self.stderr))
    }
}

pub fn with_line(base: &str, line: &str) -> String {
    format!("{base}{line}\n")
}

pub fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

pub fn assert_fixture_unchanged(e: &Env) {
    assert_eq!(e.read("tasks.md"), FIXTURE);
}

// Every line except the given 1-based ones must be byte-identical.
pub fn assert_only_lines_changed(before: &str, after: &str, changed: &[usize]) {
    let b: Vec<&str> = before.split_inclusive('\n').collect();
    let a: Vec<&str> = after.split_inclusive('\n').collect();
    assert_eq!(b.len(), a.len(), "line count changed:\n{after}");
    for (i, (x, y)) in b.iter().zip(&a).enumerate() {
        if !changed.contains(&(i + 1)) {
            assert_eq!(x, y, "line {} changed", i + 1);
        }
    }
}

// Line numbers of a `N: <line>` text listing.
pub fn nums(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(|l| l.split(':').next().unwrap().to_string())
        .collect()
}

// Line numbers of a JSON listing.
pub fn json_lines(v: &Value) -> Vec<f64> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|i| f(&i["line"]))
        .collect()
}
