use std::ops::Range;

use chrono::NaiveDate;

use crate::task::{Est, ParseError, Status, Task, Unit};

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
    status: Status,
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
    let status = match b[3] {
        b' ' => Status::Todo,
        b'/' => Status::Doing,
        b'x' | b'X' => Status::Done,
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
        status,
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
        status: p.status,
        description: description.to_string(),
        est: None,
        due: None,
        created: None,
        done_on: None,
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
        _ => return Err(format!("unknown token @{name}")),
    }
    Ok(())
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
    (closed && !matches!(c, ' ' | '/' | 'x' | 'X')).then(|| format!("unknown checkbox: [{c}]"))
}
