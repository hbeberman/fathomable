---
type: Decision
title: Threads fold in the list
description: In the review list every thread folds and expands as it does in the text, with the same chevrons, `z`, `Z`, clicks, and menu entries, and a folded thread is one packed row; file rows and folded threads are stops for `j`/`k`, and `z` acts on the row the cursor is on; `Z` folds or expands every thread and no longer touches files; a file row always carries its `▾` or `▸`; and in the text a stub is a stop too.
resource: crates/fathomable/src/app/threads/list_fold.rs
related_resources:
  - crates/fathomable/src/app/threads/list.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/draw/header.rs
  - crates/fathomable/src/app/draw/threads_pane.rs
  - crates/fathomable/src/app/input/mouse.rs
  - crates/fathomable/src/app/input/menu.rs
  - crates/fathomable/src/app/threads/stubs.rs
tags:
  - decision
  - annotations
  - input
  - rendering
---

# 0076 Threads fold in the list

Status: accepted (2026-09-11)

Amended 2026-09-16 by
[0086](0086-one-thread-summary-and-its-actions.md): review-list folding,
stops, file rows, and nesting remain, but expanded and folded thread rows
now use the shared summary facts and layout. Expanded rows expose direct
mouse actions; the cursor's folded row may trade preview width for the same
actions. Reply count, compact modification time, detached `?` suffix, and
current lifecycle replace the historical packed-row state wording below.

## Context

In the text a thread has two states and a full set of controls
([0049](0049-inline-threads-and-the-rail.md),
[0065](0065-z-folds-and-unfolds.md), [0073](0073-the-chevron.md)): a
folded thread is a one-row stub with a bold `▸`, an expanded one shows
`▾` on its header, and `z`, `Z`, `c`, a click on the chevron, a
double-click on the row, and the menu's `expand thread` / `fold
thread` move between them.

In the review list ([0066](0066-one-circle-language.md)) every thread
was always expanded, with every message under its header and a blank
row after it. The only fold was per file: `z` folded the cursor's
file, `Z` every file, and a click on a file row folded it. The file
row drew `▸ ` only while folded and nothing while open, so the path
slid two cells as the file folded and unfolded. And the text's `j`/`k`
stepped over a collapsed stub, so the only way onto one was a click.

The user asked on 2026-09-11 for the same thread controls in the list
as in the text, for the file rows to keep their arrow in both states,
and for a round of suggestions. They settled: `z` acts on the row the
cursor is on; `Z` folds or expands every thread and there is no
fold-all for files; the list opens with every thread expanded and
remembers what was folded for the session; folded threads pack with
no blank row; and a folded thread is a stop for `j`/`k`, in the list
and in the text alike.

## Decision

### A thread in the list folds

- **Two states, as in the text.** Each entry of the list is expanded
  or folded. Expanded is the header row and the messages, as before,
  with a blank row after. Folded is one row, the stub's form: the
  cursor cell, `▸` in the chevron's tone, the circle, the place
  (`L14-16` or `file`, and the branch or commit note after it when
  there is one), the newest message's author as `author_label` names
  them, its short age, then the first line of that message cut with
  `…`. No blank row follows a folded thread, so a folded file's
  threads read as a packed list.
- **The header's chevron.** An expanded entry's header draws `▾` in
  the cell after the cursor cell, bold in the info colour, so the
  chevrons of a folded and an expanded thread line up under one
  another as they do in the text.
- **Default and memory.** The list opens with every thread expanded.
  What the reader folds stays folded while the viewer runs, across
  closing and reopening the list and across `x` and `f`, as the file
  folds already did. The set is the list's own; the sidebar threads
  pane has no thread fold.

### File rows are stops, and `z` acts on the row

- **A file row is a stop.** `j`/`k` in the list stop on a file row,
  then on each of its threads when it is unfolded, folded threads
  included. A folded file is its row alone, one stop. The cursor's
  thread while the cursor rests on a file row is the file's first
  thread, as it was inside a folded file, so `Enter`, `c`, `o`, and the
  hints keep acting on a thread; `l`/`h` do nothing on a file row or a
  folded thread, since there is no visible message to walk, and the
  bar drops the `messages` hint there. `gg` lands on the first stop
  and `G` on the last.
- **`z` acts on the row the cursor is on.** On a thread's rows it
  folds the thread or expands it. On a file row it folds the file or
  unfolds it; the cursor stays on the file row. `f`, which drops the
  file rows, leaves `z` its thread meaning.
- **`Z` folds or expands every thread.** When any listed thread is
  expanded, `Z` folds them all; when every one is folded, it expands
  them all: the case rule of [0065](0065-z-folds-and-unfolds.md) and
  the text's `Z`. `Z` does not touch the file folds, and the list has
  no fold-all for files: the file menu loses `fold all` / `unfold
  all` in the list and keeps them in the threads pane, where `Z` still
  folds every file, there being no thread fold there.
- **The key bar.** The list's bar reads `fold z · fold all Z` in file
  scope too, since both now work there.

### Mouse and menu

- **The chevron and a double-click.** A click on the chevron cell of a
  thread's header or its folded row, or a double-click anywhere on
  either row within the multi-click window of
  [0050](0050-mouse-menus-and-gestures.md), folds or expands the
  thread and selects it. One click elsewhere on the row selects the
  thread as before. A click on a message row still selects the
  message. Opening and closing end the gesture, as in the text.
- **A file row.** A click folds or unfolds the file and rests the
  cursor on its row. A right-click rests the cursor there and opens
  the file's menu: `z fold` / `z unfold`, `Enter open file`, and the
  resolved toggle.
- **A thread's menu** in the list gains `z expand thread` / `z fold
  thread` at the top, as the text's has, and loses `fold file`, which
  `z` no longer does from a thread row; the file row's menu and a
  click on it fold the file.

### File rows keep their arrow

- **`▾` open, `▸` folded.** A file row in the review list and in the
  threads pane draws `▾ ` before its path while it is open and `▸ `
  while it is folded, as the files pane draws a directory, so the
  path never moves.

### A stub is a stop in the text

- **`j`/`k` stop on a collapsed stub.** Stubs are stops for every
  motion that settles on a row, as an expanded thread's message rows
  are: `j` from a line lands on the stub under it, then on the next
  stub stacked there, then on the next line. The cursor resting on a
  stub is the state a click already gave ([0073](0073-the-chevron.md),
  amended 2026-09-09): the terminal cursor hides, the stub's `▎` bar
  marks the place, the thread cursor is the stub's thread at its
  newest message, and `z` expands it. A relayout keeps a cursor
  on a stub on that stub. The expanded thread's header row stays no
  stop: `j` steps from the line above to the first message.

## Consequences

- `app/threads/list_fold.rs`, which this record backs, keeps the
  list's fold state and its `z`, `Z`, and toggles for files and
  threads; `list.rs` gains a `Row::Stub`, stops that are rows rather
  than entries, and the cursor's rest on a file row.
- `draw/mod.rs` draws the stub row and the arrows on file rows;
  `draw/header.rs` gives the entry header its chevron and the list's
  bar its hints; `draw/threads_pane.rs` draws the pane's arrows.
- `input/mouse.rs` reads the chevron cell and the double-click on the
  list's thread rows; `input/menu.rs` reshapes the list's thread and
  file menus; the bindings' words for the list's `z` and `Z` change.
- `app/threads/stubs.rs` marks a collapsed stub's row a stop.
- 0049's walkable-rows rule, 0065's stub note, 0066's review list and
  mouse sections, and 0073's press rule carry dated notes pointing
  here; `docs/guide.md` describes the list's folds, the arrows, and
  the stubs as stops.
