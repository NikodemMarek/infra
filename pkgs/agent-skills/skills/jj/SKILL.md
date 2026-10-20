---
name: jj
description: How to work with Jujutsu (jj) version control non-interactively, and the user's stacked-MR workflow (`jj upload` → one MR/PR per change, on GitLab or GitHub). Use whenever the repo has a `.jj` directory (`jj root` succeeds), or the task involves jj, commits/changes in a jj repo, stacking, rebasing, squashing, splitting, describing, reordering or dropping changes, pushing, bookmarks, merge requests, or addressing review feedback.
---

# Jujutsu (jj)

jj 0.45, Git backend, remote on GitLab or GitHub (`jj upload` picks the forge from the remote host, or `upload.forge`). Config: the Nix-wrapped `jj` sets `JJ_CONFIG` to its read-only store config (`jj config path --user`); `~/.config/jj/config.toml` is not read and `jj config set --user` fails. Per-repo config (`jj config set --repo`) works.

**Is this a jj repo?** `jj root` succeeds or `.jj/` exists → use `jj` for **every write** (commit, rebase, branch, push). A `.git/` next to it ("colocated") is fine for read-only git tools, but never `git commit/checkout/rebase/reset/stash/push` there. git sees a detached HEAD and that is normal.

## Mental model (differs from git)

- **The working copy is a commit: `@`.** Every jj command first snapshots files on disk into `@`. There is no staging area, no `git add`, no stash. Untracked files are tracked automatically unless gitignored.
- **Change ID vs commit ID.** Change IDs (letters k–z, e.g. `qkuklztw`) stay the same when a change is rewritten. Commit IDs (hex) change on every rewrite. **Always refer to changes by change ID.**
- **Rewriting is normal and cheap.** Rewriting a change rebases its descendants automatically. Bookmarks follow the rewritten commits.
- **Conflicts don't block.** A rebase that conflicts still succeeds, and the conflict is recorded in the commit (`(conflict)` in the log). Fix it later.
- **Everything is undoable.** `jj undo` reverts the last operation. `jj op log` lists operations, and `jj op restore <op-id>` jumps back to any one of them.
- **Immutable commits** (trunk and anything pushed to it) can't be rewritten. Never pass `--ignore-immutable` unless the user explicitly asks.
- **Bookmarks = git branches.** They do not move when you create new commits. `jj upload` manages them for this user (see below).

## Never open an editor or TUI (you have no terminal)

These hang or fail without a TTY, so always use the non-interactive form:

| Don't | Do |
|---|---|
| `jj describe` | `jj describe -m "msg"` (or `-r <rev> -m`) |
| `jj commit` | `jj commit -m "msg"` |
| `jj squash` when both sides have descriptions (opens editor) | `jj squash ... -u` (keep destination message) or `-m "msg"` |
| `jj split` with no paths (opens diff editor) | `jj split -r <rev> -m "msg for first part" <paths...>` — the named paths go to the first commit, the rest to the second |
| `-i` / `--interactive` / `--tool`, `jj resolve` (merge tool), `jj diffedit` | edit files directly, then a normal command |

Multi-line message: `jj describe -m "$(printf 'feat: title\n\nBody line')"`, or pipe it: `printf '...' | jj describe --stdin`.

## Everyday commands

| Task | Command |
|---|---|
| Status / what's in `@` | `jj st` |
| History of current stack | `jj stack` (alias: `trunk() \| trunk()..(@::)`) or `jj log -r '<revset>'` |
| Diff | `jj diff` (of `@`), `jj diff -r <rev> --git`, `jj diff --stat`, `jj show <rev>` |
| Finish current change, start a new one on top | `jj commit -m "msg"` (= describe + new) |
| Name the current change | `jj describe -m "msg"` |
| New empty change on top of X | `jj new X -m "msg"` |
| Insert a new change after / before X | `jj new -A X -m "msg"` / `jj new -B X -m "msg"` |
| Work directly inside an existing change | `jj edit X` (later changes are rebased automatically as you edit) |
| Move all of `@` into its parent | `jj squash` |
| Move part / all of one change into another | `jj squash --from A --into B [paths...] -u` |
| Auto-distribute `@`'s hunks into the stack changes that last touched those lines | `jj absorb` |
| Drop a change (descendants rebase onto its parent) | `jj abandon X` |
| Discard edits to a file in `@` | `jj restore <paths>` (from parent) / `jj restore --from X <paths>` |
| Move a single change | `jj rebase -r X -A Y` (after Y) / `-B Y` (before) / `-o Y` (onto, as a new branch) |
| Move a change with its descendants | `jj rebase -s X -o Y` |
| Move whole stack onto latest trunk | `jj rebase -b @ -o 'trunk()'` (or `jj sync`) |
| Fetch | `jj git fetch` |
| Undo last operation | `jj undo` |
| How did this change evolve? | `jj evolog -r X` |

Example: reorder the stack `A → B → C` into `A → C → B` with `jj rebase -r C -A A`.

### Revsets you'll use

`@` current · `@-` parent · `X::` X and descendants · `::X` X and ancestors · `X..Y` ancestors of Y not reachable from X · `trunk()` main branch head · `trunk()..@` your stack below @ · `stack()` the whole stack around @ · `mine()` · `conflicts()` · `empty()` · `description(substring:"fix")` · `bookmarks()` · combine with `|` `&` `~`.

### Machine-readable output

Use `--no-graph -T <template>` instead of parsing the default output:

```
jj log --no-graph -r 'trunk()..@' -T 'change_id.short(8) ++ " " ++ description.first_line() ++ " " ++ bookmarks ++ "\n"'
```

Useful template fields: `change_id`, `commit_id`, `description`, `bookmarks`, `local_bookmarks`, `parents.map(|p| p.change_id().short()).join(",")` (`parents` is a list, so it can't be printed directly), `empty`, `conflict`, `immutable`, `author.email()`.

## Conflicts

1. `jj log -r 'conflicts()'` shows conflicted changes, and `jj resolve --list -r X` shows their files.
2. `jj new X` (or `jj edit X`), then edit the files and remove the markers. jj's markers look like this:
   ```
   <<<<<<< conflict 1 of 1
   %%%%%%% diff from: zsqksuup 51719f56 "feat: F" (parents of rebased revision)
   \\\\\\\        to: pqomlopu 904c1e54 "feat: P" (rebase destination)
   -old line
   +side 1 line
   +++++++ yorpmywr 4acfd397 "feat: Q" (rebased revision)
   side 2 content
   >>>>>>> conflict 1 of 1 ends
   ```
   The labels after the markers vary (commit IDs and descriptions). The `%%%%%%%` section (with its `\\\\\\\ to:` header line) is a **diff** to apply to the `+++++++` side, not literal content. Write the final merged text and delete all marker lines.
3. If you used `jj new X`: `jj squash` folds the resolution into X. `jj st` should no longer list conflicts.

## The user's stacked-MR workflow

**One change = one merge request.** The commit's first line becomes the MR title and its body becomes the MR description when the MR is created; after that the forge's title and description win (see Rules). Each MR targets the branch of the change below it, and the bottom MR targets trunk.

| Command | What it does |
|---|---|
| `jj upload -n` | dry run: prints the plan (bookmark → target, title) and changes nothing; does not sync; refuses if a `remote changes on …` change exists |
| `jj upload` | runs `jj sync` first and stops if it fails; then uploads the whole stack around `@`: sets bookmarks, force-pushes, creates/updates MRs, retargets after reordering, closes MRs and deletes branches of abandoned or squashed-away changes |
| `jj upload -r '<revset>'` | uploads only those changes (the ones below them must be in trunk or already uploaded) |
| `jj upload --draft` | new MRs open as Draft |
| `jj sync` | fetches, cleans up commits the fetch revived, rebases the stack onto trunk (dropping merged changes), and reconciles each MR branch: commits that exist only on the remote are squashed into the change if it is unchanged locally since the last push; if both changed, a `remote changes on <branch>` change is put after it and sync fails; if it succeeds, pulls the title/description of open MRs into the changes |

Independent changes are separate stacks, each based on what it needs (possibly a shared base change, whose MR both stacks share). `jj upload` uploads the line through `@`, so run it with `@` at each stack's tip (or `-r` per stack).

Script: the `jj-upload` package, with the jj aliases `upload` and `sync`. It is configured under `[upload]` in the jj config.

Rules:
- **Once an MR exists, the forge's title and description are the source of truth.** `jj upload` pulls them into the commit description, overwriting local `jj describe` edits to changes that already have an MR (it only maintains the stack list in the MR description). Edit title/description on the forge; edit the commit message locally only before the first upload. Local images/videos in a description (`![alt](path)`, absolute or relative to the repo root) are uploaded to GitLab and the links rewritten when the MR is created (GitHub: not supported; missing files and failed uploads: warning, links stay as they are).
- **Bookmarks starting with `nm/` are managed** (`nm/<slug>-<change-id>`). Don't create, rename, move or delete them by hand. To drop an MR, `jj abandon` the change; the next `jj upload` closes it.
- **Every change needs a description before upload.** Use Conventional Commits (`feat: …`, `fix: …`). Each change should compile and pass tests on its own.
- **Addressing review feedback on change X:**
  - Edit in place: `jj edit X`, change the files, `jj new <top-of-stack>` to go back.
  - Or fix on top: make the fix in `@`, then `jj squash --into X <paths> -u`, or let `jj absorb` route it.
  - Then `jj upload`, and reply to the threads with `gitlab-review` (drafts; `resolve` when settled), never directly.
- **After the bottom MR is merged:** just run `jj upload` (it syncs first and retargets the rest of the stack).
- **A `remote changes on <branch>` change means sync could not reconcile the remote branch with the change before it.** Inspect it with `jj show`. If the sync message offers a squash, run `jj squash -r <T> -u`. Never discard it or push over the remote without the user's OK.
- **Always push finished work** with `jj upload`, without asking (in coordinator mode: the coordinator does it, not subagents). `jj git fetch` and `jj upload -n` are safe.

## Finishing a task in a jj repo

- Leave every change you made described. If you were working in `@`, either `jj commit -m ...` or `jj describe -m ...`.
- Report the change IDs and titles you created or rewrote (from `jj stack`), and say whether any change has a conflict.
- If something went wrong, `jj op log` + `jj undo` / `jj op restore` before attempting manual repair.
