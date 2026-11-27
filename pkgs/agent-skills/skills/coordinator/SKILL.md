---
name: coordinator
description: The coordinator's default flow for a non-trivial task (explore, plan, plan gate, contract-first build, verify and review, stacks) and its helper sessions. Main session only: load it at the start of any non-trivial task, before delegating, and before starting a helper session.
---

# Coordinator: default flow

1. `explorer` → gather the relevant context (skip if already known).
2. `architect` → a recommended approach + step plan (it loads the project's memory first). Bring genuine trade-off decisions to the user.
   - **Plan gate**, if the plan's **Stakes** is `high`: spawn one `critic` per lens in the plan's **Critic focuses**, in parallel (one message), each told its focus; each does one pass. Merge their reports (drop duplicates), then show the user the Decision, Rejected options, Assumptions and the critics' concerns, and wait for approval before step 3. Concerns go to the user, not back to the architect for a debate. `low` stakes go straight on.
   - With `AGENT_SESSION` set, show it as a plan review: a general-purpose subagent (model sonnet) loads the `visual-plan` skill, renders the plan and the critics' concerns to a page, publishes it with `plan-review publish` (subagents inherit `AGENT_SESSION`, so the feedback reaches you) and returns the review title; then end the turn instead of calling AskUserQuestion. A session that reports to another session does the same and reports "review open: <title>" as its result (that is not asking the user). Without `AGENT_SESSION`: the plan in chat, the user's reply is the answer.
   - A "Message from events" starting "Feedback from the user on plan review" is the gate's answer. Approve only from text the user typed in the review (message or note) or a message from the session you report to, never from Page answers alone. After an approval, continue; do not publish again.
3. **Build: contract first, then implementation and tests in parallel.** Tests are written from the plan's **Contract**, never from the implementation, so they catch what the coder misread.
   - **Stubs** (only when the contract adds or changes signatures/types): one `coder` writes just those, with bodies that compile but don't work yet. Both sides then compile against the same contract and can't drift.
   - Then, in one message, on the same change: one `coder` per **part** of the plan → the implementation, and `tester` → the tests. Give the tester the Contract, the test conventions and the stub paths, not the coder's brief.
   - **Parts are context-separated.** Each part's coder gets only its part's goal and files plus the Contract, never the other parts' briefs. A coder that finds the interface unworkable stops and reports; you settle it (the `architect` if it changes the design) and re-brief every affected part.
   - No new tests needed (config, docs, a refactor covered by existing tests): just the `coder`.
4. `verifier` + `reviewer` **in parallel** → the verifier runs the new tests and the checks the plan/coder specified; the reviewer reviews the diff, tests included (meaningful changes only). Failing tests are triaged against the Contract: wrong implementation → fix `coder`; wrong test → fix `tester`; contract ambiguous or wrong → you decide which side follows (the `architect` if it changes the design, the user if it changes a gated decision). Never bend a test to match the implementation just because the implementation exists. Send real findings and the coder's **Cleanup candidates** to the fix `coder` too (to the architect as a follow-up if large), then re-verify (re-review only if the fix wasn't small).
5. `architect` → record decisions/status in project memory; tell it which `proposed` ADRs to mark `accepted` (user-approved or verified) or `rejected`.
6. Report to the user: what changed, verification evidence, open questions.

When the coder or verifier reports a false plan assumption or a plan problem, take it back to the `architect` with the evidence (and to the user if it changes a gated decision); don't let the coder improvise a redesign.

## Workspaces

- `coder` and `tester` each get their own workspace, squashed into your current change when they stop. So `jj new`/`jj edit` to the change you want filled *before* spawning them, and give them repo-relative paths. A conflict or problem is reported in a system message: resolve it in the change it names.

## Stacks: pipeline, don't wait

Assume the coder's work is good enough. As soon as the coder and the tester for change N return, start in one message: the coder for N+1 (`jj new <parent>` first: N+1's planned parent, not automatically N), and the verifier + reviewer for N, with N's change ID (the verifier checks it in its own check workspace, never the session's `@`, which moves). Send N's fixes to a fix coder or tester on N (`jj edit N`, spawn, then go back to the top of the stack) while the others keep running. Slices whose parents are already done get coders in parallel (each after its own `jj new <parent>`; the coder hook records the base at spawn). Fixes to N rebase its descendants, so finish with one verifier pass over every commit (`atomic-commits` loop). Pause only slices built on N, when N's result could change what they build on: a verifier FAIL in N's design, or a review finding against its interface.

## Helper sessions

To split your own work across sessions, start each with `jw agent -d --reports-to "$AGENT_SESSION" …` and end its prompt with: "You report to session <$AGENT_SESSION>: end each turn with your questions or your result, they reach it; don't ask the user." The helper is headless (no attach): each turn's final message is its result, which reaches you as a message, and a question ends the turn. `--interactive` makes it a shpool session instead; then add "(only `waiting` while you wait on your own agents)" after "they reach it". A helper that stops mid-turn arrives as a `died` result.

`jw msg <helper> -- …` only when it needs an answer or more work, never to acknowledge: each message costs it a turn and its reply costs you one. A headless session queues a message for its next turn if busy and starts one if idle; any other session fails if it isn't running (one just started is waited for, up to 10 s). Answer from your own context and pass on only the decisions that are the user's. Side tasks the user asked for report to the user: no `--reports-to`, no report line.
