use std::ops::Range;

use chrono::{Datelike, NaiveDate, Weekday};

mod commands;
mod store;

pub use commands::{
    check_env, execute, AddArgs, DoneArgs, LintArgs, ListArgs, Output, Request, TakeArgs, WeekArgs,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Hours,
    Days,
    Months,
    Years,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Est {
    pub amount: f64,
    pub unit: Unit,
}

impl std::fmt::Display for Est {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let unit = match self.unit {
            Unit::Hours => 'h',
            Unit::Days => 'd',
            Unit::Months => 'm',
            Unit::Years => 'y',
        };
        write!(f, "{}{unit}", self.amount)
    }
}

impl Est {
    /// Some only for an estimate in hours.
    pub fn hours(&self) -> Option<f64> {
        (self.unit == Unit::Hours).then_some(self.amount)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Week {
    pub year: i32,
    pub week: u32,
}

impl std::fmt::Display for Week {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-w{:02}", self.year, self.week)
    }
}

impl Week {
    /// An existing ISO week given as `YYYY-wNN`.
    pub fn parse(s: &str) -> Option<Week> {
        let (year, week) = s.split_once("-w")?;
        let all_digits = year.bytes().chain(week.bytes()).all(|b| b.is_ascii_digit());
        if year.len() != 4 || week.len() != 2 || !all_digits {
            return None;
        }
        let year: i32 = year.parse().ok()?;
        let week: u32 = week.parse().ok()?;
        let monday = NaiveDate::from_isoywd_opt(year, week, Weekday::Mon)?;
        let iso = monday.iso_week();
        (iso.year() == year && iso.week() == week).then_some(Week { year, week })
    }

    /// The ISO week containing `date`.
    pub fn of(date: NaiveDate) -> Week {
        let iso = date.iso_week();
        Week {
            year: iso.year(),
            week: iso.week(),
        }
    }

    pub fn monday(&self) -> NaiveDate {
        self.day(Weekday::Mon)
    }

    pub fn sunday(&self) -> NaiveDate {
        self.day(Weekday::Sun)
    }

    fn day(&self, weekday: Weekday) -> NaiveDate {
        NaiveDate::from_isoywd_opt(self.year, self.week, weekday)
            .expect("Week holds an existing ISO week")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub done: bool,
    pub description: String,
    pub est: Option<Est>,
    pub due: Option<NaiveDate>,
    pub created: Option<NaiveDate>,
    pub done_on: Option<NaiveDate>,
    pub drop: Option<Week>,
    pub week: Option<Week>,
    /// The raw `@proj` value, not trimmed.
    pub proj: Option<String>,
    pub step: Option<u32>,
    pub tags: Vec<String>,
}

impl Task {
    pub fn est_hours(&self) -> Option<f64> {
        self.est?.hours()
    }
}

/// `@est` value: `^\d+(\.\d+)?[hdmy]$` with amount > 0.
pub fn parse_est(value: &str) -> Option<Est> {
    let unit = match value.chars().last()? {
        'h' => Unit::Hours,
        'd' => Unit::Days,
        'm' => Unit::Months,
        'y' => Unit::Years,
        _ => return None,
    };
    let number = &value[..value.len() - 1];
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let shaped = match number.split_once('.') {
        Some((whole, fraction)) => digits(whole) && digits(fraction),
        None => digits(number),
    };
    let amount: f64 = number.parse().ok().filter(|&a| shaped && a > 0.0)?;
    Some(Est { amount, unit })
}

/// A strict `YYYY-MM-DD` date.
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let b = s.as_bytes();
    let shaped = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        });
    if !shaped {
        return None;
    }
    NaiveDate::from_ymd_opt(
        s[..4].parse().ok()?,
        s[5..7].parse().ok()?,
        s[8..].parse().ok()?,
    )
}

enum Kind {
    /// `@name(value)`; `name_end` is the byte offset of the `(`.
    Token {
        name_end: usize,
    },
    Tag,
    Word,
}

struct Piece {
    start: usize,
    end: usize,
    kind: Kind,
}

struct Parsed<'a> {
    line: &'a str,
    done: bool,
    /// Byte offset where the text after the checkbox begins.
    start: usize,
    units: Vec<Piece>,
    /// Index of the first unit of the trailing run of tokens and tags.
    meta: usize,
}

impl Parsed<'_> {
    fn text(&self, unit: &Piece) -> &str {
        &self.line[unit.start..unit.end]
    }

    fn token_parts(&self, unit: &Piece) -> Option<(&str, &str)> {
        match unit.kind {
            Kind::Token { name_end } => Some((
                &self.line[unit.start + 1..name_end],
                &self.line[name_end + 1..unit.end - 1],
            )),
            _ => None,
        }
    }

    fn metadata(&self) -> &[Piece] {
        &self.units[self.meta..]
    }
}

fn parse(line: &str) -> Option<Parsed<'_>> {
    let b = line.as_bytes();
    if !line.starts_with("- [") || b.len() < 5 || b[4] != b']' {
        return None;
    }
    let done = match b[3] {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    let start = match b.get(5) {
        None => 5,
        Some(b' ') => 6,
        _ => return None,
    };
    let units = split_units(line, start);
    let meta = units
        .iter()
        .rposition(|u| matches!(u.kind, Kind::Word))
        .map_or(0, |i| i + 1);
    Some(Parsed {
        line,
        done,
        start,
        units,
        meta,
    })
}

fn split_units(line: &str, from: usize) -> Vec<Piece> {
    let mut units = Vec::new();
    let mut i = from;
    while i < line.len() {
        let c = line[i..].chars().next().unwrap();
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let word_end = line[i..]
            .find(char::is_whitespace)
            .map_or(line.len(), |n| i + n);
        let unit = match c {
            '@' => token_end(line, i).map(|(name_end, end)| Piece {
                start: i,
                end,
                kind: Kind::Token { name_end },
            }),
            '#' if word_end > i + 1 => Some(Piece {
                start: i,
                end: word_end,
                kind: Kind::Tag,
            }),
            _ => None,
        }
        .unwrap_or(Piece {
            start: i,
            end: word_end,
            kind: Kind::Word,
        });
        i = unit.end;
        units.push(unit);
    }
    units
}

/// Length of the token name at the start of `s` (the text after `@`).
fn token_name_len(s: &str) -> Option<usize> {
    let len = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(s.len());
    s.starts_with(|c: char| c.is_ascii_alphabetic())
        .then_some(len)
}

/// For a token starting at `at`: (offset of the `(`, end offset past the `)`).
fn token_end(line: &str, at: usize) -> Option<(usize, usize)> {
    let rest = &line[at + 1..];
    let name_len = token_name_len(rest)?;
    let value_len = rest[name_len..].strip_prefix('(')?.find(')')?;
    let end = at + 1 + name_len + 1 + value_len + 1;
    let tail = &line[end..];
    (tail.is_empty() || tail.starts_with(char::is_whitespace)).then_some((at + 1 + name_len, end))
}

/// `Ok(None)` for a line that is not a task, an error for a task line with a problem.
pub fn parse_line(line: &str) -> Result<Option<Task>, ParseError> {
    parse_task(line).map_err(ParseError)
}

fn parse_task(line: &str) -> Result<Option<Task>, String> {
    if let Some(problem) = unknown_checkbox(line) {
        return Err(problem);
    }
    let Some(p) = parse(line) else {
        return Ok(None);
    };
    let description_end = p.units.get(p.meta).map_or(line.len(), |u| u.start);
    let description = line[p.start..description_end].trim();
    if description.is_empty() {
        return Err("empty description".to_string());
    }
    if let Some(u) = p.units[..p.meta]
        .iter()
        .find(|u| starts_like_token(p.text(u)))
    {
        return Err(format!("token inside description: {}", p.text(u)));
    }

    let mut task = Task {
        done: p.done,
        description: description.to_string(),
        est: None,
        due: None,
        created: None,
        done_on: None,
        drop: None,
        week: None,
        proj: None,
        step: None,
        tags: Vec::new(),
    };
    let mut seen = Vec::new();
    for unit in p.metadata() {
        if let Some((name, value)) = p.token_parts(unit) {
            if seen.contains(&name) {
                return Err(format!("duplicate token @{name}"));
            }
            seen.push(name);
            set_token(&mut task, name, value)?;
        } else {
            let tag = &p.text(unit)[1..];
            if tag.split('/').any(str::is_empty) {
                return Err(format!("invalid tag: #{tag}"));
            }
            task.tags.push(tag.to_string());
        }
    }
    if task.step.is_some() && task.proj.is_none() {
        return Err("@step without @proj".to_string());
    }
    Ok(Some(task))
}

fn set_token(task: &mut Task, name: &str, value: &str) -> Result<(), String> {
    let invalid = || format!("invalid @{name} value: {value}");
    let date = || parse_date(value).ok_or_else(|| format!("invalid date in @{name}: {value}"));
    match name {
        "est" => task.est = Some(parse_est(value).ok_or_else(invalid)?),
        "due" => task.due = Some(date()?),
        "created" => task.created = Some(date()?),
        "done" => task.done_on = Some(date()?),
        "drop" => task.drop = Some(Week::parse(value).ok_or_else(invalid)?),
        "week" => task.week = Some(Week::parse(value).ok_or_else(invalid)?),
        "proj" if value.trim().is_empty() => return Err("empty @proj".to_string()),
        "proj" => task.proj = Some(value.to_string()),
        "step" => task.step = Some(parse_step(value).ok_or_else(invalid)?),
        _ => return Err(format!("unknown token @{name}")),
    }
    Ok(())
}

/// A positive integer that fits a u32, in ASCII digits only.
fn parse_step(value: &str) -> Option<u32> {
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().filter(|&n| n > 0)
}

pub fn with_token(line: &str, name: &str, value: &str) -> String {
    if let Some(range) = parse(line).and_then(|p| find_value(&p, name)) {
        return format!("{}{value}{}", &line[..range.start], &line[range.end..]);
    }
    format!("{} @{name}({value})", line.trim_end())
}

/// Removes the metadata token `@name(..)` and the whitespace before it.
pub fn without_token(line: &str, name: &str) -> String {
    let Some(p) = parse(line) else {
        return line.to_string();
    };
    let Some(unit) = p
        .metadata()
        .iter()
        .find(|u| p.token_parts(u).is_some_and(|(n, _)| n == name))
    else {
        return line.to_string();
    };
    let from = p.start + line[p.start..unit.start].trim_end().len();
    format!("{}{}", &line[..from], &line[unit.end..])
}

fn find_value(p: &Parsed, name: &str) -> Option<Range<usize>> {
    p.metadata().iter().find_map(|unit| match unit.kind {
        Kind::Token { name_end } if p.token_parts(unit)?.0 == name => {
            Some(name_end + 1..unit.end - 1)
        }
        _ => None,
    })
}

fn starts_like_token(word: &str) -> bool {
    let Some(rest) = word.strip_prefix('@') else {
        return false;
    };
    token_name_len(rest).is_some_and(|n| rest[n..].starts_with('('))
}

fn unknown_checkbox(line: &str) -> Option<String> {
    let c = line.strip_prefix("- [")?.chars().next()?;
    let rest = &line[3 + c.len_utf8()..];
    let closed = rest == "]" || rest.starts_with("] ");
    (closed && !matches!(c, ' ' | 'x' | 'X')).then(|| format!("unknown checkbox: [{c}]"))
}
