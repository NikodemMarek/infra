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
}
