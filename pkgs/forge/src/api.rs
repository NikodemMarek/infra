use serde::de::DeserializeOwned;

use crate::cmd::{run, Result};
use crate::target::{Kind, Target};

pub struct Api {
    bin: &'static str,
    host: String,
}

impl Api {
    /// Fails without any `api` call when gh / glab has no credential for the host.
    pub fn connect(target: &Target) -> Result<Api> {
        let host = target.host.as_str();
        let ok = match target.kind {
            Kind::GitHub => {
                let out = run(&["gh", "auth", "token", "--hostname", host], false)?;
                out.ok && !out.stdout.trim().is_empty()
            }
            Kind::GitLab => run(&["glab", "auth", "status", "--hostname", host], false)?.ok,
        };
        if !ok {
            return Err(format!("no {} credential for {host}", target.kind.bin()));
        }
        Ok(Api {
            bin: target.kind.bin(),
            host: target.host.clone(),
        })
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let out = run(&[self.bin, "api", "--hostname", &self.host, path], true)?;
        serde_json::from_str(&out.stdout)
            .map_err(|e| format!("`{}` returned an unexpected response: {e}", self.bin))
    }
}

pub fn quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
