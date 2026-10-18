use chrono::NaiveDate;

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

/// The checkbox of a task: `[ ]`, `[/]` or `[x]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Todo,
    Doing,
    Done,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Status::Todo => "todo",
            Status::Doing => "doing",
            Status::Done => "done",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub status: Status,
    pub description: String,
    pub est: Option<Est>,
    pub due: Option<NaiveDate>,
    pub created: Option<NaiveDate>,
    pub done_on: Option<NaiveDate>,
    pub tags: Vec<String>,
}
