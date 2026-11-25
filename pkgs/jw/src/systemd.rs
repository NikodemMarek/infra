use std::process::Command;

pub struct Systemd;

impl Systemd {
    /// Whether the user unit is active; None if systemd does not say (unknown unit, no user manager).
    pub fn is_active(&self, unit: &str) -> Option<bool> {
        let out = Command::new("systemctl")
            .args(["--user", "is-active", unit])
            .output()
            .ok()?;
        match String::from_utf8_lossy(&out.stdout).trim() {
            "active" | "activating" | "deactivating" | "reloading" => Some(true),
            "inactive" | "failed" => Some(false),
            _ => None,
        }
    }

    /// Starts argv as the transient user unit `unit` in workdir, with the given unit properties.
    pub fn run(
        &self,
        unit: &str,
        workdir: &str,
        properties: &[&str],
        argv: &[String],
    ) -> Result<(), String> {
        let mut cmd = Self::command(unit);
        cmd.args(["--working-directory", workdir]);
        for &property in properties {
            cmd.args(["--property", property]);
        }
        cmd.arg("--").args(argv);
        Self::exec(cmd)
    }

    /// Starts argv as the transient user unit `unit` once delay_secs have passed.
    pub fn run_later(&self, unit: &str, delay_secs: u64, argv: &[String]) -> Result<(), String> {
        let mut cmd = Self::command(unit);
        cmd.arg(format!("--on-active={delay_secs}"));
        cmd.arg("--").args(argv);
        Self::exec(cmd)
    }

    fn command(unit: &str) -> Command {
        let mut cmd = Command::new("systemd-run");
        cmd.args(["--user", "--collect", "--quiet", "--unit", unit]);
        cmd
    }

    fn exec(mut cmd: Command) -> Result<(), String> {
        let out = cmd
            .output()
            .map_err(|e| format!("cannot run systemd-run: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        let last = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty());
        Err(match (last, out.status.code()) {
            (Some(line), _) => line.to_string(),
            (None, Some(code)) => format!("systemd-run exited with {code}"),
            (None, None) => "systemd-run killed by a signal".to_string(),
        })
    }
}
