---
type: Decision
title: The thread list
description: Space A opens every thread on the current work, open then resolved, grouped by file, in place of the document; Enter jumps to one, and the file picker it replaces is gone.
resource: crates/fathomable/src/app/thread_list.rs
tags:
  - decision
  - annotations
  - input
  - rendering
---

# 0025 The thread list

Status: accepted (2026-08-27)

## Context

[0024](0024-workspace-sessions.md) made a thread belong to the commit it
was written against and hides every thread the current `HEAD` cannot
reach. The viewer, however, still only shows threads one file at a time:
the gutter marks, `]c`/`[c`, and the `Space A` picker of
[0013](0013-annotation-storage-and-ux.md) are all scoped to the open
document, and the picker shows one line per thread. There was no way to
see the review as a whole — what is still open on this work, across every
file, and what has been settled — without paging through the tree.

The user asked for that view to be prominent: not a popup, but a surface
that takes the column the way the document does. Settled in a question
round on 2026-08-27; the choices are recorded below.

## Decision

### Placement

- `Space A` opens the **thread list** in place of the document. It takes
  the whole text column, full height; the tree stays alongside and keeps
  paging; the status line's pill reads `THREADS`. The list is not a
  document: it joins neither the open-file history nor the recent list.
- A pane along the bottom (as the thread pane) was rejected as too cramped
  for dozens of threads; a full-screen overlay was rejected because the
  tree should remain usable beside it.
- Opening the list closes the thread pane. `Esc` closes the list and
  returns to the document that was showing. Opening a file by any route
  — the tree, the picker, `[o`/`]o`, an agent `open` — closes it too.

### Contents

- The list holds every thread the current `HEAD` shows (0024's scope), or
  only the current file's when the file filter is on. `f` toggles the
  filter; the header reads `threads: workspace` or `threads: <path>`.
  The workspace is the default because the list exists to show the whole
  work.
- Two sections, **open** first (open, edited, and detached threads), then
  **resolved** (resolved and auto-resolved), each headed by its count.
  Inside a section, files in path order, each headed by its path, and
  threads in line order. Resolved entries are drawn dimmed.
- Every entry shows the thread in full: a header with the range, the
  status in its gutter colour, and the age of the last activity, then the
  comment and each reply as the thread pane renders them (author, age,
  `[proposes resolving]` badge, indented body). `z` folds the selected
  entry to its header and unfolds it; `Z` folds and unfolds the resolved
  section. Nothing is folded when the list opens.
- Ranges and statuses come from the loaded document's marks when the file
  is open this session, so an edited or detached thread reads as such;
  a thread on a file that has not been opened shows its stored range.

### Keys and mouse

- `h`/`l` move between entries and select the newest message in each;
  `j`/`k` move between the selected thread's messages (amended
  2026-08-30). `gg`/`G` jump between entries, and `Ctrl-d`/`Ctrl-u` move
  by half a page. The selected message's author and body rows are drawn
  in `ui.picker.selected`; a folded thread highlights its header instead.
  The list scrolls to keep the whole selected message visible when it
  fits, or its first row visible when it does not.
- `Enter` opens the entry's file at the thread's first line, opens the
  thread pane on the selected message, and closes the list. `Space A`
  again reopens the list on the same entry and message, with the same
  filter and folds, for the rest of the session.
- `e` opens the selected user-authored message in the comment editor;
  agent-authored messages are read-only (amended 2026-08-30). `r` replies
  through the same box; the list stays on screen above it, as the thread
  pane does, the submitted reply becomes the selected message, and focus
  returns to the list when the box closes. `x` resolves an open entry or
  reopens a resolved one; the entry moves to the other section and stays
  selected.
- The wheel scrolls the list three rows per tick; a click selects the
  message under the pointer, or the entry when its header was clicked.
  The list takes no border drag.
- The list follows the store: a reply from another viewer or a headless
  `--mcp` (0024's watcher) redraws it with the selection kept by thread
  id; a `HEAD` change re-applies the scope.

### What goes away

- The `Space A` **picker** over the current file's threads is removed,
  with `PickerKind::Threads`. Its one-line rows are subsumed by the
  file-filtered list, and it was the only picker whose rows were not
  paths. `]c`/`[c` and `Space a` are unchanged.

## Consequences

- `Focus` gains a `Threads` variant; the pill, the key dispatch, and the
  mouse routing branch on it. The list's state includes the selected
  thread and message; its state and row model live in
  `app/thread_list.rs`, which this record backs, and `ui` draws the rows.
- The list's rows are computed from the store on every draw and key, not
  cached, so there is no list state to invalidate on reload. The cost is
  one pass over the store's threads, which the picker already paid.
- 0013's "`Space A` opens the picker over the file's threads" is
  superseded; 0012's picker section loses its threads variant.
- `docs/guide.md` gains the list keys and loses the picker line in the
  same change.
