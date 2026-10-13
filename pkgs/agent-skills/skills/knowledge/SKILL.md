---
name: knowledge
description: Recall and contribute curated knowledge (gotchas, conventions, tool behaviour, user preferences, org and project facts). Use before starting work in a language, tool, org or project to check what is already known, and whenever you learned something reusable that a future agent should know.
---

# Curated knowledge

The store is `@memoryDir@` (git). The knowledge curator, a background `claude -p` run started through the event router, owns it. Never edit it directly. It is bounded: entries are merged, go stale, and get dropped when a scope is over budget, so a lesson must earn its place. Skill-worthy patterns become skill ideas, drafts in `@memoryDir@/skill-ideas/`; tell the user when the session context mentions new ones. They copy them into the skills/ dir of the Claude config repo (`~/projects/real-dotfiles/pkgs/agent-skills/skills/`) by hand and then run `knowledge drop --taken skill-ideas/<name>.md` (the curator may keep improving that skill), or reject them with `knowledge drop skill-ideas/<name>.md` (the curator is told not to draft it again). Org knowledge never becomes a skill, only a playbook entry.

## Recall
- With Bash: `knowledge ask "<question>"` (run in the project dir) answers with sources from curated knowledge and project memory. Without Bash, read the index as below.
- `@memoryDir@/KNOWLEDGE.md` is the index, one line per entry, grouped by scope. Main sessions get the entries for their directory at start.
- Scopes: `global/`, `lang/<language>/`, `topic/<tool-or-domain>/`, `org/<org>/` (e.g. `org/acme/`; `playbook-*.md` are larger references), `projects/<slug>/facts/`. Grep the scope directory for your topic, then Read the entry.
- Entries carry `confidence`, `trust` and `last_verified`. Treat low-confidence or stale entries as hints to verify. Credentials referenced as `private/<org>.md#...` are in `@memoryDir@/private/` (local only, never commit or paste them elsewhere).
- Project design memory (overview, decisions, log, notes) is the architect's: `@memoryDir@/projects/<slug>/`.

## Contribute
Submit only what passes ALL of: reusable beyond this task; non-obvious (not general knowledge, not derivable from the repo in a minute); stable for a month or more; actionable; evidenced (file:line, command output, or what the user said). Never submit task status, progress, "tests pass", transcripts, secrets or personal data. Most tasks produce nothing worth submitting.

- With Bash:
  `knowledge submit --title "<one line>" --kind <fact|gotcha|howto|convention|preference|reference> [--scope <hint>] --evidence "<where it was observed>" <<'EOF'`
  then the lesson (rule, why, example) and `EOF`. It prints an id; `knowledge status <id>` shows the verdict later (curation runs in batches, within about 6 hours). Always pipe the body on stdin: `--file` and `--user` are refused for agents.
- Without Bash (read-only subagents): end your final report with one block per lesson; a hook submits it. Start the fences at the beginning of the line, like this:

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
- If submitting fails ("router unreachable"), say so in your report; do not retry in a loop.
