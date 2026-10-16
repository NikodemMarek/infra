---
name: knowledge
description: Recall and contribute curated knowledge (gotchas, conventions, tool behaviour, user preferences, org and project facts). Use before starting work in a language, tool, org or project to check what is already known, and whenever you learned something reusable that a future agent should know.
---

# Curated knowledge

The store is `@memoryDir@` (git). A background curator (`knowledge curate`, every 6 hours) folds queued notes into it. Never edit its files directly (edits are denied); queue a note instead. The files are small, so a lesson must earn its place.

## Recall
- `global.md` is already in your context. Scope files `{topic,lang,org}/<name>.md` and `projects/<slug>/facts.md` are injected into main sessions at start; subagents Read the ones relevant to their task. A scope file may link to a larger reference doc one level deeper (`topic/<name>/<doc>.md`).
- Otherwise search: `rg -i <term> @memoryDir@ --glob '!private/**'`.
- Credentials referenced as `private/<org>.md` are in `@memoryDir@/private/` (local only, never commit or paste them elsewhere).
- Project design memory (overview, decisions, log, notes) is the architect's: `@memoryDir@/projects/<slug>/`.

## Contribute
Submit only what passes ALL of: reusable beyond this task; non-obvious (not general knowledge, not derivable from the repo in a minute); stable for a month or more; actionable; evidenced (file:line, command output, or what the user said). Never submit task status, progress, "tests pass", transcripts, secrets or personal data. Most tasks produce nothing worth submitting.

A note is a `title:` line, optional `kind:` (fact|gotcha|howto|convention|preference|reference), `scope:` (e.g. `topic/jj`) and `evidence:` lines, then `---`, then the lesson (rule, why, example). A block without a title or `---` is skipped (`knowledge submit` accepts any text).

- With Bash: pipe the note to `knowledge submit` (no flags); it prints the note's path.

  ```
  knowledge submit <<'EOF'
  title: <one line>
  kind: gotcha
  scope: topic/jj
  evidence: <project, date, what you saw>
  ---
  <rule, why, example>
  EOF
  ```
- Without Bash (read-only subagents): end your final report with one block per lesson; a hook queues it. Start the fences at the beginning of the line, like this:

```knowledge
title: <one line>
kind: gotcha
scope: topic/jj
evidence: <project, date, what you saw>
---
<rule, why, example>
```

  The block ends at the first line that is only the closing fence, so a body with a fenced code example needs four backticks on both fences (````knowledge ... ````), and then the body may hold normal triple-backtick code.
- Preferences the user states in conversation are valuable: submit them (kind preference, evidence "user said ...").
- If memory lacked an answer you then found and it passes the bar, submit it.
- If submitting fails, say so in your report; do not retry in a loop.

The user runs `knowledge skills` on demand to draft skills from the knowledge; org knowledge never becomes a skill.
