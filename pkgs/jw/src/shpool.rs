use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

pub struct Shpool;

impl Shpool {
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("shpool");
        cmd.arg("--no-daemonize").args(args);
        cmd
    }

    /// The state (attached or disconnected) of each shpool session.
    pub fn list(&self) -> Result<HashMap<String, String>, String> {
        let out = self
            .command(&["list"])
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| format!("shpool: {e}"))?;
        if !out.status.success() {
            return Err("shpool list failed (is shpool.socket running?)".to_string());
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        Ok(stdout
            .lines()
            .skip(1)
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                Some((fields.first()?.to_string(), fields.last()?.to_string()))
            })
            .collect())
    }

    pub fn kill(&self, name: &str) -> Result<bool, String> {
        let status = self
            .command(&["kill", name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .map_err(|e| format!("shpool: {e}"))?;
        Ok(status.success())
    }

    /// Starts the session name in dir, running the shell script job, in the background; it is killed when idle for ttl
    /// (none: never).
    pub fn start(
        &self,
        name: &str,
        dir: &str,
        job: &str,
        ttl: Option<&str>,
    ) -> Result<bool, String> {
        let script = format!("/bin/sh {}", crate::agent::sh_quote(job));
        let mut args = vec!["attach", "-b"];
        if let Some(ttl) = ttl {
            args.extend(["--ttl", ttl]);
        }
        args.extend(["-d", dir, "-c", script.as_str(), name]);
        let status = self
            .command(&args)
            .status()
            .map_err(|e| format!("shpool: {e}"))?;
        Ok(status.success())
    }

    /// Replaces this process with an attached shpool client; only returns the error if that fails.
    pub fn attach(&self, name: &str) -> String {
        let err = self.command(&["attach", name]).exec();
        format!("shpool: {err}")
    }
}
