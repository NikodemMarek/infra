---
name: atomic-commits
description: Plan, build and verify a change as a stack of small, atomic commits — each one easy to review, non-breaking (builds and passes the full test suite on its own) and fully tested. Use when implementing any non-trivial feature, fix or refactor; when asked for small/atomic/reviewable/incremental commits; when splitting, slicing or restructuring a change or stack; or before uploading a stack for review. Works with the `jj` skill (load it too in a jj repo).
---

# Small, atomic, non-breaking, fully tested commits

**Goal:** a stack where every commit could be merged on its own, in order, and leave trunk green and shippable. A reviewer should understand each commit in about 15 minutes without looking at the others.

In jj repos this pairs with the user's `jj upload` (one commit = one MR), so **every commit is a merge request**. Hold each one to that standard.

## The four properties (check every commit against all of them)

1. **Atomic: one concern.** The title says what it does with no "and". It must not mix:
   - refactor with behaviour change
   - mechanical change (rename, move, format, dependency bump, generated code) with logic
   - two unrelated fixes

   If you have to explain two things, it's two commits.
2. **Small.** Aim for **≤ ~200 changed lines**, excluding lockfiles, generated code and snapshots, with **~400 as a hard ceiling**. Pure mechanical commits (renames, moves) may be bigger, because they're reviewed by skimming. Their message must say so ("mechanical: no behaviour change").
3. **Non-breaking.** At this commit the project builds, the full test suite and linters pass, and nothing user-visible or API-visible breaks:
   - No dead references.
   - No half-wired feature reachable by users.
   - No "fixed in next commit".
   - No migration the code at this commit can't run with.
4. **Fully tested.** Tests live **in the same commit** as the behaviour they cover:
   - New behaviour comes with tests for it, including edge cases and error paths.
   - A bug fix comes with a regression test that fails without the fix.
   - A refactor gets no new tests, but existing tests must cover the code it touches. If they don't, add characterization tests in a **preceding** commit.
   - Never put "tests" in a separate commit after the code.

## How to slice: ordering

Each commit may depend only on commits below it, and should sit on the lowest change it needs. A change that doesn't need the one before it starts a separate stack (on trunk, or on the shared base it needs); don't chain unrelated changes just because they were made in sequence. Typical order (bottom first):

1. **Preparatory refactors** that make the change easy: extract, rename, move. These are behaviour-neutral and covered by existing tests. "Make the change easy, then make the easy change."
2. **Missing tests** for code you're about to change (characterization).
3. **New building blocks:** types, pure functions, interfaces. Each comes with its unit tests and is unused or unreachable by users yet. That's fine; unused code is not breaking.
4. **Data/storage layer**, using the **expand** half of migrations: add the column or table, keep the old one working.
5. **Business logic** using the new pieces, behind a flag or not yet wired to an entry point if incomplete.
6. **Wiring/exposure:** endpoint, UI, config. This is the commit where behaviour becomes visible. It carries the integration or acceptance tests.
7. **Cleanup / contract:** remove the old path or flag, and drop the old column in a later stack, once deployed.

Techniques that keep every step non-breaking:
- **Parallel change (expand → migrate → contract):** add the new API next to the old one, move callers over (in several commits if there are many), then delete the old one.
- **Branch by abstraction:** introduce an interface over the old implementation, add the new implementation behind it, switch, then remove the old one.
- **Feature flag / dark launch:** merge incomplete behaviour switched off. A test covers both flag states.
- **Deprecate before removing** anything public. The removal is its own (last) commit.

## Workflow

### 1. Plan the stack before writing code
Write the slice list first: one line per commit with
- a Conventional Commit title (`feat(scope): …`, `fix: …`, `refactor: …`, `test: …`, `chore: …`)
- its parent: trunk or another slice (the lowest it needs)
- what's in it and how it is tested.

Check each slice against the four properties. In coordinator mode, the architect produces this plan. Bring it to the user when the slicing involves a real trade-off, such as a feature flag versus a long-lived stack, or migration ordering.

In jj you can create the stack skeleton up front and fill it in:
```
jj new 'trunk()' -m "refactor(x): extract Foo from Bar"
jj new -m "feat(x): add Baz with tests"
jj new <extract-change> -m "feat(x): expose Foo via /api/foo"   # needs only the extract: a second stack on the shared base
```

### 2. Implement one slice at a time, in its own change
- `jj edit <change>` to work inside a slice, or work in a fresh `@` and `jj squash --into <change> <paths> -u` when it's done.
- Notice something unrelated (a typo, an adjacent bug, a refactor you want)? **Don't fold it in.** Make it a separate change, e.g. `jj new -B <slice> -m "refactor: …"` below the slice that needs it, or a separate stack on trunk.
- Edited a lower slice? Descendants are rebased automatically. Check for conflicts (`jj log -r 'conflicts()'`) and fix them in the slice where they appear.

### 3. Verify every commit, not just the top
A green top of the stack proves nothing about the commits below it. Run the full build + test + lint at **each** commit, using a separate workspace so the main working copy and its `@` are untouched:

```bash
# run from the main repo; the check workspace is created once, next to it
CHECK="$(jj root)-check"
[ -d "$CHECK" ] || jj workspace add --name check "$CHECK"
cd "$CHECK"
jj workspace update-stale 2>/dev/null   # goes stale whenever you rewrite commits; else the loop silently checks nothing

CHANGES=$(jj log --no-graph --reversed -r 'trunk()..default@ ~ empty()' -T 'change_id.short(12) ++ "\n"') \
  && [ -n "$CHANGES" ] || { echo "FAILED: could not list the stack"; exit 1; }
for c in $CHANGES; do
  jj new "$c" >/dev/null
  echo "=== $(jj log --no-graph -r "$c" -T 'change_id.short(8) ++ " " ++ description.first_line()')"
  <BUILD && TEST && LINT> || { echo "FAILED at $c"; exit 1; }
  jj abandon @ >/dev/null   # drop any build artefacts that got snapshotted
done
```

- `default@` is the main workspace's working copy, so the loop checks your stack from inside the `check` workspace. It lists only the line under that `@`: run it once per stack (with `@` at that stack's tip), or replace `default@` with the tip.
- Use the project's real commands (see its README / CLAUDE.md / CI config). Run the **full** suite, not only tests near the change.
- A failure at commit N means N is broken, even if N+1 "fixes" it. Fix it in N: move the missing piece down with `jj squash --from <N+1> --into <N> <paths> -u`, or reorder. Then re-run the loop from N. If `--from` ends up empty, jj abandons that change; that's intended if it was only a fix-up.
- In coordinator mode, the verifier runs this loop and the coder does not commit "fixes" on top.

### 4. Review each commit as the reviewer will see it
For each change run `jj show <change> --stat`, then `jj diff -r <change> --git`, and ask:
- Is the title accurate, with no "and"?
- Is it under the size budget?
- Are there unrelated hunks: formatting noise, stray debug output, commented-out code?
- Is every behaviour change covered by a test in this commit?
- Does the body say **why**? The diff already says what.

Fix with `jj split`, `jj squash`, `jj rebase -r` or `jj describe` (non-interactive forms are in the `jj` skill).

### 5. Hand off
Report the stack (`jj stack`): change ID, title, size and verification result per commit. Upload (`jj upload`, each stack from its tip) only when the user asks.

## Commit message

```
<type>(<scope>): <imperative summary, ≤ 72 chars>

Why this change is needed and any context a reviewer needs:
constraints, alternatives rejected, follow-ups in later commits.
Mention "no behaviour change" for refactors.
```

- The title becomes the MR title and the body becomes the MR description, so write them for the reviewer.
- No "WIP", "fix review", "address comments" commits. Review fixes go **into** the commit they belong to (`jj edit` / `jj squash --into` / `jj absorb`).

## Red flags: stop and re-slice

- A commit is titled "… and …", "misc", "cleanup" (with logic inside), or "fix tests".
- Tests are added in a later commit than the code.
- A commit only builds together with the next one.
- One commit both reformats a file and changes logic in it.
- The diff exceeds ~400 lines and isn't purely mechanical.
- A public API, a DB column or a config key is removed in the same commit that adds its replacement.
