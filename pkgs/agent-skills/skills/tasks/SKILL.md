---
name: tasks
description: Read and manage the user's markdown tasks (raw.md, backlog.md, weekly files) with the `tm` CLI: capture, add, promote raw tasks, mark done, query, weekly planning and closing. Use when the user mentions their tasks, todos, backlog or weekly planning.
---

# Tasks (`tm`)

The user keeps tasks as markdown checkboxes in `$TM_DIR` (default `~/omniverse/tasks`):

- `raw.md`: unstructured captures, not yet planned.
- `backlog.md`: structured tasks waiting for a week.
- `<isoyear>/wNN.md`: one file per ISO week (Mon-Sun), e.g. `2026/w42.md`. No front matter.

## Task line

`- [ ] description @est(2h) @due(YYYY-MM-DD) @created(YYYY-MM-DD) @done(YYYY-MM-DD) @drop(YYYY-wNN) @proj(name) @step(n) #tag/sub`

- **Tokens** (`@name(value)`) are typed fields, each at most once, after the description. `@drop` records the week a task was carried over from, `@step` orders tasks within a `@proj`, and it needs `@proj`.
- **Tags** (`#tag`, `#area/sub`) are free labels, also after the description.
- `@est` is in hours (`2h`, `0.5h`); `m` is months, `d` is undefined. A task over one day must be split before it is planned.

## Rules

- Use `tm` for every change to task lines. Never edit them by hand: `tm` touches one line and keeps the rest byte-identical.
- Quote text arguments (`tm add "write report @est(2h)"`); the shell treats `(` specially (fish too).
- Prefer `--json` for reading, and `file:line` references (printed by `add`, `list`, `take`, ...) for writing.
- A plain-text reference is a substring of an open task's description. If it is ambiguous, `tm` fails and lists the candidates as `file:line`; retry with one of those.
- After any manual edit of a task file, run `tm lint` (prints `file:line: problem`, exit 1 if any).

## Commands

- `tm raw "<text>"`: capture a new line in raw.md.
- `tm add "<text>"`: add a structured task to backlog.md (adds `@created`, lints first). Give the text without `- [ ]`.
- `tm promote <ref> ["<text>"]`: move a raw task to backlog.md, optionally replacing it with a restructured line.
- `tm done <ref>...`: mark done in the current week, next week or backlog (`file:line` for other files).
- `tm list [query] [--in raw|backlog|<week>] [--open|--done] [--tag T]... [--proj P] [--due-before DATE] [--json]`: query tasks. A week is `current`, `next` or `YYYY-wNN`.
- `tm week [<week>] [--json]`: planned, done and open hours, plus the average done hours of the past weeks.
- `tm take <ref>... [--week <week>]`: move backlog tasks into a week (default: current, on Sunday the next one). Refuses tasks over one day.
- `tm close [<week>]`: copy a finished week's open tasks back to backlog.md with `@drop(week)`. Safe to repeat.
- `tm lint`: validate backlog.md and the week files.

## Processing raw.md

1. `tm list --in raw --json`.
2. Per task, decide with the user (ask when the intent is unclear): a clear description, `@est` in hours, `@due` if there is a deadline, `#tags` (reuse existing ones: check `tm list --json`).
3. `tm promote <ref> "<restructured line without checkbox>"`. If one capture holds several tasks, promote the first and `tm add` the others.

## Weekly planning (Sunday)

1. `tm close` to carry the unfinished tasks of the week that ends over to the backlog.
2. `tm week next --json`: note `past_avg_done_hours`, the realistic load.
3. `tm list --in backlog --open --json`: anything over one day must be split into smaller tasks first (`tm add` each part). `tm` cannot delete, so tell the user to remove the oversized original.
4. Propose a selection: closest `@due` first, total estimate around the past average, fewer is better.
5. Confirm the selection with the user, then `tm take <refs>`. The last line shows planned hours against the average.
