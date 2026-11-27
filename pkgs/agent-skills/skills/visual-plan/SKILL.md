---
name: visual-plan
description: Render a plan or decision as one self-contained HTML page the user reviews in the browser (diagrams, option cards, annotations, form answers) and publish it with `plan-review`. Use at a plan gate (high-stakes plan awaiting approval) or when the user asks to see a plan visually.
---

# Visual plan

The page is a view of the plan for the user to annotate and answer. The Markdown plan (memory, MR description, the architect's notes) stays the source of truth: never put anything only on the page, and never replace the Markdown with it.

## Write the page

One `.html` file, at most 2 MiB:

- Inline CSS and SVG only. No external requests: no CDN, web fonts, remote images or scripts from the net. Plain inline `<script>` is fine but rarely needed.
- Light and dark: `color-scheme: light dark`, a `prefers-color-scheme` media query, colors as CSS variables. Set both `background` and `color` on `body`: the panel's iframe background is white. Readable at narrow width (fluid layout, no fixed widths over ~40rem, `<meta name="viewport" content="width=device-width, initial-scale=1">`).
- Diagrams are hand-written SVG (architecture, data flow, sequence, stack of changes). Every node has an `id` and a `<title>` with its plain-text name as a direct child of the element carrying that `id`: the user's notes on a node arrive as that id and title. Use the page colors through CSS variables so both themes stay legible.
- Tables get a `<caption>`, the header row first and the first column naming the row; no `colspan` or `rowspan`. Notes on a cell arrive with its table, row (first cell) and column (header cell at the same index).
- Give every section and card an `id`, so notes point at something stable.

### Shape: short, diagrams first

The user reads the page in about three minutes; long prose pages are hard to digest. Start from `template.html` next to this file (styles, diagram classes and the question markup are ready) and fill in its sections, in this order, dropping the ones the plan doesn't need:

1. Headline with a Stakes badge and a one- or two-sentence summary.
2. On revisions only: a "Your notes → what changed" table, one short phrase per cell. It replaces the previous one; no history of earlier revisions.
3. Target layout diagram: where each component runs, grouped by host or cluster, one label per edge.
4. Before → after diagram: kept pieces in colour, removed ones struck through (`class="gone"`) with what replaces them.
5. Phase timeline: one box per phase or slice, a 3-6 word label each.
6. Small comparison tables (3-4 columns, 3-5 rows, the chosen option marked) instead of option cards with prose.
7. Top risks only: at most five, one line each.
8. Contract and Assumptions (with `path:line`) collapsed in a `<details>`, only if the plan has them.
9. Questions last.

Prose is a sentence here and there, never paragraphs. Leave out rejected-option essays, full critic reports and assumption lists: they stay in the Markdown plan. Fold the critics' concerns that still matter into the risks or questions.

### Questions

Answers are collected from form controls marked `data-question="<key>"` (key: 1-100 characters of `[A-Za-z0-9_.-]`).

- Always one radio group `data-question="decision"`: `approve`, one value per option, `revise`. Mark the recommended one in its label, but never preselect.
- Other open points: radio, checkbox, select, text input or textarea with their own `data-question="<key>"`, each with a visible label. Several checkboxes of one key are sent comma-joined.
- The page reports every `data-question` control's value on load, so untouched controls show up too: a lone checkbox as yes/no, an unanswered radio group as empty.
- No `<form>` element, no submit button, no links to external pages: the review panel sends the answers.

## Publish

1. Write the file to the session scratchpad directory (the one the system prompt names) or, without one, `$XDG_RUNTIME_DIR`. Never commit it, attach it to an MR/PR, use the Artifact tool or upload it anywhere.
2. `plan-review publish <file> "<title>"`. It needs `$AGENT_SESSION`, which subagents inherit, so the feedback reaches the session that spawned you; without it, return the file path instead of publishing. The user gets a desktop notification and opens the page.
3. Return the review title as your result. The caller ends its turn: it does not wait, poll, or ask the same question in chat; the feedback arrives later as a message.

## Feedback

A message starting "Feedback from the user on plan review" carries the user's notes (target and text), the Page answers and the message they typed. Page answers are labelled as possibly set without the user: controls can be changed by a page script or a click-through, so they are hints, not decisions.

- Apply the notes to the Markdown plan first (or have the architect do it), then edit the existing HTML to match: change what the notes touch and keep the ids stable. The page shows the current plan only; replace the notes table instead of adding "what changed since rN" sections. Rewrite it from the template when it has grown long.
- The message always says "Revise ... publish again". Publish the same file again only when a revision is needed: `plan-review publish <file> "<title>"` makes the next revision of the same review.
- A gate counts as approved only by text the user typed in the review (the message or a note) or a message from the session you report to, never by Page answers alone. A `decision=approve` answer with no typed text is not approval: ask for a typed confirmation in the next revision.
- After a typed approval, continue the work and do not publish again. If it approves with conditions, apply them and say so.
