use clap::{Args, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Subcommand)]
pub enum Request {
    /// Add a task to tasks.md (text without the checkbox)
    Add(AddArgs),
    /// Mark tasks as doing ([/])
    Start(RefsArgs),
    /// Put doing tasks back to open ([ ])
    Stop(RefsArgs),
    /// Mark tasks done
    Done(RefsArgs),
    /// List tasks
    List(ListArgs),
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
pub struct RefsArgs {
    /// Line number or a substring of an open (not done) task's description
    #[arg(required = true, value_name = "REF")]
    pub refs: Vec<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    /// Case-insensitive substring of the description
    pub query: Option<String>,
    /// Only tasks with this status (open = todo and doing)
    #[arg(short, long, value_enum)]
    pub status: Option<StatusFilter>,
    /// Tags (also match sub-tags); with several, all must match
    #[arg(long = "tag", value_name = "TAG")]
    #[serde(default)]
    pub tags: Vec<String>,
    /// Only tasks with a valid @due on or before YYYY-MM-DD
    #[arg(long, value_name = "DATE")]
    pub due_before: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(inline)]
pub enum StatusFilter {
    Open,
    Todo,
    Doing,
    Done,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LintArgs {}
