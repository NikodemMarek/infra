---
name: tasks
description: Read and manage the user's markdown tasks (tasks.md) with the `tm` CLI: add, start, stop, mark done, query. Use when the user mentions their tasks, todos or backlog.
---

# Tasks (`tm`)

The user keeps tasks as markdown checkboxes in one rolling backlog, `$TM_DIR/tasks.md`. The checkbox is the state: `[ ]` todo, `[/]` doing (being worked on), `[x]` done. "Open" means not done, so `[ ]` and `[/]`.

## Task line

`- [ ] description @est(2h) @due(YYYY-MM-DD) @created(YYYY-MM-DD) @done(YYYY-MM-DD) #tag/sub`

- **Tokens** (`@name(value)`) are typed fields, each at most once, after the description. Other token names are errors.
- **Tags** (`#tag`, `#area/sub`) are free labels, also after the description.
- `@est` is plain metadata for the user's own reading: `h` hours, `d` days, `m` months, `y` years (`2h`, `0.5h`, `3d`).

## Rules

- Use `tm` for every change to task lines. Never edit them by hand: `tm` touches one line and keeps the rest byte-identical.
- Quote text arguments (`tm add "write report @est(2h)"`); the shell treats `(` specially (fish too).
- Prefer `--json` for reading, and line numbers (printed as `N: <line>` by `add`, `list`, `start`, ...) for writing.
- A reference is a line number (digits only) or a substring of an open task's description. If a substring is ambiguous, `tm` fails and lists the candidates as `N: <line>`; retry with one of those numbers.
- `tm` never moves lines, so line numbers stay valid across `tm` commands. After any manual edit of the file, run `tm lint` (prints `N: problem`, exit 1 if any) and `tm list` again before using line numbers.
- Invalid task lines are skipped, with a `tm: skipping line N: <problem>` warning on stderr; run `tm lint` to see them all. A line-number reference to an invalid line is an error.
- `tm` cannot delete or reword tasks; tell the user when one needs that.

## Commands

- `tm add "<text>"`: append a structured task (adds `@created`, lints first). Give the text without the checkbox; it may include tokens such as `@due`.
- `tm start <ref>...`: mark tasks doing (`[/]`). Fails on a task that is already doing or done.
- `tm stop <ref>...`: put doing tasks back to todo (`[ ]`). Fails on a task that is not doing.
- `tm done <ref>...`: mark tasks done (`[x]`, sets `@done`); works on todo and doing tasks.
- `tm list [query] [--status open|todo|doing|done] [--tag T]... [--due-before DATE] [--json]`: query tasks. `open` is everything not done (`[ ]` and `[/]`).
- `tm lint`: validate tasks.md.

## Workflow

- What is on the plate: `tm list --status open --json`. What is in progress: `tm list --status doing`.
- `tm add` new tasks as they come up. `tm start` a task when work on it begins and `tm done` when it is finished; `tm stop` shelves a started task.
- Keep few tasks doing at once. This is advice, not enforced: `tm` allows any number.
- When asked what to work on next, suggest from `tm list --status open --json`: doing tasks first, then the closest `@due`.
