use std::collections::HashMap;
use std::env;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::cmd::{run, Output, Result};
use crate::jj::{re, Jj};

const STACK_START: &str = "<!-- jj-stack:start -->";
const STACK_END: &str = "<!-- jj-stack:end -->";

#[derive(Clone, Deserialize)]
pub struct Mr {
    pub number: u64,
    #[serde(rename = "ref")]
    pub reference: String,
    pub url: String,
    pub state: String,
    pub title: String,
    pub body: String,
    pub base: String,
    pub head: String,
    pub head_sha: Option<String>,
    pub landed: Vec<String>,
}

#[derive(Deserialize)]
struct Repo {
    forge: String,
}

#[derive(Deserialize)]
struct Upload {
    url: String,
}

fn decode<T: DeserializeOwned>(out: &str) -> Result<T> {
    serde_json::from_str(out).map_err(|e| format!("`forge` returned an unexpected response: {e}"))
}

pub struct Forge {
    remote: String,
    recent: Option<Vec<Mr>>,
}

impl Forge {
    pub fn new(remote: &str) -> Forge {
        Forge {
            remote: remote.to_string(),
            recent: None,
        }
    }

    fn run(&self, args: &[&str], check: bool) -> Result<Output> {
        let head = ["forge", "--remote", &self.remote];
        run(&[&head[..], args].concat(), check)
    }

    fn json<T: DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        decode(&self.run(args, true)?.stdout)
    }

    pub fn term(&self) -> Result<&'static str> {
        let repo: Repo = self.json(&["repo"])?;
        Ok(if repo.forge == "github" { "PR" } else { "MR" })
    }

    pub fn find(&self, branch: &str) -> Result<Option<Mr>> {
        let mrs: Vec<Mr> = self.json(&["pr", "list", "--head", branch, "--limit", "1"])?;
        Ok(mrs.into_iter().next())
    }

    pub fn recent(&mut self) -> Result<&[Mr]> {
        if self.recent.is_none() {
            self.recent = Some(self.json(&["pr", "list"])?);
        }
        Ok(self.recent.as_deref().unwrap_or_default())
    }

    pub fn create(
        &self,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<Mr> {
        let mut args = vec![
            "pr", "create", "--head", head, "--base", base, "--title", title, "--body", body,
        ];
        if draft {
            args.push("--draft");
        }
        self.json(&args)
    }

    pub fn update(
        &self,
        mr: &Mr,
        title: Option<&str>,
        base: Option<&str>,
        body: Option<&str>,
    ) -> Result<Mr> {
        if title.is_none() && base.is_none() && body.is_none() {
            return Ok(mr.clone());
        }
        let number = mr.number.to_string();
        let mut args = vec!["pr", "edit", number.as_str()];
        for (flag, value) in [("--title", title), ("--base", base), ("--body", body)] {
            if let Some(value) = value {
                args.extend([flag, value]);
            }
        }
        self.json(&args)
    }

    pub fn close(&self, mr: &Mr) -> Result<()> {
        self.run(&["pr", "close", &mr.number.to_string()], true)?;
        Ok(())
    }

    pub fn reopen(&self, mr: &Mr) -> Result<Option<Mr>> {
        let p = self.run(&["pr", "reopen", &mr.number.to_string()], false)?;
        if !p.ok {
            eprintln!(
                "  cannot reopen {}: {}; opening a new one",
                mr.reference,
                p.stderr.trim()
            );
            return Ok(None);
        }
        decode(&p.stdout).map(Some)
    }

    pub fn upload(&self, path: &str) -> Result<Option<String>> {
        let p = self.run(&["upload", path], false)?;
        match p.ok.then(|| decode::<Upload>(&p.stdout).ok()).flatten() {
            Some(upload) => Ok(Some(upload.url)),
            None => {
                let reason = match p.stderr.trim() {
                    "" => "invalid response",
                    stderr => stderr,
                };
                eprintln!("  {path}: upload failed: {reason}; link left as is");
                Ok(None)
            }
        }
    }
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

pub fn upload_media(text: &str, fg: &Forge) -> Result<String> {
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
