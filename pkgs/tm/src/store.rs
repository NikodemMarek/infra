use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::{env, fs};

use chrono::{Local, NaiveDate};

use crate::parse::{parse_date, parse_line, with_token};
use crate::task::{ParseError, Status, Task};

pub type Result<T> = std::result::Result<T, String>;

const FILE: &str = "tasks.md";

/// The task file as text lines, each keeping its own `\n`, so writing the
/// lines back is byte-identical.
pub struct TaskFile {
    pub lines: Vec<String>,
}

impl TaskFile {
    pub fn text(&self, i: usize) -> &str {
        self.lines[i].strip_suffix('\n').unwrap_or(&self.lines[i])
    }

    pub fn task(&self, i: usize) -> std::result::Result<Option<Task>, ParseError> {
        if i >= self.lines.len() {
            return Ok(None);
        }
        parse_line(self.text(i))
    }

    pub fn set(&mut self, i: usize, line: &str) {
        let newline = if self.lines[i].ends_with('\n') {
            "\n"
        } else {
            ""
        };
        self.lines[i] = format!("{line}{newline}");
    }

    /// Returns the 1-based number of the new line.
    pub fn append(&mut self, line: &str) -> usize {
        if let Some(last) = self.lines.last_mut() {
            if !last.ends_with('\n') {
                last.push('\n');
            }
        }
        self.lines.push(format!("{line}\n"));
        self.lines.len()
    }
}

pub struct Store {
    pub dir: PathBuf,
    pub today: NaiveDate,
    pub file: TaskFile,
    /// Invalid lines already warned about, as line indexes.
    skipped: BTreeSet<usize>,
    /// The warnings for those lines, in the order they were found.
    pub warnings: Vec<String>,
}

impl Store {
    /// Reads `tasks.md`; a missing file is empty.
    pub fn open() -> Result<Store> {
        let dir = match env::var_os("TM_DIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => return Err("TM_DIR is not set".to_string()),
        };
        if !dir.is_dir() {
            return Err(format!("tasks dir not found: {}", dir.display()));
        }
        let today = match env::var("TM_TODAY") {
            Ok(s) => parse_date(&s).ok_or_else(|| format!("invalid TM_TODAY: {s}"))?,
            Err(_) => Local::now().date_naive(),
        };
        let path = dir.join(FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io_error(&path, e)),
        };
        Ok(Store {
            dir,
            today,
            file: TaskFile {
                lines: text.split_inclusive('\n').map(String::from).collect(),
            },
            skipped: BTreeSet::new(),
            warnings: Vec::new(),
        })
    }

    pub fn save(&self) -> Result<()> {
        let path = self.dir.join(FILE);
        fs::write(&path, self.file.lines.concat()).map_err(|e| io_error(&path, e))
    }

    /// The tasks with their line indexes. An invalid line is skipped, with a
    /// warning the first time it is seen.
    pub fn tasks(&mut self) -> Vec<(usize, Task)> {
        let mut tasks = Vec::new();
        for l in 0..self.file.lines.len() {
            match self.file.task(l) {
                Ok(task) => tasks.extend(task.map(|t| (l, t))),
                Err(e) => {
                    if self.skipped.insert(l) {
                        self.warnings.push(format!("skipping line {}: {e}", l + 1));
                    }
                }
            }
        }
        tasks
    }

    /// Adds `@created(today)` unless present; the error is the first problem in the line.
    pub fn new_task(&self, line: &str) -> Result<String> {
        let task = parse_line(line).map_err(|e| e.to_string())?;
        if task.is_some_and(|t| t.created.is_some()) {
            return Ok(line.to_string());
        }
        Ok(with_token(line, "created", &self.today.to_string()))
    }

    /// Finds the line index and status of one open task by line number or by a
    /// substring of its description.
    fn resolve(&mut self, reference: &str) -> Result<(usize, Status)> {
        if reference.trim().is_empty() {
            return Err("empty reference".to_string());
        }
        if reference.bytes().all(|b| b.is_ascii_digit()) {
            let number = reference.parse::<usize>().ok();
            // Leading zeros are dropped; digits too long for a usize stay as given.
            let shown = number.map_or_else(|| reference.to_string(), |n| n.to_string());
            let task = number
                .and_then(|n| n.checked_sub(1))
                .map(|l| (l, self.file.task(l)));
            return match task {
                Some((_, Err(e))) => Err(format!("line {shown}: {e}")),
                Some((_, Ok(Some(t)))) if t.status == Status::Done => {
                    Err(format!("already done: line {shown}"))
                }
                Some((l, Ok(Some(t)))) => Ok((l, t.status)),
                _ => Err(format!("not a task: line {shown}")),
            };
        }

        let query = reference.to_lowercase();
        let mut matches = Vec::new();
        for (l, task) in self.tasks() {
            let description = task.description.to_lowercase();
            if task.status != Status::Done && description.contains(&query) {
                matches.push((l, task.status, description == query));
            }
        }
        match matches.as_slice() {
            [] => Err(format!("no open task matches \"{reference}\"")),
            [(l, status, _)] => Ok((*l, *status)),
            _ => {
                let mut exact = matches.iter().filter(|m| m.2);
                if let (Some(&(l, status, _)), None) = (exact.next(), exact.next()) {
                    return Ok((l, status));
                }
                let mut message = format!("ambiguous \"{reference}\", {} matches:", matches.len());
                for &(l, _, _) in &matches {
                    message += &format!("\n{}: {}", l + 1, self.file.text(l));
                }
                Err(message)
            }
        }
    }

    /// Resolves every reference before anything is written; the same task twice is an error.
    /// `check` returns the error text for a status the command cannot act on.
    pub fn resolve_all(
        &mut self,
        refs: &[String],
        check: impl Fn(Status) -> Option<&'static str>,
    ) -> Result<Vec<usize>> {
        let mut found = Vec::new();
        for reference in refs {
            let (l, status) = self.resolve(reference)?;
            if found.contains(&l) {
                return Err(format!("same task given twice: {reference}"));
            }
            if let Some(problem) = check(status) {
                return Err(format!("{problem}: line {}", l + 1));
            }
            found.push(l);
        }
        Ok(found)
    }
}

fn io_error(path: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", path.display())
}
