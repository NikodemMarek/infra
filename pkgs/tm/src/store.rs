use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::{env, fs};

use chrono::{Datelike, Duration, Local, NaiveDate};

use crate::{parse_date, parse_line, week_start, with_token, ParseError, Task};

pub type Result<T> = std::result::Result<T, String>;

/// A task file as text lines, each keeping its own `\n`, so writing the
/// lines back is byte-identical.
pub struct TaskFile {
    pub name: String,
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

    pub fn contains(&self, line: &str) -> bool {
        (0..self.lines.len()).any(|i| self.text(i) == line)
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
    pub files: Vec<TaskFile>,
    /// Invalid lines already warned about, as (file index, line index).
    skipped: BTreeSet<(usize, usize)>,
    /// The warnings for those lines, in the order they were found.
    pub warnings: Vec<String>,
}

impl Store {
    pub fn open() -> Result<Store> {
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
            warnings: Vec::new(),
        })
    }

    /// Index of the file in `files`, reading it on first use; a missing file is empty.
    pub fn load(&mut self, name: &str) -> Result<usize> {
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
    pub fn save(&self, i: usize) -> Result<()> {
        let file = &self.files[i];
        let path = self.dir.join(&file.name);
        let parent = path.parent().unwrap_or(&self.dir);
        let base = path.file_name().unwrap_or_default().to_string_lossy();
        let tmp = parent.join(format!(".{base}.tm-tmp"));
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        fs::write(&tmp, file.lines.concat()).map_err(|e| io_error(&tmp, e))?;
        fs::rename(&tmp, &path).map_err(|e| io_error(&path, e))
    }

    pub fn save_all(&self, files: impl IntoIterator<Item = usize>) -> Result<()> {
        let files: BTreeSet<usize> = files.into_iter().collect();
        files.into_iter().try_for_each(|i| self.save(i))
    }

    pub fn loc(&self, (f, l): (usize, usize)) -> String {
        format!("{}:{}", self.files[f].name, l + 1)
    }

    /// The tasks of a loaded file with their line indexes. An invalid line is
    /// skipped, with a warning the first time it is seen.
    pub fn tasks(&mut self, f: usize) -> Vec<(usize, Task)> {
        let mut tasks = Vec::new();
        for l in 0..self.files[f].lines.len() {
            match self.files[f].task(l) {
                Ok(task) => tasks.extend(task.map(|t| (l, t))),
                Err(e) => {
                    if self.skipped.insert((f, l)) {
                        let warning = format!("skipping {}: {e}", self.loc((f, l)));
                        self.warnings.push(warning);
                    }
                }
            }
        }
        tasks
    }

    pub fn week_spec(&self, spec: &str) -> Result<NaiveDate> {
        match spec {
            "current" => Ok(monday_of(self.today)),
            "next" => Ok(monday_of(self.today) + Duration::days(7)),
            _ => week_start(spec).ok_or_else(|| format!("invalid week: {spec}")),
        }
    }

    /// Mondays of the existing week files, ascending.
    pub fn existing_weeks(&self) -> Vec<NaiveDate> {
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
    pub fn task_files(&self) -> Vec<String> {
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
    pub fn resolve_all(
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
    pub fn new_task(&self, line: &str) -> Result<String> {
        let task = parse_line(line).map_err(|e| e.to_string())?;
        if task.is_some_and(|t| t.created.is_some()) {
            return Ok(line.to_string());
        }
        Ok(with_token(line, "created", &self.today.to_string()))
    }

    pub fn append_to(&mut self, name: &str, line: &str) -> Result<usize> {
        let f = self.load(name)?;
        let number = self.files[f].append(line);
        self.save(f)?;
        Ok(number)
    }
}

fn io_error(path: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

pub fn monday_of(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday().into())
}

pub fn week_id(monday: NaiveDate) -> String {
    let week = monday.iso_week();
    format!("{}-w{:02}", week.year(), week.week())
}

pub fn week_file(monday: NaiveDate) -> String {
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
