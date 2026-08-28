---
type: Decision
title: Revisiting threads
description: c on a thread opens it and C always starts one; the thread pane walks every thread in the file; the gutter brackets a thread's range with rounded corners; and a file-threads pane under the tree lists the file's threads, open and resolved, in step with the cursor.
resource: crates/fathomable/src/app/file_threads.rs
tags:
  - decision
  - annotations
  - input
  - rendering
---

# 0027 Revisiting threads

Status: accepted (2026-08-28)

## Context

[0013](0013-annotation-storage-and-ux.md) made writing a comment one
key: `c`. Coming back to one was three: `]c` to find it, `Space a` to
open it, `n` to reach the right one. Pressing `c` on a line that already
carries a thread started a second thread instead of opening the first,
which is almost never what the reader meant. The gutter drew every row
of a thread as the same `▎`, so where a thread began and ended — and
whether two overlapped — was not visible. And the only view of a file's
threads as a set was the workspace list of
[0025](0025-thread-list.md), which takes the document away to show it.

The user asked for revisiting to be as cheap as writing: `c` should
open what is there, the file's threads should be in sight while the
file is, and the gutter should show a thread's extent. Settled in a
question round on 2026-08-28; the choices are recorded below.

## Decision

### `c` opens, `C` starts

- `c` with no selection on a row that carries a thread opens the
  thread pane on it, as `Space a` does. `c` with a selection, or on a
  row without a thread, starts a new thread as before.
- `C` always starts a new thread, on the selection or the cursor line,
  so a second thread on annotated lines is one deliberate key away.
  Going straight into a reply from `c` was rejected: the thread may be
  off screen, and it should be read before it is answered.

### The thread pane walks the file

- `n` and `p` in the thread pane step through every thread of the file
  in line order (start line, then the order the store holds them),
  not just the threads on the cursor row. Each step moves the view
  cursor to the thread's first line, so the pane and the text agree.
  The header reads `thread 3/7` for the file; a file with one thread
  reads `thread` as before.
- The pane opens on the first thread of the cursor row when there are
  several; the rest are one `n` away. The `+1` badge considered for
  rows that start more than one thread is subsumed by the count.

### The gutter brackets a range

- The note cell draws a thread's rows as a rounded bracket: `╭` on its
  first row, `│` between, `╰` on its last row, and `•` for a thread on
  a single row. A rendered row that folds several source lines (a
  wrapped paragraph) holds a thread on one row when both its ends fall
  in that row.
- The colour is the most urgent state on the row, as before.
- Threads that overlap share the one cell. The row's glyph comes from
  the thread that starts or ends there with the shortest range, so a
  nested thread's `╭` sits under the outer thread's `│` and its `╰`
  over the `│` that resumes; a one-row thread inside a range
  interrupts the line with `•`. When a bracket and a dot fall on one
  row the bracket wins: a range's ends are what make it legible, and
  the dotted thread is still listed in the file-threads pane and one
  `n` away in the thread pane. Two threads that overlap without nesting
  draw the same as a nested pair; the pane disambiguates. A second
  gutter lane was rejected as width spent on a rare case.

  ```text
    3 ╭ fn foo() {          outer starts
    4 │   let a = 1;
    5 ╭   if a {            nested starts
    6 │     bar();
    7 ╰   }                 nested ends, outer continues
    8 │
    9 •   baz();            one-row thread inside the outer
   10 │
   11 ╭   return a;         nested starts
   12 ╰ }                   both end
  ```

### The file-threads pane

- A pane along the bottom of the tree column lists every thread of the
  current document, open and resolved, in line order. It is drawn only
  when the tree is shown and the document has threads, so an
  unannotated file costs the tree nothing. A pane in the text column
  was rejected: the thread pane already lives there, and this one is a
  glance, not a reading surface.
- It takes as many rows as it has entries plus its rule and header,
  capped at a third of the column; its top rule is draggable
  ([0007](0007-key-grammar-and-mouse.md)) and the height is kept for
  the session.
- The header reads `threads 2/3` (open of total). Each row shows the
  range, a status glyph in its gutter colour (`●` open, edited, or
  detached; `✓` resolved or auto-resolved, dimmed), the first line of
  the comment cut to the width, and at the right edge the reply count
  and the age of the last activity (`↩2 5m`); a narrow column drops
  the tail first.
- The highlighted row is the thread under the view cursor — the first
  on the cursor row, else the nearest thread above it, else the first
  in the file — so moving through the text moves the highlight. The
  pane has no selection of its own: the cursor is the selection, which
  is why it needs no state to keep in step with reloads or scope
  changes. It scrolls to keep the highlighted row visible.
- The pane takes focus on a click or `Space t` (the tree is shown
  first if it was hidden; a file without threads says so instead). The
  pill reads `FILE`. `j`/`k` (and the wheel over the pane) move the
  view cursor to the next or previous thread, wrapping as `]c`/`[c`
  do; `Enter` opens the thread pane on the highlighted thread and
  focuses it; `r` replies and `x` resolves or reopens it in place;
  `Esc` returns focus to the text. A click on a row opens the thread
  pane on that thread, as `Enter` does; a click on the header only
  focuses the pane. The tree's `j` never crosses into the pane: the two
  are separate pages of the column.

## Consequences

- `Focus` gains a `FileThreads` variant and `Border` a `FileThreads`
  rule; `Space t` joins the menu and `C` the view keys. The pane's
  layout and key handling live in `app/file_threads.rs`, which this
  record backs; `ui` draws it under the tree with the sidebar's
  styles, and the tree's rows and scroll-off shrink by the pane's
  height.
- The `ThreadPanel` no longer holds the ids of one row; it holds the
  shown thread, and its position is computed from the document's marks
  so a reload cannot leave it pointing past the end.
- 0013's gutter `▎` and its "`n`/`p` switch between threads on the
  row" are superseded; `docs/guide.md` gains the new keys and the
  glyph legend in the same change.
