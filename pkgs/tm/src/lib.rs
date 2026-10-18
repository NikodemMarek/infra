use std::ops::Range;

use chrono::{Datelike, NaiveDate, Weekday};

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub done: bool,
    pub description: String,
    pub tokens: Vec<(String, String)>,
    pub tags: Vec<String>,
}

impl Task {
    pub fn token(&self, name: &str) -> Option<&str> {
        self.tokens
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn est_hours(&self) -> Option<f64> {
        match parse_est(self.token("est")?)? {
            (hours, 'h') => Some(hours),
            _ => None,
        }
    }
}

/// `@est` value as (amount, unit): `^\d+(\.\d+)?[hdmy]$` with amount > 0.
pub fn parse_est(value: &str) -> Option<(f64, char)> {
    let unit = value.chars().last().filter(|c| "hdmy".contains(*c))?;
    let number = &value[..value.len() - 1];
    let (int, frac) = number.split_once('.').unwrap_or((number, "0"));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || !digits(frac) {
        return None;
    }
    let amount: f64 = number.parse().ok()?;
    (amount > 0.0).then_some((amount, unit))
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

/// The Monday of an existing ISO week given as `YYYY-wNN`.
pub fn week_start(id: &str) -> Option<NaiveDate> {
    let (year, week) = id.split_once("-w")?;
    let all_digits = year.bytes().chain(week.bytes()).all(|b| b.is_ascii_digit());
    if year.len() != 4 || week.len() != 2 || !all_digits {
        return None;
    }
    let year: i32 = year.parse().ok()?;
    let week: u32 = week.parse().ok()?;
    let monday = NaiveDate::from_isoywd_opt(year, week, Weekday::Mon)?;
    let iso = monday.iso_week();
    (iso.year() == year && iso.week() == week).then_some(monday)
}

enum Kind {
    /// `@name(value)`; `name_end` is the byte offset of the `(`.
    Token {
        name_end: usize,
    },
    Tag,
    Word,
}

struct Unit {
    start: usize,
    end: usize,
    kind: Kind,
}

struct Parsed<'a> {
    line: &'a str,
    done: bool,
    /// Byte offset where the text after the checkbox begins.
    start: usize,
    units: Vec<Unit>,
    /// Index of the first unit of the trailing run of tokens and tags.
    meta: usize,
}

impl Parsed<'_> {
    fn text(&self, unit: &Unit) -> &str {
        &self.line[unit.start..unit.end]
    }

    fn token_parts(&self, unit: &Unit) -> Option<(&str, &str)> {
        match unit.kind {
            Kind::Token { name_end } => Some((
                &self.line[unit.start + 1..name_end],
                &self.line[name_end + 1..unit.end - 1],
            )),
            _ => None,
        }
    }

    fn metadata(&self) -> &[Unit] {
        &self.units[self.meta..]
    }

    fn task(&self) -> Task {
        let description_end = self
            .units
            .get(self.meta)
            .map_or(self.line.len(), |u| u.start);
        let mut task = Task {
            done: self.done,
            description: self.line[self.start..description_end].trim().to_string(),
            tokens: Vec::new(),
            tags: Vec::new(),
        };
        for unit in self.metadata() {
            if let Some((name, value)) = self.token_parts(unit) {
                task.tokens.push((name.to_string(), value.to_string()));
            } else if matches!(unit.kind, Kind::Tag) {
                task.tags.push(self.text(unit)[1..].to_string());
            }
        }
        task
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

fn split_units(line: &str, from: usize) -> Vec<Unit> {
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
            '@' => token_end(line, i).map(|(name_end, end)| Unit {
                start: i,
                end,
                kind: Kind::Token { name_end },
            }),
            '#' if word_end > i + 1 => Some(Unit {
                start: i,
                end: word_end,
                kind: Kind::Tag,
            }),
            _ => None,
        }
        .unwrap_or(Unit {
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

pub fn parse_line(line: &str) -> Option<Task> {
    parse(line).map(|p| p.task())
}

pub fn with_token(line: &str, name: &str, value: &str) -> String {
    if let Some(range) = parse(line).and_then(|p| find_value(&p, name)) {
        return format!("{}{value}{}", &line[..range.start], &line[range.end..]);
    }
    format!("{} @{name}({value})", line.trim_end())
}

fn find_value(p: &Parsed, name: &str) -> Option<Range<usize>> {
    p.metadata().iter().find_map(|unit| match unit.kind {
        Kind::Token { name_end } if p.token_parts(unit)?.0 == name => {
            Some(name_end + 1..unit.end - 1)
        }
        _ => None,
    })
}

pub fn lint_line(line: &str) -> Vec<String> {
    let Some(p) = parse(line) else {
        return unknown_checkbox(line).into_iter().collect();
    };
    let mut issues = Vec::new();
    if p.task().description.is_empty() {
        issues.push("empty description".to_string());
    }

    let tokens: Vec<(&str, &str)> = p
        .metadata()
        .iter()
        .filter_map(|u| p.token_parts(u))
        .collect();
    for (i, (name, value)) in tokens.iter().enumerate() {
        issues.extend(check_token(name, value));
        if tokens[..i].iter().filter(|(n, _)| n == name).count() == 1 {
            issues.push(format!("duplicate token @{name}"));
        }
    }
    let has = |name: &str| tokens.iter().any(|(n, _)| *n == name);
    if has("step") && !has("proj") {
        issues.push("@step without @proj".to_string());
    }

    for unit in &p.units[..p.meta] {
        let text = p.text(unit);
        if starts_like_token(text) {
            issues.push(format!("token inside description: {text}"));
        }
    }
    for unit in p.metadata() {
        let text = p.text(unit);
        if matches!(unit.kind, Kind::Tag) && text[1..].split('/').any(str::is_empty) {
            issues.push(format!("invalid tag: {text}"));
        }
    }
    issues
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

fn check_token(name: &str, value: &str) -> Option<String> {
    let invalid = format!("invalid @{name} value: {value}");
    match name {
        "est" => parse_est(value).is_none().then_some(invalid),
        "due" | "created" | "done" => parse_date(value)
            .is_none()
            .then(|| format!("invalid date in @{name}: {value}")),
        "drop" => week_start(value).is_none().then_some(invalid),
        "proj" => value.trim().is_empty().then(|| "empty @proj".to_string()),
        "step" => {
            let digits = value.bytes().all(|b| b.is_ascii_digit());
            let positive = digits && value.bytes().any(|b| b != b'0');
            (!positive).then_some(invalid)
        }
        _ => Some(format!("unknown token @{name}")),
    }
}
