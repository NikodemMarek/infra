use std::fmt::Write;

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use clap::{Args, Subcommand};
use serde_json::json;

use crate::store::{monday_of, week_file, week_id, Result, Store};
use crate::{parse_date, with_token, Est, Task, Unit};

#[derive(Debug, Subcommand)]
pub enum Request {
    /// Add a task to backlog.md (text without the checkbox)
    Add(AddArgs),
    /// Mark tasks done
    Done(DoneArgs),
    /// List tasks
    List(ListArgs),
    /// Summarize a week (default current)
    Week(WeekArgs),
    /// Move backlog tasks into a week
    Take(TakeArgs),
    /// Copy a finished week's open tasks back to backlog.md (default latest finished week)
    Close(WeekArgs),
    /// Check backlog.md and the week files for format problems
    Lint(LintArgs),
}

#[derive(Debug, Args)]
pub struct AddArgs {
    #[arg(allow_hyphen_values = true)]
    pub text: String,
}

#[derive(Debug, Args)]
pub struct DoneArgs {
    /// file:line or a substring of the description
    #[arg(required = true, value_name = "REF")]
    pub refs: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Case-insensitive substring of the description
    pub query: Option<String>,
    /// Scope: backlog or a week (current, next, YYYY-wNN)
    #[arg(long = "in")]
    pub scope: Option<String>,
    /// Only open tasks
    #[arg(long, conflicts_with = "done")]
    pub open: bool,
    /// Only done tasks
    #[arg(long)]
    pub done: bool,
    /// Tag (also matches sub-tags); repeat to require several
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
    /// Exact @proj value
    #[arg(long)]
    pub proj: Option<String>,
    /// Only tasks with a valid @due on or before YYYY-MM-DD
    #[arg(long, value_name = "DATE")]
    pub due_before: Option<String>,
}

#[derive(Debug, Args)]
pub struct WeekArgs {
    /// current, next or YYYY-wNN
    pub week: Option<String>,
}

#[derive(Debug, Args)]
pub struct TakeArgs {
    /// file:line or a substring of the description
    #[arg(required = true, value_name = "REF")]
    pub refs: Vec<String>,
    /// Target week: current, next or YYYY-wNN
    #[arg(long)]
    pub week: Option<String>,
}

#[derive(Debug, Args)]
pub struct LintArgs {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Exactly what the CLI writes to stdout, trailing newline included.
    pub stdout: String,
    /// Lines without the `tm: ` prefix: skip warnings first, then the error (if any).
    pub stderr: Vec<String>,
    /// The CLI exits with 1.
    pub failed: bool,
}

/// `println!` into the output buffer.
macro_rules! out {
    ($out:expr, $($arg:tt)*) => {
        let _ = writeln!($out, $($arg)*);
    };
}

/// Reads TM_DIR and TM_TODAY on every call. Never prints, panics or exits.
pub fn execute(request: Request, json: bool) -> Output {
    let mut stdout = String::new();
    let mut store = match Store::open() {
        Ok(store) => store,
        Err(e) => {
            return Output {
                stdout,
                stderr: vec![e],
                failed: true,
            }
        }
    };
    let result = run(&mut store, request, json, &mut stdout);
    let mut stderr = store.warnings;
    match result {
        Ok(failed) => Output {
            stdout,
            stderr,
            failed,
        },
        Err(e) => {
            stderr.push(e);
            Output {
                stdout: String::new(),
                stderr,
                failed: true,
            }
        }
    }
}

/// The checks every command does first: the tasks dir exists and TM_TODAY is valid.
pub fn check_env() -> std::result::Result<(), String> {
    Store::open().map(|_| ())
}

/// Returns whether the command failed without an error message (lint found problems).
fn run(store: &mut Store, request: Request, json: bool, out: &mut String) -> Result<bool> {
    match request {
        Request::Add(args) => store.add(&args.text, out)?,
        Request::Done(args) => store.done(&args.refs, out)?,
        Request::List(args) => store.list(args, json, out)?,
        Request::Week(args) => store.week(args.week, json, out)?,
        Request::Take(args) => store.take(&args.refs, args.week, out)?,
        Request::Close(args) => store.close(args.week, out)?,
        Request::Lint(LintArgs {}) => return store.lint(out),
    }
    Ok(false)
}

impl Store {
    fn add(&mut self, text: &str, out: &mut String) -> Result<()> {
        if text.trim().is_empty() {
            return Err("the text must not be empty".to_string());
        }
        if text.contains(['\n', '\r']) {
            return Err("text must be a single line".to_string());
        }
        if text.starts_with("- [") {
            return Err("give the text without the checkbox".to_string());
        }
        let line = self.new_task(&format!("- [ ] {text}"))?;
        let number = self.append_to("backlog.md", &line)?;
        out!(out, "backlog.md:{number}");
        Ok(())
    }

    fn done(&mut self, refs: &[String], out: &mut String) -> Result<()> {
        if refs.is_empty() {
            return Err("refs must not be empty".to_string());
        }
        let current = week_file(monday_of(self.today));
        let next = week_file(monday_of(self.today) + Duration::days(7));
        let scope = [current.as_str(), next.as_str(), "backlog.md"];
        let found = self.resolve_all(refs, &scope, true)?;
        let today = self.today.to_string();
        for &(f, l) in &found {
            let checked = format!("- [x]{}", &self.files[f].text(l)[5..]);
            self.files[f].set(l, &with_token(&checked, "done", &today));
        }
        self.save_all(found.iter().map(|&(f, _)| f))?;
        for &at in &found {
            out!(out, "{}", self.loc(at));
        }
        Ok(())
    }

    fn list(&mut self, args: ListArgs, json: bool, out: &mut String) -> Result<()> {
        let ListArgs {
            query,
            scope,
            open,
            done,
            tags,
            proj,
            due_before,
        } = args;
        if open && done {
            return Err("open and done cannot both be true".to_string());
        }
        let before = due_before
            .map(|s| parse_date(&s).ok_or_else(|| format!("invalid date: {s}")))
            .transpose()?;
        let names = match scope.as_deref() {
            None => self.task_files(),
            Some("backlog") => vec!["backlog.md".to_string()],
            Some(spec) => vec![week_file(self.week_spec(spec)?)],
        };
        let query = query.map(|q| q.to_lowercase());
        let keep = |t: &Task| {
            let has_tag = |tag: &String| {
                let sub = format!("{tag}/");
                t.tags.iter().any(|g| g == tag || g.starts_with(&sub))
            };
            query
                .as_ref()
                .is_none_or(|q| t.description.to_lowercase().contains(q))
                && !(open && t.done)
                && !(done && !t.done)
                && tags.iter().all(has_tag)
                && (proj.is_none() || t.proj == proj)
                && before.is_none_or(|b| t.due.is_some_and(|d| d <= b))
        };

        let mut rows = Vec::new();
        for name in names {
            let f = self.load(&name)?;
            let tasks = self.tasks(f);
            let file = &self.files[f];
            for (l, task) in tasks.into_iter().filter(|(_, t)| keep(t)) {
                rows.push((file.name.clone(), l + 1, file.text(l).to_string(), task));
            }
        }

        if json {
            let items: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|(file, line, text, task)| {
                    json!({
                        "file": file,
                        "line": line,
                        "done": task.done,
                        "description": task.description,
                        "tags": task.tags,
                        "est": task.est.map(|e| e.to_string()),
                        "est_hours": task.est_hours(),
                        "due": task.due.map(|d| d.to_string()),
                        "created": task.created.map(|d| d.to_string()),
                        "done_on": task.done_on.map(|d| d.to_string()),
                        "drop": task.drop.map(|w| w.to_string()),
                        "proj": task.proj,
                        "step": task.step,
                        "text": text,
                    })
                })
                .collect();
            out!(out, "{}", json!(items));
        } else {
            for (file, line, text, _) in rows {
                out!(out, "{file}:{line}: {text}");
            }
        }
        Ok(())
    }

    fn week(&mut self, week: Option<String>, json: bool, out: &mut String) -> Result<()> {
        let monday = self.week_spec(week.as_deref().unwrap_or("current"))?;
        let end = monday + Duration::days(6);
        let name = week_file(monday);
        let exists = self.dir.join(&name).exists();
        let f = self.load(&name)?;
        let s = stats(&self.tasks(f));
        let (past_weeks, past_avg) = self.past_weeks()?;
        let status = if end < self.today {
            "past"
        } else if monday <= self.today {
            "current"
        } else {
            "future"
        };
        let id = week_id(monday);

        if json {
            out!(
                out,
                "{}",
                json!({
                    "week": id,
                    "start": monday.to_string(),
                    "end": end.to_string(),
                    "status": status,
                    "file": name,
                    "exists": exists,
                    "tasks": s.tasks,
                    "done": s.done,
                    "open": s.open,
                    "planned_hours": s.planned,
                    "done_hours": s.done_hours,
                    "open_hours": s.open_hours,
                    "other_estimates": s.other,
                    "no_estimate": s.no_estimate,
                    "past_weeks": past_weeks,
                    "past_avg_done_hours": past_avg,
                })
            );
        } else {
            out!(out, "{id} ({monday}..{end}, {status}) {name}");
            out!(out, "tasks {}: {} done, {} open", s.tasks, s.done, s.open);
            out!(
                out,
                "hours: planned {}h, done {}h, open {}h",
                hours(s.planned),
                hours(s.done_hours),
                hours(s.open_hours)
            );
            out!(out, "no estimate: {}", s.no_estimate);
            if !s.other.is_empty() {
                out!(out, "other estimates: {}", s.other.join(", "));
            }
            out!(out, "past weeks: {}", past_weeks.join(", "));
            out!(out, "past average done: {}", average_text(past_avg));
        }
        Ok(())
    }

    /// The last 4 existing week files that ended before today, and their average done hours.
    fn past_weeks(&mut self) -> Result<(Vec<String>, Option<f64>)> {
        let today = self.today;
        let ended: Vec<NaiveDate> = self
            .existing_weeks()
            .into_iter()
            .filter(|&m| m + Duration::days(6) < today)
            .collect();
        let last = &ended[ended.len().saturating_sub(4)..];
        let mut done_hours = 0.0;
        for &monday in last {
            let f = self.load(&week_file(monday))?;
            done_hours += stats(&self.tasks(f)).done_hours;
        }
        let average = (!last.is_empty()).then(|| done_hours / last.len() as f64);
        Ok((last.iter().map(|&m| week_id(m)).collect(), average))
    }

    fn take(&mut self, refs: &[String], week: Option<String>, out: &mut String) -> Result<()> {
        if refs.is_empty() {
            return Err("refs must not be empty".to_string());
        }
        let monday = match week {
            Some(spec) => self.week_spec(&spec)?,
            None if self.today.weekday() == Weekday::Sun => {
                monday_of(self.today) + Duration::days(7)
            }
            None => monday_of(self.today),
        };
        let found = self.resolve_all(refs, &["backlog.md"], false)?;
        for &at in &found {
            if let Ok(Some(Task { est: Some(est), .. })) = self.files[at.0].task(at.1) {
                if too_big(&est) {
                    return Err(format!(
                        "too big, split it first: {} @est({est})",
                        self.loc(at)
                    ));
                }
            }
        }

        let name = week_file(monday);
        let dest = self.load(&name)?;
        let mut numbers = Vec::new();
        for &(f, l) in &found {
            let line = self.files[f].text(l).to_string();
            numbers.push(self.files[dest].append(&line));
        }
        let backlog = self.load("backlog.md")?;
        let mut removed: Vec<usize> = found.iter().map(|&(_, l)| l).collect();
        removed.sort_unstable_by(|a, b| b.cmp(a));
        for l in removed {
            self.files[backlog].lines.remove(l);
        }
        self.save(dest)?;
        self.save(backlog)?;

        for number in numbers {
            out!(out, "{name}:{number}");
        }
        let planned = stats(&self.tasks(dest)).planned;
        let (_, average) = self.past_weeks()?;
        out!(
            out,
            "{}: planned {}h, past average done {}",
            week_id(monday),
            hours(planned),
            average_text(average)
        );
        Ok(())
    }

    fn close(&mut self, week: Option<String>, out: &mut String) -> Result<()> {
        let monday = match week {
            Some(spec) => self.week_spec(&spec)?,
            None if self.today.weekday() == Weekday::Sun => monday_of(self.today),
            None => monday_of(self.today) - Duration::days(7),
        };
        let end = monday + Duration::days(6);
        let id = week_id(monday);
        if end > self.today {
            return Err(format!("week {id} is not finished (ends {end})"));
        }

        let name = week_file(monday);
        let week = self.load(&name)?;
        let backlog = self.load("backlog.md")?;
        let open: Vec<String> = self
            .tasks(week)
            .into_iter()
            .filter(|(_, t)| !t.done)
            .map(|(l, _)| with_token(self.files[week].text(l), "drop", &id))
            .collect();
        let mut copied = Vec::new();
        for line in open {
            if !self.files[backlog].contains(&line) {
                copied.push(self.files[backlog].append(&line));
            }
        }
        if copied.is_empty() {
            out!(out, "nothing to close in {name}");
            return Ok(());
        }
        self.save(backlog)?;
        for number in copied {
            out!(out, "backlog.md:{number}");
        }
        Ok(())
    }

    /// Returns whether any line has problems.
    fn lint(&mut self, out: &mut String) -> Result<bool> {
        let mut problems = false;
        for name in self.task_files() {
            let f = self.load(&name)?;
            for l in 0..self.files[f].lines.len() {
                if let Err(e) = self.files[f].task(l) {
                    out!(out, "{}: {e}", self.loc((f, l)));
                    problems = true;
                }
            }
        }
        Ok(problems)
    }
}

#[derive(Default)]
struct Stats {
    tasks: usize,
    done: usize,
    open: usize,
    planned: f64,
    done_hours: f64,
    open_hours: f64,
    other: Vec<String>,
    no_estimate: usize,
}

fn stats(tasks: &[(usize, Task)]) -> Stats {
    let mut s = Stats::default();
    for (_, task) in tasks {
        let est = task.est_hours();
        let h = est.unwrap_or(0.0);
        s.tasks += 1;
        s.planned += h;
        if task.done {
            s.done += 1;
            s.done_hours += h;
        } else {
            s.open += 1;
            s.open_hours += h;
        }
        match task.est {
            None => s.no_estimate += 1,
            Some(est) if est.hours().is_none() => s.other.push(est.to_string()),
            Some(_) => {}
        }
    }
    s
}

fn too_big(est: &Est) -> bool {
    match est.unit {
        Unit::Hours => false,
        Unit::Days => est.amount > 1.0,
        Unit::Months | Unit::Years => true,
    }
}

fn hours(h: f64) -> String {
    format!("{}", (h * 100.0).round() / 100.0)
}

fn average_text(average: Option<f64>) -> String {
    average.map_or("n/a".to_string(), |a| format!("{}h", hours(a)))
}
