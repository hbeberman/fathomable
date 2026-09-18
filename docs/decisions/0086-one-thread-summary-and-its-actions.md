---
type: Decision
title: One thread summary and its actions
description: Inline and review headers share one factual summary layout and direct cleanup actions, while cursor lifecycle actions live in action-first hoverable footers and the sidebar keeps two-row cards built from the same facts.
resource: crates/fathomable/src/app/threads/summary.rs
tags:
  - annotations
  - decision
  - input
  - rendering
---

# 0086 One thread summary and its actions

Status: accepted (2026-09-16)

History views and cleanup actions amended 2026-09-16 by
[0087](0087-global-comparisons-and-board-history.md): the same summary/list
engine now serves Recently resolved and Archived threads, with archive and
restore actions. `Space c a`, `Space c A`, `Space c R`, and `Space c h`
provide deliberate repository-board cleanup and history.

Review navigation amended 2026-09-17: bare `t` opens and focuses the normal
Reviews view instead of toggling it closed. `Esc` remains the explicit return
to the document.

Amended later 2026-09-17: bare `f` opens File view, and `s` owns
file/workspace scope in Reviews. The two surface title menus expose the same
mouse navigation as **Open Reviews** and **Open File**.

Amended later 2026-09-17: auto-resolve and resolve/reopen leave thread
headers for each pane's footer. Footer hints read action then hotkey and
highlight their whole clickable region on hover. Headers retain dim
`autoresolve` or `resolve proposed` status text; direct Archive and Restore
cleanup actions remain on their specific rows.

Builds on [0085](0085-thread-lifecycle-and-auto-resolve.md) and supersedes
the state words and counts of [0032](0032-placement-and-state.md),
[0066](0066-one-circle-language.md), and
[0075](0075-the-header-names-its-counts.md). It keeps the author stripes of
[0071](0071-author-stripes.md), the folding behavior of
[0073](0073-the-chevron.md), and the sidebar's compact overview role.

## Context

Expanded inline headers, folded inline stubs, review entries, and sidebar
cards independently assemble overlapping thread facts. They disagree about
field order, status words, author, timestamps, counts, truncation, and
placement. Expanded headers in particular stack redundant state words over
messages that already name their authors and ages.

The user wants one factual model everywhere without forcing every surface
into the same physical shape. Open conversations should also expose their
resolution controls to the mouse without returning action hints to a
jumble of metadata.

## Decision

### One summary, two shapes

A `ThreadSummary` supplies:

- lifecycle glyph and style;
- placement and optional worktree or historical-commit context;
- latest message author and first body line;
- reply count, excluding the opening comment;
- compact thread modification age;
- fold state and available direct actions.

One layout engine owns field order, cell-width allocation, truncation,
styled spans, hover regions, and exact hit regions for inline and review
headers. Surface code adds only nesting, selection, and cursor decoration.

The factual grammar is:

```text
<glyph> <disclosure> [latest author + preview] [cleanup actions]
    [status] [context] <file|Lx-y|Lx-y?> [reply count] <modified>
```

Collapsed headers include latest author and an ellipsized first body line.
Expanded headers omit both because the conversation below already contains
them. Reply count reads `↩n` and is omitted at zero. Modification reads
`now`, `5m`, `2h`, or `3d`, never "ago". Worktree or commit context
precedes location. Detached last-known locations append `?`.

The sidebar threads pane keeps its two-row status cards: the first row
carries glyph, place, latest author, replies, and modification time; the
second carries the latest-message preview. It consumes the same summary but
does not draw a thread disclosure control or expand messages inside the
sidebar. Enter continues to open the selected conversation in the text.

### Header status and pane actions

Inline and review headers carry facts, not lifecycle controls. When
one-shot permission is enabled, the right-aligned factual tail includes dim
`autoresolve`. Otherwise a thread with current completion intent includes
dim `resolve proposed`. These labels are passive and have no hit region.

Resolved and archived history rows retain direct **Archive** and
**Restore** cleanup actions because those actions target that specific row,
including a non-cursor row. Their displayed key follows the word on cursor
rows. At rest they keep the header surface; hover patches only that cleanup
action with `ui.list.hover`.

The disclosure arrow and its three following cells remain one padded
fold/unfold target. A cleanup action begins at its first visible character,
separator cells belong to no action, and hidden or clipped text has no hit
region. Cleanup hits take precedence over double-click folding.

Auto-resolve and resolve/reopen always live on the focused pane's bottom
key bar and target its cursor thread. Footer hints read action then hotkey:

```text
reply c · auto-resolve R · resolve r · fold z · fold all Z
```

A resolved thread substitutes `reopen r` and omits auto-resolve. Each
action's label, separating cell, and hotkey form one mouse target. Hover
patches that whole target with `ui.list.hover`; separators between actions
remain inert and unhighlighted. Hints still drop from the end when the pane
is narrow.

The factual tail is right-aligned. The preview is the first elastic field
to disappear. At narrower widths optional context, reply count,
modification time, and location drop before lifecycle status.

### Lifecycle colours and counts

Lifecycle and authorship are independent. Message names and stripes keep
`thread.user` and `thread.agent`; lifecycle uses `thread.active`,
`thread.proposed`, and `thread.resolved`. `thread.waiting` retires.

Header and directory counts partition threads:

```text
● 4 active  ◐ 1 resolution proposed  ○ 2 resolved
```

A proposal is not counted again as active. Zero counts are omitted; when
narrow, all words drop together before counts themselves drop. Aggregate
priority is resolution proposed, active, then resolved. Placement never
changes the lifecycle glyph or priority.

The status line loses waiting. It may keep a factual proposal count and
total thread count.

### Key grammar

- Bare `t` opens and focuses the full Reviews view from normal non-input
  panes; repeated presses leave it open.
- Bare `t` no longer picks a diff target or opens file-scoped sidebar
  threads; `Space d t` remains the diff-target picker.
- The old `Space r` review route retires.
- Bare `r` resolves or reopens the cursor thread.
- Bare `R` enables or disables one-shot auto-resolve.
- `Space c r` remains the pane-independent reply command.
- The old bare `o` and `Space c o` resolution commands retire.
- `Tab`, `Shift-Tab`, `]r`, and `[r` retire with waiting traversal.
- Draft, picker, search, and command input continue to receive their input
  rather than these normal-mode actions.

## Consequences

- A new summary/layout boundary replaces separate inline and review header
  formatters while the sidebar preserves its useful two-row form.
- Mouse and keyboard dispatch share footer action identities and visibility
  rules.
- Theme users replace `thread.open` and `thread.waiting` with
  `thread.active` and `thread.proposed`; exact-current configuration policy
  provides no aliases.
- The setup guide changes with the keys, tool field, lifecycle words, and
  header examples.
