---
type: Decision
title: The thread list
description: Reviews opens in place of the document, groups repository threads by file, exposes filters through its title and key bar, and opens a selected thread back in its file.
resource: crates/fathomable/src/app/threads/list.rs
tags:
  - decision
  - annotations
  - input
  - rendering
---

# 0025 The thread list

Discovery amended 2026-09-21 by
[0094](0094-diff-range-thread-discovery.md): normal lists follow the accepted
comparison's commit range and share an **All threads** backstop with the
sidebar. Bare `A`, `Space T A`, and checked title/Review menu items control
the override; file and resolved-status filters remain independent.

Status: accepted (2026-08-27); amended 2026-09-05 by
[0066](0066-one-circle-language.md): the list is always by file then
line under file rows, `s` is gone, `z` folds the cursor's file and `Z`
every file, and the header counts by colour; amended 2026-09-03 by
[0046](0046-one-thread-cursor.md): the list's selection is the shared
thread cursor, it no longer remembers a selection across close and open,
`Ctrl-d`/`Ctrl-u` move by half a page of rows, `PgUp`/`PgDn` are gone,
and `o` resolves. Amended 2026-09-04 by
[0049](0049-inline-threads-and-the-rail.md): the list is the **review list** (pill
`REVIEW`), opens sorted by newest agent reply first with `s` toggling
to file and line order, hides resolved threads until `x` shows them
(`Z` is gone), carries the path in every entry header, `f` narrows to
the current file, and `Enter` opens the file with the thread expanded
in place since the thread pane is gone.

Amended 2026-09-17: the normal view's header is `Reviews` at the left, with
passive responsive scope and lifecycle counts at the right. Clicking the
title opens checked **Only current file** and **Show resolved** settings
below the header. Bare `t` opens and focuses Reviews; it no longer closes an
open view, while `Esc` still returns to the document.

Amended later 2026-09-17: bare `f` opens File view, so the normal board's
file/workspace scope moves to `s`. The Reviews title menu begins with
**Open File**, followed by a separator and its checked settings. Paging the
Files pane changes the current file without closing Reviews; explicit Open,
Go to, `f`, or `Esc` returns to File view.

Selection superseded 2026-09-14 by [0079](0079-list-focus-language.md):
review entry rows use shared active/remembered styles; message author
and body rows retain author stripes with a focus-aware selection bar.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

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
  returns to the document that was showing. Explicit file navigation — the
  picker, `[o`/`]o`, an agent `open`, a row-menu **Open**, or `Enter` —
  closes it too. Passive highlight paging in the Files pane instead changes
  the current file behind the open list.

### Contents

- The list holds every thread the current `HEAD` shows (0024's scope), or
  only the current file's when the file filter is on. `s` toggles the
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
  `j`/`k` move between the selected thread's messages (swapped on
  2026-09-04, [0049](0049-inline-threads-and-the-rail.md); amended
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
  `app/threads/list.rs`, which this record backs, and `ui` draws the rows.
- The list's row model is computed from the store on every draw and key, so
  placement, lifecycle, folds, and selection need no list invalidation.
  Rendered message bodies are different: Markdown parsing and fenced-code
  highlighting are retained by thread revision and effective body width in
  the shared message-layout cache also used by inline threads. Rebuilding
  rows clones those prepared lines instead of highlighting every message
  again.
- 0013's "`Space A` opens the picker over the file's threads" is
  superseded; 0012's picker section loses its threads variant.
- `docs/guide.md` gains the list keys and loses the picker line in the
  same change.
