use crate::args::{LintArgs, Request};
use crate::store::{Result, Store};

/// `println!` into the output buffer.
macro_rules! out {
    ($out:expr, $($arg:tt)*) => {
        let _ = writeln!($out, $($arg)*);
    };
}

mod add;
mod lint;
mod list;
mod state;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Exactly what the CLI writes to stdout, trailing newline included.
    pub stdout: String,
    /// Lines without the `tm: ` prefix: skip warnings first, then the error (if any).
    pub stderr: Vec<String>,
    /// The CLI exits with 1.
    pub failed: bool,
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

/// The checks every command does first: TM_DIR is set and exists and TM_TODAY is valid.
pub fn check_env() -> std::result::Result<(), String> {
    Store::open().map(|_| ())
}

/// Returns whether the command failed without an error message (lint found problems).
fn run(store: &mut Store, request: Request, json: bool, out: &mut String) -> Result<bool> {
    match request {
        Request::Add(args) => add::add(store, &args.text, out)?,
        Request::Start(args) => state::start(store, &args.refs, out)?,
        Request::Stop(args) => state::stop(store, &args.refs, out)?,
        Request::Done(args) => state::done(store, &args.refs, out)?,
        Request::List(args) => list::list(store, args, json, out)?,
        Request::Lint(LintArgs {}) => return lint::lint(store, out),
    }
    Ok(false)
}
