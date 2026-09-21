---
type: Decision
title: Direct workspace navigation
description: Shift-arrow and HJKL traverse comparison changes and changed files; Tab and Shift-Tab traverse open threads; the File footer teaches both loops.
resource: crates/fathomable/src/app/navigation.rs
related_resources:
  - crates/fathomable/src/app/placement.rs
  - crates/fathomable/src/app/threads/peek.rs
tags:
  - annotations
  - decision
  - git
  - input
  - rendering
---

# 0090 Direct workspace navigation

Status: accepted (2026-09-18)

Amended later 2026-09-18: `Shift-Up`/`Shift-Down` join `K`/`J` on the
comparison-stop cycle. `Shift-Left`/`Shift-Right` and `H`/`L` add a changed-file
cycle that always lands on the destination's first diff. The grouped footer
legends are `diffs ⇧arrows/HJKL` and `threads (⇧)Tab`.

Amended 2026-09-20: comparison stops retain complete hunk identity and place
the first rendered changed row at the top third of File. Thread traversal is
surface-specific, seats the newest message, and temporarily reveals folded
destinations without changing persistent folds.

Amended later 2026-09-20: main Threads reserves `Tab`/`Shift-Tab` for direct
open-thread traversal and uses `j`/`Down` and `k`/`Up` for a continuous
visible review walk through messages, folded threads, and file rows. Its old
`h`/`l` message bindings become directional folding: `h`/Left collapses the
selected thread or file-group header and `l`/Right expands it. File retains ordinary
cursor movement.

Amended later 2026-09-18 by [0091](0091-pane-focus-navigation.md): the four
normal panes are now named File, File list, Threads, and Thread list.
References below to Files mean File list, and references to Reviews mean the
main Threads surface. The Review menu and review workflow retain their names.

Supersedes the bracket-prefixed comparison and thread traversal keys in
[0017](0017-git-status-navigation.md) and
[0046](0046-one-thread-cursor.md). It amends the binding, footer,
comparison, and worktree contracts in [0045](0045-bindings-are-data.md),
[0067](0067-the-texts-key-bar.md),
[0069](0069-the-diffs-keys-on-the-bar.md),
[0070](0070-one-workspace-many-worktrees.md), and
[0087](0087-global-comparisons-and-board-history.md).

## Context

Fathomable used eight bracket-prefixed sequences for two review loops:
next and previous hunk, changed file, file-local thread, and workspace
thread. The split made the main workflow harder to discover and asked the
reader to decide whether the next useful item was still in this file.
The File footer also disappeared on an ordinary file, so the action that
starts review, `c`, had no persistent reminder.

Editors disagree on exact keys but converge on repeatable next and previous
actions that cross file boundaries. Fathomable needs two such loops: what
changed in the selected comparison, and which open discussion needs review.

## Decision

### Direct workspace traversal

`Shift-Down` or `J` moves to the next comparison stop and `Shift-Up` or `K`
to the previous one. Stops are the comparison's ordered text hunks, not
rendered rows. Distinct hunks remain distinct when Markdown layout maps them
to the same row. Every changed path with no text hunk contributes one
path-level stop, including mode, type, binary, unsupported, and
unavailable-content changes.

`Shift-Right` or `L` moves to the next changed path and `Shift-Left` or `H`
to the previous one. Changed-file traversal always focuses the first text
hunk in the destination file; a hunkless changed path focuses its path-level
stop. Both comparison traversals follow path order, wrap, and focus File.
Off reports that diff mode is off; an empty comparison reports that it has
no changes.

`Tab` moves to the next open review thread and `Shift-Tab` to the previous
one. Both terminal encodings of Shift-Tab map to the same action. The base
order is projected path, file-wide thread before line threads, projected
line, then stable thread id. Open means active or resolution-proposed;
resolved and archived threads do not participate.

The candidate set and destination surface belong to the navigation owner.
File and File list traverse workspace-wide open threads and land in File.
Main Threads traverses only open threads admitted by its current view and
filters and remains in Threads. Thread list traverses the open threads
admitted by its pane scope and filter, retains pane focus, and previews the
current main surface. When that target is outside the current main Threads
view, main Threads stays unchanged and reports
`thread is outside the current Threads view; press Enter to open`.

The six actions apply in normal File, File list, Threads, and Thread list.
Drafts, pickers, and input lines keep precedence. The retired `]g`/`[g`,
`]G`/`[G`, `]c`/`[c`, and `]C`/`[C` sequences have no aliases.

Every hunk destination carries its exact hunk index and complete old and new
ranges, including hunks sharing one target line. After layout is current,
`J`/`K`, shifted arrows, Go actions, and the first-hunk `H`/`L` routes put
the first rendered changed row at
`floor((body height - 1) / 3)`, clamped only by document bounds. Unified
pure deletions use the first removed row; an entirely deleted Normal file
uses the first old-side line displayed below its deletion banner.

### Source when available, evidence otherwise

File and File-list traversal first asks the selected-version document
machinery to show the thread's projected source. When that source and mark
can be displayed, File opens, the thread is revealed, and File takes focus.
The rendered span from visible source, detached marker, or file-wide anchor
through the newest reply is centered when it fits. On overflow, the newest
reply's author/header/start is centered. Both File's logical thread cursor and
its actual text cursor sit on that newest reply, including a one-thread
wrap.

Deleted, binary, oversized, or otherwise undisplayable source falls back to
the exact expanded Threads entry and its immutable evidence. Navigation
never changes the active worktree or comparison endpoint implicitly.
`]w` and `[w` remain the explicit worktree cycle.
That explicit evidence destination takes precedence over the cursor retained
by the File surface being replaced, including when Enter first closes main
Threads to attempt File. The fallback reopens main Threads on the requested
thread and its requested message rather than inheriting File's prior cursor.

Main Threads applies the analogous fit/overflow placement from immutable
origin context through the newest reply. Navigation-owned peeks override a
selected thread's or containing file's effective fold only while needed.
They do not mutate persistent fold sets or hidden-stub preference. A
successful later destination, thread scope or filter change, or worktree
switch dismisses the prior peek; a failed or empty navigation attempt does
not. An explicit keyboard, Enter, mouse, or menu fold action takes ownership
of the effective state and cannot later be undone by that peek.

The jumplist records both source positions and exact Reviews entries, so
`Alt-Left` and `Alt-Right` return across a fallback without confusing two
threads on the same source line. A File position on a projected Normal
deletion also retains the hunk ranges, Base line, and within-line seat.
Restoration reconstructs that projection before seating the cursor, while an
ordinary Target source position keeps its existing line-based behavior. No
raw viewport scroll is stored.

A main Threads peek retains its selected message independently from the
Thread-list cursor. Main-surface message movement updates that logical message;
resize and delayed relayout clamp it to the current conversation and recompute
the viewport from it. A rejected Thread-list preview can therefore move its own
selection without changing the message or viewport retained by main Threads.
Actions and jumplist capture resolve from the surface that dispatches them, so
focus returning to main Threads cannot target the rejected sidebar preview.

Every successful comparison or thread landing also asks Files to reveal the
destination path. Files expands listed ancestors and centers the destination
when it has enough rows, but never takes keyboard focus from File or Reviews.
The destination is retained while Files is hidden, so showing it later applies
the same reveal. On a Reviews fallback, the destination remains the thread's
path even though File restores the previously displayed source.

Files filters remain authoritative. If they exclude the destination, navigation
does not fabricate a row or bypass a rule: the existing Files highlight stays
where it is. The remembered destination is retried when the listing changes or
Files reopens.

### A persistent, compact File footer

An ordinary text File always keeps its bottom key bar. Its default loop is:

```text
comment c · diffs ⇧arrows/HJKL · threads (⇧)Tab
```

`comment c` appears only while a new comment can be persisted. On a thread
row it becomes `reply c`. `diffs` appears only when the selected comparison
has a stop, and `threads` only when an open thread exists. Immediate thread
actions precede traversal hints, so narrowing drops the workspace loop
before the local action.

Where both thread-fold actions apply, one paired hint reads `folding z/Z`,
with a click target for each key. The compact traversal legends are grouped
mnemonics; the Go menu exposes each traversal action as a separate mouse
target. No standalone `Z` hint is shown away from a thread, and `y` remains
an undisplayed power-user action.

Unified and Normal use the same comparison-navigation hint. Endpoint and
whitespace controls remain in the Diff menu and commands rather than
occupying the File footer. The Go menu exposes all six direct traversal
actions for mouse use, subject to the same availability rules.

## Consequences

- `app/navigation.rs` owns exact-hunk and path-level comparison stops.
- The binding table, dispatch, help, menus, and hints share six direct
  actions; terminal Shift-Tab normalization and shifted-arrow retention
  happen at event conversion.
- Open-thread traversal uses the focused surface's candidate policy, never
  activates another worktree, and uses immutable Threads evidence only when
  File cannot display the source.
- Files mirrors a listed traversal destination without taking focus; hidden
  state and temporarily filtered destinations retain the reveal target.
- The jumplist distinguishes Target File lines, exact projected Base seats,
  and exact Threads messages, restoring each through reconstructed logical
  state rather than saved viewport coordinates.
- The File footer is persistent for ordinary text documents, while draft,
  file-info, directory, and Reviews surfaces retain their own presentation.
- The guide teaches the same two workspace loops and the footer's compact
  folding vocabulary.
