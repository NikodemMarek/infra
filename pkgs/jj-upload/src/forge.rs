use std::collections::HashMap;
use std::env;
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::cmd::{run, Output, Result};
use crate::jj::{forge_target, re, repr, Jj};

mod github;
mod gitlab;

use github::GitHub;
use gitlab::GitLab;

const STACK_START: &str = "<!-- jj-stack:start -->";
const STACK_END: &str = "<!-- jj-stack:end -->";

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
    let kind = match Jj::config("upload.forge")? {
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
    fn run(&self, args: &[&str], check: bool) -> Result<Output> {
        let head = [self.bin, "api", "--hostname", &self.host];
        run(&[&head[..], args].concat(), check)
    }

    fn decode<T: DeserializeOwned>(&self, out: &str) -> Result<Option<T>> {
        if out.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(out)
            .map_err(|e| format!("`{}` returned an unexpected response: {e}", self.bin))
    }

    fn one<T: DeserializeOwned>(&self, args: &[&str]) -> Result<Option<T>> {
        self.decode(&self.run(args, true)?.stdout)
    }

    fn many<T: DeserializeOwned>(&self, args: &[&str]) -> Result<Vec<T>> {
        Ok(self.one::<Vec<T>>(args)?.unwrap_or_default())
    }

    fn required<T: DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        self.one(args)?
            .ok_or_else(|| format!("`{}` returned no merge request", self.bin))
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
    if re(r"^[a-zA-Z][a-zA-Z0-9+.-]*:").is_match(target)
        || ["//", "#", "/uploads/"]
            .iter()
            .any(|p| target.starts_with(p))
    {
        return None;
    }
    let path = Path::new(root).join(expanduser(target));
    Some(path.to_string_lossy().into_owned())
}

fn tracked(path: &str, root: &str) -> Result<bool> {
    if !Path::new(path).starts_with(root) {
        return Ok(false);
    }
    Jj::file_tracked(path)
}

pub fn upload_media(text: &str, fg: &dyn Forge) -> Result<String> {
    if !text.contains("![") {
        return Ok(text.to_string());
    }
    let root = Jj::root()?;
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
                    let url = if !Path::new(&path).is_file() {
                        eprintln!("  {}: file not found; link left as is", target.as_str());
                        None
                    } else if tracked(&path, &root)? {
                        None
                    } else {
                        fg.upload(&path)?
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
