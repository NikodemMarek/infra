use std::collections::BTreeSet;
use std::fmt::Write;

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::store::{Result, Store};
use crate::{parse_date, with_token, without_token, Est, Task, Unit, Week};

#[derive(Debug, Subcommand)]
pub enum Request {
    /// Add a task to tasks.md (text without the checkbox)
    Add(AddArgs),
    /// Mark tasks done
    Done(DoneArgs),
    /// List tasks
    List(ListArgs),
    /// Summarize a week (default current)
    Week(WeekArgs),
    /// Plan tasks for a week (sets @week)
    Take(TakeArgs),
    /// Return a finished week's open tasks to the backlog with @drop (default latest finished week)
    Close(WeekArgs),
    /// Check tasks.md for format problems
    Lint(LintArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    /// One line: the description with #tags and @tokens
    #[arg(allow_hyphen_values = true)]
    pub text: String,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoneArgs {
    /// Line number or a substring of an open task's description
    #[arg(required = true, value_name = "REF")]
    pub refs: Vec<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    /// Case-insensitive substring of the description
    pub query: Option<String>,
    /// Scope: backlog (no @week) or a week (current, next, YYYY-wNN)
    #[arg(long = "in")]
    #[serde(rename = "in")]
    pub scope: Option<String>,
    /// Only open tasks
    #[arg(long, conflicts_with = "done")]
    #[serde(default)]
    pub open: bool,
    /// Only done tasks
    #[arg(long)]
    #[serde(default)]
    pub done: bool,
    /// Tags (also match sub-tags); with several, all must match
    #[arg(long = "tag", value_name = "TAG")]
    #[serde(default)]
    pub tags: Vec<String>,
    /// Exact @proj value
    #[arg(long)]
    pub proj: Option<String>,
    /// Only tasks with a valid @due on or before YYYY-MM-DD
    #[arg(long, value_name = "DATE")]
    pub due_before: Option<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WeekArgs {
    /// current, next or YYYY-wNN
    pub week: Option<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TakeArgs {
    /// Line number or a substring of an open task's description
    #[arg(required = true, value_name = "REF")]
    pub refs: Vec<String>,
    /// Target week: current, next or YYYY-wNN (default current, next on Sunday)
    #[arg(long)]
    pub week: Option<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
        let number = self.file.append(&line);
        self.save()?;
        out!(out, "{number}: {line}");
        Ok(())
    }

    fn done(&mut self, refs: &[String], out: &mut String) -> Result<()> {
        if refs.is_empty() {
            return Err("refs must not be empty".to_string());
        }
        let found = self.resolve_all(refs)?;
        let today = self.today.to_string();
        for &l in &found {
            let checked = format!("- [x]{}", &self.file.text(l)[5..]);
            self.file.set(l, &with_token(&checked, "done", &today));
        }
        self.save()?;
        self.print_lines(&found, out);
        Ok(())
    }

    fn print_lines(&self, lines: &[usize], out: &mut String) {
        for &l in lines {
            out!(out, "{}: {}", l + 1, self.file.text(l));
        }
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
        // None lists everything, Some(None) the backlog, Some(Some(week)) one week.
        let scope_week = match scope.as_deref() {
            None => None,
            Some("backlog") => Some(None),
            Some(spec) => Some(Some(self.week_spec(spec)?)),
        };
        let query = query.map(|q| q.to_lowercase());
        let keep = |t: &Task| {
            let has_tag = |tag: &String| {
                let sub = format!("{tag}/");
                t.tags.iter().any(|g| g == tag || g.starts_with(&sub))
            };
            scope_week.is_none_or(|w| w == t.week)
                && query
                    .as_ref()
                    .is_none_or(|q| t.description.to_lowercase().contains(q))
                && !(open && t.done)
                && !(done && !t.done)
                && tags.iter().all(has_tag)
                && (proj.is_none() || t.proj == proj)
                && before.is_none_or(|b| t.due.is_some_and(|d| d <= b))
        };

        let rows: Vec<(usize, &str, Task)> = self
            .tasks()
            .into_iter()
            .filter(|(_, t)| keep(t))
            .map(|(l, task)| (l + 1, self.file.text(l), task))
            .collect();

        if json {
            let items: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|(line, text, task)| {
                    json!({
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
                        "week": task.week.map(|w| w.to_string()),
                        "proj": task.proj,
                        "step": task.step,
                        "text": text,
                    })
                })
                .collect();
            out!(out, "{}", json!(items));
        } else {
            for (line, text, _) in rows {
                out!(out, "{line}: {text}");
            }
        }
        Ok(())
    }

    fn week(&mut self, week: Option<String>, json: bool, out: &mut String) -> Result<()> {
        let week = self.week_spec(week.as_deref().unwrap_or("current"))?;
        let (monday, end) = (week.monday(), week.sunday());
        let tasks = self.tasks();
        let s = stats(
            tasks
                .iter()
                .map(|(_, t)| t)
                .filter(|t| t.week == Some(week)),
        );
        let (past_weeks, past_avg) = past_average(&tasks, self.today);
        let status = if end < self.today {
            "past"
        } else if monday <= self.today {
            "current"
        } else {
            "future"
        };
        let id = week.to_string();

        if json {
            out!(
                out,
                "{}",
                json!({
                    "week": id,
                    "start": monday.to_string(),
                    "end": end.to_string(),
                    "status": status,
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
            out!(out, "{id} ({monday}..{end}, {status})");
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

    fn take(&mut self, refs: &[String], week: Option<String>, out: &mut String) -> Result<()> {
        if refs.is_empty() {
            return Err("refs must not be empty".to_string());
        }
        let week = match week {
            Some(spec) => self.week_spec(&spec)?,
            None if self.today.weekday() == Weekday::Sun => {
                Week::of(self.today + Duration::days(7))
            }
            None => Week::of(self.today),
        };
        let found = self.resolve_all(refs)?;
        for &l in &found {
            if let Ok(Some(Task { est: Some(est), .. })) = self.file.task(l) {
                if too_big(&est) {
                    return Err(format!(
                        "too big, split it first: line {} @est({est})",
                        l + 1
                    ));
                }
            }
        }

        let id = week.to_string();
        for &l in &found {
            let line = with_token(self.file.text(l), "week", &id);
            self.file.set(l, &line);
        }
        self.save()?;

        self.print_lines(&found, out);
        let tasks = self.tasks();
        let planned = stats(
            tasks
                .iter()
                .map(|(_, t)| t)
                .filter(|t| t.week == Some(week)),
        )
        .planned;
        let (_, average) = past_average(&tasks, self.today);
        out!(
            out,
            "{id}: planned {}h, past average done {}",
            hours(planned),
            average_text(average)
        );
        Ok(())
    }

    fn close(&mut self, week: Option<String>, out: &mut String) -> Result<()> {
        let week = match week {
            Some(spec) => self.week_spec(&spec)?,
            None if self.today.weekday() == Weekday::Sun => Week::of(self.today),
            None => Week::of(self.today - Duration::days(7)),
        };
        let end = week.sunday();
        let id = week.to_string();
        if end > self.today {
            return Err(format!("week {id} is not finished (ends {end})"));
        }

        let open: Vec<usize> = self
            .tasks()
            .into_iter()
            .filter(|(_, t)| !t.done && t.week == Some(week))
            .map(|(l, _)| l)
            .collect();
        if open.is_empty() {
            out!(out, "nothing to close in {id}");
            return Ok(());
        }
        for &l in &open {
            let line = with_token(&without_token(self.file.text(l), "week"), "drop", &id);
            self.file.set(l, &line);
        }
        self.save()?;
        self.print_lines(&open, out);
        Ok(())
    }

    /// Returns whether any line has problems.
    fn lint(&mut self, out: &mut String) -> Result<bool> {
        let mut problems = false;
        for l in 0..self.file.lines.len() {
            if let Err(e) = self.file.task(l) {
                out!(out, "{}: {e}", l + 1);
                problems = true;
            }
        }
        Ok(problems)
    }
}

/// The last 4 weeks that ended before today and appear as `@week` or `@drop`
/// of a task, ascending, and their average done hours.
fn past_average(tasks: &[(usize, Task)], today: NaiveDate) -> (Vec<String>, Option<f64>) {
    let ended: BTreeSet<Week> = tasks
        .iter()
        .flat_map(|(_, t)| [t.week, t.drop])
        .flatten()
        .filter(|w| w.sunday() < today)
        .collect();
    let last: Vec<Week> = ended.into_iter().rev().take(4).rev().collect();
    let done_hours: f64 = last
        .iter()
        .map(|w| {
            let tasks = tasks.iter().map(|(_, t)| t);
            stats(tasks.filter(|t| t.week == Some(*w))).done_hours
        })
        .sum();
    let average = (!last.is_empty()).then(|| done_hours / last.len() as f64);
    (last.iter().map(Week::to_string).collect(), average)
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

fn stats<'a>(tasks: impl Iterator<Item = &'a Task>) -> Stats {
    let mut s = Stats::default();
    for task in tasks {
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
