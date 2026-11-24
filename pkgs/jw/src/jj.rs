use std::process::{Command, Stdio};
use std::time::Duration;

use crate::cmd::output_timeout;

pub struct Jj {
    dir: Option<String>,
    repo: Option<String>,
}

impl Jj {
    pub fn here() -> Jj {
        Jj {
            dir: None,
            repo: None,
        }
    }

    pub fn in_dir(dir: &str) -> Jj {
        Jj {
            dir: Some(dir.to_string()),
            repo: None,
        }
    }

    /// jj -R path: the workspace at path, whatever the current directory is.
    pub fn repo(path: &str) -> Jj {
        Jj {
            dir: None,
            repo: Some(path.to_string()),
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new("jj");
        if let Some(dir) = &self.dir {
            cmd.current_dir(dir);
        }
        if let Some(repo) = &self.repo {
            cmd.args(["-R", repo]);
        }
        cmd
    }

    /// Runs jj with stdout and stderr captured; a failure carries the command and jj's stderr.
    pub fn query(&self, args: &[&str]) -> Result<String, String> {
        self.query_stderr(args).map(|(stdout, _)| stdout)
    }

    /// As query, with jj's stderr (its warnings) of a successful run too.
    pub fn query_stderr(&self, args: &[&str]) -> Result<(String, String), String> {
        let out = self
            .command()
            .args(["--no-pager", "--color=never"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("jj: {e}"))?;
        if !out.status.success() {
            let place = self.repo.as_deref().or(self.dir.as_deref()).unwrap_or(".");
            return Err(format!(
                "`jj {}` in {place}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok((
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    /// Runs jj with its messages and errors going straight to the terminal; a failure carries no message.
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = self
            .command()
            .arg("--no-pager")
            .args(args)
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| format!("jj: {e}"))?;
        if !out.status.success() {
            return Err(String::new());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn workspace_root(&self) -> Result<String, String> {
        let out = self
            .command()
            .args(["--ignore-working-copy", "workspace", "root"])
            .output()
            .map_err(|e| format!("jj: {e}"))?;
        if !out.status.success() {
            return Err("not in a jj repo".to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn fetch(&self) -> Result<(), String> {
        self.run(&["--ignore-working-copy", "git", "fetch"])
            .map(|_| ())
    }

    pub fn has_remotes(&self) -> Result<bool, String> {
        let out = self.query(&["--ignore-working-copy", "git", "remote", "list"])?;
        Ok(!out.trim().is_empty())
    }

    pub fn git_init(&self) -> Result<(), String> {
        self.run(&["git", "init"]).map(|_| ())
    }

    /// Makes bookmark main exist (on the working-copy commit if missing) and trunk() resolve to it, so workspaces of
    /// a repo without a remote start on main; safe to repeat after a partial run.
    pub fn ensure_main(&self) -> Result<(), String> {
        let existing = self.query(&["--ignore-working-copy", "bookmark", "list", "main"])?;
        if existing.trim().is_empty() {
            self.run(&["bookmark", "create", "main", "-r", "@"])?;
        }
        self.run(&[
            "config",
            "set",
            "--repo",
            r#"revset-aliases."trunk()""#,
            "main",
        ])
        .map(|_| ())
    }

    pub fn git_clone(&self, url: &str, dir: &str) -> Result<(), String> {
        self.run(&["git", "clone", "--colocate", url, dir])
            .map(|_| ())
    }

    /// The commit ids of revision, "root" for the root commit.
    pub fn commits(&self, revision: &str, snapshot: bool) -> Result<Vec<String>, String> {
        let mut args = vec![];
        if !snapshot {
            args.push("--ignore-working-copy");
        }
        args.extend([
            "log",
            "--no-graph",
            "-r",
            revision,
            "-T",
            r#"if(root, "root", commit_id) ++ "\n""#,
        ]);
        let out = self.run(&args)?;
        Ok(out.split_whitespace().map(str::to_string).collect())
    }

    pub fn workspace_add(&self, name: &str, commit: &str, dir: &str) -> Result<(), String> {
        self.run(&["workspace", "add", "--name", name, "-r", commit, dir])
            .map(|_| ())
    }

    pub fn workspace_names(&self) -> Result<Vec<String>, String> {
        let out = self.run(&[
            "--ignore-working-copy",
            "workspace",
            "list",
            "-T",
            r#"name ++ "\n""#,
        ])?;
        Ok(out.split_whitespace().map(str::to_string).collect())
    }

    /// Forgets the workspace name of this repo, the workspace of the directory without one.
    pub fn workspace_forget(&self, name: Option<&str>) -> Result<(), String> {
        let mut args = vec!["--ignore-working-copy", "workspace", "forget"];
        args.extend(name);
        self.run(&args).map(|_| ())
    }

    /// Snapshots the working copy.
    pub fn status(&self) -> Result<(), String> {
        self.run(&["status"]).map(|_| ())
    }

    pub fn file_list(&self) -> Result<Vec<String>, String> {
        let out = self.run(&["--ignore-working-copy", "file", "list"])?;
        Ok(out.lines().map(str::to_string).collect())
    }

    pub fn commit_id(&self) -> Result<String, String> {
        self.run(&[
            "--ignore-working-copy",
            "log",
            "--no-graph",
            "-r",
            "@",
            "-T",
            "commit_id",
        ])
    }

    /// Visible work that no workspace's or pushed history includes: change id and first line per head.
    pub fn heads_outside(&self) -> Result<Vec<String>, String> {
        let out = self.run(&[
            "--ignore-working-copy",
            "log",
            "--no-graph",
            "-r",
            "heads(trunk().. ~ ::(working_copies() | remote_bookmarks()))",
            "-T",
            r#"change_id.shortest() ++ " " ++ description.first_line() ++ "\n""#,
        ])?;
        Ok(out.lines().map(str::to_string).collect())
    }

    pub fn loads(&self) -> bool {
        self.command()
            .args([
                "--no-pager",
                "--ignore-working-copy",
                "log",
                "--no-graph",
                "-T",
                "",
                "-r",
                "@",
            ])
            .output()
            .is_ok_and(|out| out.status.success())
    }

    /// Whether the stack is merged; None if that couldn't be checked.
    pub fn stack_merged(&self) -> Option<bool> {
        let mut cmd = self.command();
        cmd.args(["stack", "status"]);
        let out = output_timeout(&mut cmd, None, Duration::from_secs(300)).ok()??;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let last = stdout.lines().map(str::trim).rfind(|l| !l.is_empty());
        match out.status.code() {
            Some(1) => Some(false),
            // an old `jj stack` (a `jj log` alias) exits 0 without the summary line
            Some(0) if last == Some("merged") => Some(true),
            _ => None,
        }
    }

    /// The stack to trunk() with @, one `change<TAB>y|n<TAB>name=root...` row per commit; None if jj fails.
    pub fn stack_rows(&self) -> Option<String> {
        let out = self
            .command()
            .args([
                "--no-pager",
                "--ignore-working-copy",
                "--color=never",
                "log",
                "--no-graph",
                "-r",
                "(trunk()..(@::) ~ ((working_copies() ~ @) & empty())) | @",
                "-T",
                // same revset as `jj log` in the starship prompt
                r#"if(current_working_copy, change_id.shortest(), "-") ++ if(self.contained_in("trunk()..@"), "\ty", "\tn") ++ if(current_working_copy, "\t" ++ working_copies.map(|w| w.name() ++ "=" ++ w.root()).join("\t")) ++ "\n""#,
            ])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}
