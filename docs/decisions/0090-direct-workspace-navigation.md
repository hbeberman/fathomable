---
type: Decision
title: Direct workspace navigation
description: J and K cycle comparison changes; Tab and Shift-Tab cycle open review threads; the persistent File footer teaches that loop.
resource: crates/fathomable/src/app/navigation.rs
tags:
  - annotations
  - decision
  - git
  - input
  - rendering
---

# 0090 Direct workspace navigation

Status: accepted (2026-09-18)

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

### Two direct workspace cycles

`J` moves to the next comparison stop and `K` to the previous one. Stops are
the comparison's ordered text hunks, not rendered rows. Distinct hunks remain
distinct when Markdown layout maps them to the same row. Every changed path
with no text hunk contributes one path-level stop, including mode, type,
binary, unsupported, and unavailable-content changes. Traversal crosses
paths in comparison order, wraps, and focuses File. Off reports that diff
mode is off; an empty comparison reports that it has no changes.

`Tab` moves to the next open review thread and `Shift-Tab` to the previous
one. Both terminal encodings of Shift-Tab map to the same action. The order
is projected path, file-wide thread before line threads, projected line,
then stable thread id. Open means active or resolution-proposed. Resolved
and archived threads do not participate, regardless of Files or Reviews
filters.

The four actions apply in normal File, Files, Threads, and Reviews panes.
Drafts, pickers, and input lines keep precedence. The retired `]g`/`[g`,
`]G`/`[G`, `]c`/`[c`, and `]C`/`[C` sequences have no aliases.

### Source when available, evidence otherwise

Thread traversal first asks the selected-version document machinery to show
the thread's projected source. When that source and mark can be displayed,
File opens, the thread is revealed, and File takes focus.

Deleted, binary, oversized, or otherwise undisplayable source falls back to
the exact expanded Reviews entry and its immutable evidence. That fallback
clears obstructing review folds and ignores presentation filters. Navigation
never changes the active worktree or comparison endpoint implicitly.
`]w` and `[w` remain the explicit worktree cycle.

The jumplist records both source positions and exact Reviews entries, so
`Alt-Left` and `Alt-Right` return across a fallback without confusing two
threads on the same source line.

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
comment c · diffs K/J · threads Shift-Tab/Tab
```

`comment c` appears only while a new comment can be persisted. On a thread
row it becomes `reply c`. `diffs` appears only when the selected comparison
has a stop, and `threads` only when an open thread exists. Immediate thread
actions precede traversal hints, so narrowing drops the workspace loop
before the local action.

Where both thread-fold actions apply, one paired hint reads `folding z/Z`.
Each key retains its own click target. No standalone `Z` hint is shown away
from a thread, and `y` remains an undisplayed power-user action.

Unified and Standard use the same comparison-navigation hint. Endpoint and
whitespace controls remain in the Diff menu and commands rather than
occupying the File footer. The Go menu exposes all four direct traversal
actions for mouse use, subject to the same availability rules.

## Consequences

- `app/navigation.rs` owns exact-hunk and path-level comparison stops.
- The binding table, dispatch, help, menus, and hints share four direct
  actions; terminal Shift-Tab normalization happens at event conversion.
- Open-thread traversal uses lifecycle independently of current presentation
  filters, never activates another worktree, and falls back to Reviews.
- Files mirrors a listed traversal destination without taking focus; hidden
  state and temporarily filtered destinations retain the reveal target.
- The jumplist distinguishes File lines from exact Reviews threads.
- The File footer is persistent for ordinary text documents, while draft,
  file-info, directory, and Reviews surfaces retain their own presentation.
- The guide teaches the same two workspace loops and the footer's compact
  folding vocabulary.
