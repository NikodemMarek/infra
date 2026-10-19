---
name: tasks
description: Read and manage the user's markdown tasks (tasks.md) with the `tm` CLI: add, mark done, query, weekly planning and closing. Use when the user mentions their tasks, todos, backlog or weekly planning.
---

# Tasks (`tm`)

The user keeps tasks as markdown checkboxes in one file, `$TM_DIR/tasks.md` (`TM_DIR` defaults to `~/omniverse/tasks`). A task's week (ISO, Mon-Sun) is its `@week` token; the backlog is the tasks without `@week`.

## Task line

`- [ ] description @est(2h) @due(YYYY-MM-DD) @created(YYYY-MM-DD) @done(YYYY-MM-DD) @week(YYYY-wNN) @drop(YYYY-wNN) @proj(name) @step(n) #tag/sub`

- **Tokens** (`@name(value)`) are typed fields, each at most once, after the description. `@week` is the week the task is planned for, `@drop` records the week a task was carried over from, `@step` orders tasks within a `@proj`, and it needs `@proj`.
- **Tags** (`#tag`, `#area/sub`) are free labels, also after the description.
- `@est` is in hours (`2h`, `0.5h`); `m` is months, `d` is undefined. A task over one day must be split before it is planned.

## Rules

- Use `tm` for every change to task lines. Never edit them by hand: `tm` touches one line and keeps the rest byte-identical.
- Quote text arguments (`tm add "write report @est(2h)"`); the shell treats `(` specially (fish too).
- Prefer `--json` for reading, and line numbers (printed as `N: <line>` by `add`, `list`, `take`, ...) for writing.
- A reference is a line number (digits only) or a substring of an open task's description. If a substring is ambiguous, `tm` fails and lists the candidates as `N: <line>`; retry with one of those numbers.
- `tm` never moves lines, so line numbers stay valid across `tm` commands. After any manual edit of the file, run `tm lint` (prints `N: problem`, exit 1 if any) and `tm list` again before using line numbers.
- Invalid task lines are skipped, with a `tm: skipping line N: <problem>` warning on stderr; run `tm lint` to see them all. A line-number reference to an invalid line is an error.

## Commands

- `tm add "<text>"`: append a structured task (adds `@created`, lints first). Give the text without `- [ ]`; it may include tokens such as `@week`.
- `tm done <ref>...`: mark done (sets `@done`, keeps `@week`).
- `tm list [query] [--in backlog|<week>] [--open|--done] [--tag T]... [--proj P] [--due-before DATE] [--json]`: query tasks. A week is `current`, `next` or `YYYY-wNN`; `backlog` means no `@week`.
- `tm week [<week>] [--json]`: planned, done and open hours of the tasks with that `@week`, plus the average done hours of the past weeks.
- `tm take <ref>... [--week <week>]`: plan tasks by setting `@week` (replaced if already set; default: current week, on Sunday the next one). Refuses tasks over one day.
- `tm close [<week>]`: return a finished week's open tasks to the backlog: removes `@week` and sets `@drop(week)`. Safe to repeat.
- `tm lint`: validate tasks.md.

## Weekly planning (Sunday)

1. `tm close` to return the unfinished tasks of the week that ends to the backlog.
2. `tm week next --json`: note `past_avg_done_hours`, the realistic load.
3. `tm list --in backlog --open --json`: anything over one day must be split into smaller tasks first (`tm add` each part). `tm` cannot delete, so tell the user to remove the oversized original.
4. Propose a selection: closest `@due` first, total estimate around the past average, fewer is better.
5. Confirm the selection with the user, then `tm take <refs>`. The last line shows planned hours against the average.
