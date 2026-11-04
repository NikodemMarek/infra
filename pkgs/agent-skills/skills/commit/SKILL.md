---
name: commit
description: Describe the current jj change(s) with a Conventional Commits message. Use when the user asks to commit, describe a change, or write a commit message.
disable-model-invocation: true
allowed-tools: Bash(jj st*) Bash(jj status*) Bash(jj diff*) Bash(jj show*) Bash(jj log*) Bash(jj describe*) Bash(jj commit*)
---

# Commit Skill

Write Conventional Commits messages for jj changes and apply them. Non-interactive jj forms are in the `jj` skill.

## Steps

1. Inspect the change: `jj st` and `jj diff --git` (of `@`, or `-r <rev>` if the user named one). Check `jj log -r 'trunk()..@'` for the surrounding stack.
2. If the change is empty, say so and stop.
3. Analyze the diff:
   - If it spans multiple unrelated concerns, propose splitting it into separate changes (`jj split <paths>`) and ask the user to confirm before proceeding.
   - Otherwise, draft a single message.
4. Follow Conventional Commits format:
   - `<type>(<optional scope>): <short description>`
   - Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `style`, `perf`, `ci`, `build`
   - Subject line: short, descriptive, under 72 characters, imperative mood
   - Add a body only when the change genuinely needs more explanation
5. Show the proposed message(s) to the user and ask for approval or edits.
6. Once approved, apply it:
   - `jj commit -m "..."` to finish `@` and start a new empty change on top (default when working in `@`), or
   - `jj describe -r <rev> -m "..."` to (re)describe a specific change without moving `@`.
   - Multi-line: `jj describe -m "$(printf 'feat: title\n\nBody')"`.

## Rules

- Push finished work with `jj upload` without asking (see the `jj` skill).
- Never rewrite (`describe`, `squash`, `split`) changes other than the ones the user asked about.
- Do not add co-author trailers.
