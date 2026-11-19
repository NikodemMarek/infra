use std::collections::HashMap;
use std::env;
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::jj::{forge_target, jj, jj_config, re, repr, run, Result};

mod github;
mod gitlab;

use github::GitHub;
use gitlab::GitLab;

const STACK_START: &str = "<!-- jj-stack:start -->";
const STACK_END: &str = "<!-- jj-stack:end -->";
const MEDIA_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".bmp", ".avif", ".mp4", ".webm", ".mov",
    ".mkv", ".m4v", ".ogv",
];

enum Field<'a> {
    Text(&'a str),
    Flag(bool),
}

#[derive(Clone)]
pub struct Mr {
    pub id: String,
    pub reference: String,
    pub url: String,
    pub state: String,
    pub draft: bool,
    pub title: String,
    pub target: String,
    pub source: String,
    pub description: String,
    pub head: Option<String>,
    pub landed: Vec<String>,
}

pub trait Forge {
    fn term(&self) -> &'static str;
    fn find(&self, branch: &str) -> Result<Option<Mr>>;
    fn recent(&mut self) -> Result<&[Mr]>;
    fn create(
        &self,
        source: &str,
        target: &str,
        title: &str,
        description: &str,
        draft: bool,
    ) -> Result<Mr>;
    fn update(
        &self,
        mr: &Mr,
        title: Option<&str>,
        target: Option<&str>,
        description: Option<&str>,
    ) -> Result<Mr>;
    fn set_state(&self, mr: &Mr, open: bool) -> Result<Option<Mr>>;
    fn upload(&self, path: &str) -> Result<Option<String>>;
}

pub fn make_forge(remote: &str) -> Result<Box<dyn Forge>> {
    let (host, project) = forge_target(remote)?;
    let kind = match jj_config("upload.forge")? {
        Some(kind) => kind,
        None => ["gitlab", "github"]
            .into_iter()
            .find(|n| host.to_lowercase().contains(n))
            .unwrap_or("gitlab")
            .to_string(),
    };
    match kind.as_str() {
        "gitlab" => Ok(Box::new(GitLab::new(host, &project))),
        "github" => Ok(Box::new(GitHub::new(host, &project))),
        _ => Err(format!(
            "unknown upload.forge {}; known: gitlab, github",
            repr(&kind)
        )),
    }
}

struct Api {
    bin: &'static str,
    host: String,
}

impl Api {
    fn full<'a>(&'a self, rest: &[&'a str]) -> Vec<&'a str> {
        let mut cmd = vec![self.bin, "api", "--hostname", self.host.as_str()];
        cmd.extend_from_slice(rest);
        cmd
    }

    fn output(&self, rest: &[&str]) -> Result<String> {
        Ok(run(&self.full(rest), true)?.stdout)
    }

    fn call(&self, method: &str, path: &str, fields: &[(&str, Field)]) -> Result<String> {
        let flags: Vec<String> = fields
            .iter()
            .flat_map(|(k, v)| match v {
                Field::Text(s) => ["-f".to_string(), format!("{k}={s}")],
                Field::Flag(b) => ["-F".to_string(), format!("{k}={b}")],
            })
            .collect();
        let mut rest = vec!["-X", method, path];
        rest.extend(flags.iter().map(String::as_str));
        self.output(&rest)
    }

    fn decode<T: DeserializeOwned>(&self, out: &str) -> Result<Option<T>> {
        if out.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(out)
            .map_err(|e| format!("`{}` returned an unexpected response: {e}", self.bin))
    }

    fn one<T: DeserializeOwned + Into<Mr>>(&self, out: &str) -> Result<Option<Mr>> {
        Ok(self.decode::<T>(out)?.map(Into::into))
    }

    fn many<T: DeserializeOwned + Into<Mr>>(&self, out: &str) -> Result<Vec<Mr>> {
        Ok(self
            .decode::<Vec<T>>(out)?
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect())
    }

    fn send<T: DeserializeOwned + Into<Mr>>(
        &self,
        method: &str,
        path: &str,
        fields: &[(&str, Field)],
    ) -> Result<Option<Mr>> {
        self.one::<T>(&self.call(method, path, fields)?)
    }

    fn required(&self, mr: Option<Mr>) -> Result<Mr> {
        mr.ok_or_else(|| format!("`{}` returned no merge request", self.bin))
    }
}

fn text_fields<'a>(keys: [&'a str; 3], values: [Option<&'a str>; 3]) -> Vec<(&'a str, Field<'a>)> {
    keys.into_iter()
        .zip(values)
        .filter_map(|(key, value)| value.map(|v| (key, Field::Text(v))))
        .collect()
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

fn quote_plus(s: &str) -> String {
    quote(s).replace("%20", "+")
}

pub fn stack_section(entries: &[(&str, &str)], current: &str, term: &str) -> String {
    let mut lines = vec![
        STACK_START.to_string(),
        String::new(),
        "**Stack**:".to_string(),
        String::new(),
    ];
    for (branch, reference) in entries {
        let mark = if *branch == current {
            format!(" 👈 this {term}")
        } else {
            String::new()
        };
        lines.push(format!("- {reference}{mark}"));
    }
    lines.push(String::new());
    lines.push(STACK_END.to_string());
    lines.join("\n")
}

pub fn strip_stack(text: &str) -> String {
    let pattern = format!(
        r"(?s)\n*{}.*?{}\n*",
        regex::escape(STACK_START),
        regex::escape(STACK_END)
    );
    re(&pattern).replace_all(text, "").trim().to_string()
}

fn expanduser(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    match env::var("HOME") {
        Ok(home) if rest.is_empty() || rest.starts_with('/') => {
            let expanded = format!("{}{rest}", home.trim_end_matches('/'));
            if expanded.is_empty() {
                "/".to_string()
            } else {
                expanded
            }
        }
        _ => path.to_string(),
    }
}

fn local_file(target: &str, root: &str) -> Option<String> {
    let lower = target.to_lowercase();
    if re(r"^[a-zA-Z][a-zA-Z0-9+.-]*:").is_match(target)
        || ["//", "#", "/uploads/"]
            .iter()
            .any(|p| target.starts_with(p))
        || !MEDIA_EXTENSIONS.iter().any(|e| lower.ends_with(e))
    {
        return None;
    }
    let path = Path::new(root).join(expanduser(target));
    Some(path.to_string_lossy().into_owned())
}

pub fn upload_media(text: &str, fg: &dyn Forge) -> Result<String> {
    if !text.contains("![") {
        return Ok(text.to_string());
    }
    let root = jj(&["root"])?.trim().to_string();
    let mut urls: HashMap<String, Option<String>> = HashMap::new();
    let link = re(r"(!\[[^\]]*\]\()([^)\s]+)");
    let mut out = String::new();
    let mut last = 0;
    for caps in link.captures_iter(text) {
        let (Some(whole), Some(prefix), Some(target)) = (caps.get(0), caps.get(1), caps.get(2))
        else {
            continue;
        };
        out.push_str(&text[last..whole.start()]);
        last = whole.end();
        let url = match local_file(target.as_str(), &root) {
            None => None,
            Some(path) => {
                if !urls.contains_key(&path) {
                    let url = if Path::new(&path).is_file() {
                        fg.upload(&path)?
                    } else {
                        eprintln!("  {}: file not found; link left as is", target.as_str());
                        None
                    };
                    urls.insert(path.clone(), url);
                }
                urls[&path].clone()
            }
        };
        match url {
            Some(url) => {
                out.push_str(prefix.as_str());
                out.push_str(&url);
            }
            None => out.push_str(whole.as_str()),
        }
    }
    out.push_str(&text[last..]);
    Ok(out)
}
