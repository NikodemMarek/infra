use std::collections::BTreeSet;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::{env, fs, process};

use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};
use clap::error::ErrorKind as UsageError;
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::json;
use tm::{parse_date, parse_line, week_start, with_token, Est, ParseError, Task, Unit};

/// `println!` that exits quietly when stdout is closed (`tm list | head -1`).
macro_rules! out {
    ($($arg:tt)*) => {
        if let Err(e) = writeln!(std::io::stdout(), $($arg)*) {
            if e.kind() == ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("tm: {e}");
            process::exit(1);
        }
    };
}

/// Manage the markdown task lists (backlog and ISO-week files) in $TM_DIR.
#[derive(Parser)]
#[command(name = "tm", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Add a task to backlog.md (text without the checkbox)
    Add {
        #[arg(required = true, allow_hyphen_values = true)]
        text: Vec<String>,
    },
    /// Mark tasks done
    Done {
        /// file:line or a substring of the description
        #[arg(required = true)]
        refs: Vec<String>,
    },
    /// List tasks
    List {
        /// Case-insensitive substring of the description
        query: Option<String>,
        /// Scope: backlog or a week (current, next, YYYY-wNN)
        #[arg(long = "in")]
        scope: Option<String>,
        /// Only open tasks
        #[arg(long, conflicts_with = "done")]
        open: bool,
        /// Only done tasks
        #[arg(long)]
        done: bool,
        /// Tag (also matches sub-tags); repeat to require several
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Exact @proj value
        #[arg(long)]
        proj: Option<String>,
        /// Only tasks with a valid @due on or before YYYY-MM-DD
        #[arg(long)]
        due_before: Option<String>,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
    /// Summarize a week
    Week {
        /// current, next or YYYY-wNN (default current)
        weekspec: Option<String>,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
    /// Move backlog tasks into a week
    Take {
        /// file:line or a substring of the description
        #[arg(required = true)]
        refs: Vec<String>,
        /// Target week: current, next or YYYY-wNN
        #[arg(long)]
        week: Option<String>,
    },
    /// Copy a finished week's open tasks back to backlog.md
    Close {
        /// current, next or YYYY-wNN (default: latest finished week)
        weekspec: Option<String>,
    },
    /// Check backlog.md and the week files for format problems
    Lint,
}

type Result<T> = std::result::Result<T, String>;

fn main() {
    if let Err(e) = run(Cli::parse().command) {
        eprintln!("tm: {e}");
        process::exit(1);
    }
}

fn run(command: Command) -> Result<()> {
    let mut store = Store::open()?;
    match command {
        Command::Add { text } => store.add(text),
        Command::Done { refs } => store.done(&refs),
        Command::List {
            query,
            scope,
            open,
            done,
            tags,
            proj,
            due_before,
            json,
        } => store.list(query, scope, open, done, tags, proj, due_before, json),
        Command::Week { weekspec, json } => store.week(weekspec, json),
        Command::Take { refs, week } => store.take(&refs, week),
        Command::Close { weekspec } => store.close(weekspec),
        Command::Lint => store.lint(),
    }
}

/// A task file as text lines, each keeping its own `\n`, so writing the
/// lines back is byte-identical.
struct TaskFile {
    name: String,
    lines: Vec<String>,
}

impl TaskFile {
    fn text(&self, i: usize) -> &str {
        self.lines[i].strip_suffix('\n').unwrap_or(&self.lines[i])
    }

    fn task(&self, i: usize) -> std::result::Result<Option<Task>, ParseError> {
        if i >= self.lines.len() {
            return Ok(None);
        }
        parse_line(self.text(i))
    }

    fn contains(&self, line: &str) -> bool {
        (0..self.lines.len()).any(|i| self.text(i) == line)
    }

    fn set(&mut self, i: usize, line: &str) {
        let newline = if self.lines[i].ends_with('\n') {
            "\n"
        } else {
            ""
        };
        self.lines[i] = format!("{line}{newline}");
    }

    /// Returns the 1-based number of the new line.
    fn append(&mut self, line: &str) -> usize {
        if let Some(last) = self.lines.last_mut() {
            if !last.ends_with('\n') {
                last.push('\n');
            }
        }
        self.lines.push(format!("{line}\n"));
        self.lines.len()
    }
}

struct Store {
    dir: PathBuf,
    today: NaiveDate,
    files: Vec<TaskFile>,
    /// Invalid lines already warned about, as (file index, line index).
    skipped: BTreeSet<(usize, usize)>,
}

impl Store {
    fn open() -> Result<Store> {
        let dir = match env::var_os("TM_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => {
                PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?).join("omniverse/tasks")
            }
        };
        if !dir.is_dir() {
            return Err(format!("tasks dir not found: {}", dir.display()));
        }
        let today = match env::var("TM_TODAY") {
            Ok(s) => parse_date(&s).ok_or_else(|| format!("invalid TM_TODAY: {s}"))?,
            Err(_) => Local::now().date_naive(),
        };
        Ok(Store {
            dir,
            today,
            files: Vec::new(),
            skipped: BTreeSet::new(),
        })
    }

    /// Index of the file in `files`, reading it on first use; a missing file is empty.
    fn load(&mut self, name: &str) -> Result<usize> {
        if let Some(i) = self.files.iter().position(|f| f.name == name) {
            return Ok(i);
        }
        let path = self.dir.join(name);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io_error(&path, e)),
        };
        self.files.push(TaskFile {
            name: name.to_string(),
            lines: text.split_inclusive('\n').map(String::from).collect(),
        });
        Ok(self.files.len() - 1)
    }

    /// Writes the file to a temp file in its directory, then renames it over the original.
    fn save(&self, i: usize) -> Result<()> {
        let file = &self.files[i];
        let path = self.dir.join(&file.name);
        let parent = path.parent().unwrap_or(&self.dir);
        let base = path.file_name().unwrap_or_default().to_string_lossy();
        let tmp = parent.join(format!(".{base}.tm-tmp"));
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        fs::write(&tmp, file.lines.concat()).map_err(|e| io_error(&tmp, e))?;
        fs::rename(&tmp, &path).map_err(|e| io_error(&path, e))
    }

    fn save_all(&self, files: impl IntoIterator<Item = usize>) -> Result<()> {
        let files: BTreeSet<usize> = files.into_iter().collect();
        files.into_iter().try_for_each(|i| self.save(i))
    }

    fn loc(&self, (f, l): (usize, usize)) -> String {
        format!("{}:{}", self.files[f].name, l + 1)
    }

    /// The tasks of a loaded file with their line indexes. An invalid line is
    /// skipped, with a warning the first time it is seen.
    fn tasks(&mut self, f: usize) -> Vec<(usize, Task)> {
        let mut tasks = Vec::new();
        for l in 0..self.files[f].lines.len() {
            match self.files[f].task(l) {
                Ok(task) => tasks.extend(task.map(|t| (l, t))),
                Err(e) => {
                    if self.skipped.insert((f, l)) {
                        eprintln!("tm: skipping {}: {e}", self.loc((f, l)));
                    }
                }
            }
        }
        tasks
    }

    fn week_spec(&self, spec: &str) -> Result<NaiveDate> {
        match spec {
            "current" => Ok(monday_of(self.today)),
            "next" => Ok(monday_of(self.today) + Duration::days(7)),
            _ => week_start(spec).ok_or_else(|| format!("invalid week: {spec}")),
        }
    }

    /// Mondays of the existing week files, ascending.
    fn existing_weeks(&self) -> Vec<NaiveDate> {
        let mut weeks = Vec::new();
        let Ok(years) = fs::read_dir(&self.dir) else {
            return weeks;
        };
        for year in years.flatten() {
            let year_name = year.file_name().to_string_lossy().into_owned();
            let Ok(entries) = fs::read_dir(year.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let number = name.strip_prefix('w').and_then(|n| n.strip_suffix(".md"));
                if let Some(monday) = number.and_then(|n| week_start(&format!("{year_name}-w{n}")))
                {
                    weeks.push(monday);
                }
            }
        }
        weeks.sort();
        weeks
    }

    /// All task files in listing order: backlog, then weeks ascending.
    fn task_files(&self) -> Vec<String> {
        let weeks = self.existing_weeks().into_iter().map(week_file);
        ["backlog.md".to_string()]
            .into_iter()
            .chain(weeks)
            .collect()
    }

    /// Finds one task by `file:line` or by a substring of an open task's description.
    fn resolve(
        &mut self,
        reference: &str,
        scope: &[&str],
        any_file: bool,
    ) -> Result<(usize, usize)> {
        if let Some((name, line)) = split_ref(reference) {
            let in_scope = if any_file {
                is_task_file(name)
            } else {
                scope.contains(&name)
            };
            if !in_scope {
                let place = if any_file {
                    "the task files".to_string()
                } else {
                    scope.join(", ")
                };
                return Err(format!("{reference} is not in {place}"));
            }
            let f = self.load(name)?;
            let found = line
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .map(|l| (l, self.files[f].task(l)));
            return match found {
                Some((_, Err(e))) => Err(format!("{reference}: {e}")),
                Some((_, Ok(Some(t)))) if t.done => Err(format!("already done: {reference}")),
                Some((l, Ok(Some(_)))) => Ok((f, l)),
                _ => Err(format!("not a task: {reference}")),
            };
        }

        if reference.trim().is_empty() {
            return Err("empty reference".to_string());
        }
        let query = reference.to_lowercase();
        let mut matches = Vec::new();
        for name in scope {
            let f = self.load(name)?;
            for (l, task) in self.tasks(f) {
                let description = task.description.to_lowercase();
                if !task.done && description.contains(&query) {
                    matches.push((f, l, description == query));
                }
            }
        }
        match matches.as_slice() {
            [] => Err(format!(
                "no open task matches \"{reference}\" in {}",
                scope.join(", ")
            )),
            [(f, l, _)] => Ok((*f, *l)),
            _ => {
                let mut exact = matches.iter().filter(|m| m.2);
                if let (Some(&(f, l, _)), None) = (exact.next(), exact.next()) {
                    return Ok((f, l));
                }
                let mut message = format!("ambiguous \"{reference}\", {} matches:", matches.len());
                for &(f, l, _) in &matches {
                    message += &format!("\n{}: {}", self.loc((f, l)), self.files[f].text(l));
                }
                Err(message)
            }
        }
    }

    /// Resolves every reference before anything is written; the same task twice is an error.
    fn resolve_all(
        &mut self,
        refs: &[String],
        scope: &[&str],
        any_file: bool,
    ) -> Result<Vec<(usize, usize)>> {
        let mut found = Vec::new();
        for reference in refs {
            let at = self.resolve(reference, scope, any_file)?;
            if found.contains(&at) {
                return Err(format!("same task given twice: {reference}"));
            }
            found.push(at);
        }
        Ok(found)
    }

    /// Adds `@created(today)` unless present; the error is the first problem in the line.
    fn new_task(&self, line: &str) -> Result<String> {
        let task = parse_line(line).map_err(|e| e.to_string())?;
        if task.is_some_and(|t| t.created.is_some()) {
            return Ok(line.to_string());
        }
        Ok(with_token(line, "created", &self.today.to_string()))
    }

    fn append_to(&mut self, name: &str, line: &str) -> Result<usize> {
        let f = self.load(name)?;
        let number = self.files[f].append(line);
        self.save(f)?;
        Ok(number)
    }

    fn add(&mut self, text: Vec<String>) -> Result<()> {
        let text = join_text(text)?;
        if text.starts_with("- [") {
            return Err("give the text without the checkbox".to_string());
        }
        let line = self.new_task(&format!("- [ ] {text}"))?;
        let number = self.append_to("backlog.md", &line)?;
        out!("backlog.md:{number}");
        Ok(())
    }

    fn done(&mut self, refs: &[String]) -> Result<()> {
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
            out!("{}", self.loc(at));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn list(
        &mut self,
        query: Option<String>,
        scope: Option<String>,
        open: bool,
        done: bool,
        tags: Vec<String>,
        proj: Option<String>,
        due_before: Option<String>,
        json: bool,
    ) -> Result<()> {
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
            out!("{}", json!(items));
        } else {
            for (file, line, text, _) in rows {
                out!("{file}:{line}: {text}");
            }
        }
        Ok(())
    }

    fn week(&mut self, weekspec: Option<String>, json: bool) -> Result<()> {
        let monday = self.week_spec(weekspec.as_deref().unwrap_or("current"))?;
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
            out!("{id} ({monday}..{end}, {status}) {name}");
            out!("tasks {}: {} done, {} open", s.tasks, s.done, s.open);
            out!(
                "hours: planned {}h, done {}h, open {}h",
                hours(s.planned),
                hours(s.done_hours),
                hours(s.open_hours)
            );
            out!("no estimate: {}", s.no_estimate);
            if !s.other.is_empty() {
                out!("other estimates: {}", s.other.join(", "));
            }
            out!("past weeks: {}", past_weeks.join(", "));
            out!("past average done: {}", average_text(past_avg));
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

    fn take(&mut self, refs: &[String], week: Option<String>) -> Result<()> {
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
            out!("{name}:{number}");
        }
        let planned = stats(&self.tasks(dest)).planned;
        let (_, average) = self.past_weeks()?;
        out!(
            "{}: planned {}h, past average done {}",
            week_id(monday),
            hours(planned),
            average_text(average)
        );
        Ok(())
    }

    fn close(&mut self, weekspec: Option<String>) -> Result<()> {
        let monday = match weekspec {
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
            out!("nothing to close in {name}");
            return Ok(());
        }
        self.save(backlog)?;
        for number in copied {
            out!("backlog.md:{number}");
        }
        Ok(())
    }

    fn lint(&mut self) -> Result<()> {
        let mut problems = false;
        for name in self.task_files() {
            let f = self.load(&name)?;
            for l in 0..self.files[f].lines.len() {
                if let Err(e) = self.files[f].task(l) {
                    out!("{}: {e}", self.loc((f, l)));
                    problems = true;
                }
            }
        }
        if problems {
            process::exit(1);
        }
        Ok(())
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

fn join_text(words: Vec<String>) -> Result<String> {
    let text = words.join(" ");
    if text.trim().is_empty() {
        Cli::command()
            .error(UsageError::TooFewValues, "the text must not be empty")
            .exit();
    }
    if text.contains(['\n', '\r']) {
        return Err("text must be a single line".to_string());
    }
    Ok(text)
}

fn io_error(path: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

fn monday_of(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday().into())
}

fn week_id(monday: NaiveDate) -> String {
    let week = monday.iso_week();
    format!("{}-w{:02}", week.year(), week.week())
}

fn week_file(monday: NaiveDate) -> String {
    let week = monday.iso_week();
    format!("{}/w{:02}.md", week.year(), week.week())
}

fn is_task_file(name: &str) -> bool {
    let week = name
        .split_once("/w")
        .and_then(|(year, rest)| Some((year, rest.strip_suffix(".md")?)))
        .and_then(|(year, number)| week_start(&format!("{year}-w{number}")));
    name == "backlog.md" || week.is_some_and(|m| week_file(m) == name)
}

/// Splits `file.md:N` into the file name and the digits.
fn split_ref(reference: &str) -> Option<(&str, &str)> {
    let (name, line) = reference.rsplit_once(':')?;
    let digits = !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit());
    (digits && name.len() > 3 && name.ends_with(".md")).then_some((name, line))
}

fn hours(h: f64) -> String {
    format!("{}", (h * 100.0).round() / 100.0)
}

fn average_text(average: Option<f64>) -> String {
    average.map_or("n/a".to_string(), |a| format!("{}h", hours(a)))
}
