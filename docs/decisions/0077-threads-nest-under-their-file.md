---
type: Decision
title: Threads nest under their file
description: In the review list and the threads pane a thread's rows sit two cells in from the column's edge, one level under the file row over them as the files pane nests a directory's children, in file scope too; and the file row whose thread the cursor is on draws the yellow cursor bar in its edge cell, as the header of the cursor's thread does.
resource: crates/fathomable/src/app/draw/nest.rs
related_resources:
  - crates/fathomable/src/app/threads/list.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/draw/header.rs
  - crates/fathomable/src/app/draw/threads_pane.rs
  - crates/fathomable/src/app/input/mouse.rs
tags:
  - decision
  - annotations
  - rendering
---

# 0077 Threads nest under their file

Status: accepted (2026-09-11)

## Context

The review list ([0066](0066-one-circle-language.md),
[0076](0076-threads-fold-in-the-list.md)) groups threads under a row
per file, and the sidebar's threads pane does the same in workspace
scope. But a file row and the thread rows under it started in the same
column: in the list the file row was the cursor cell, `▾`, and the
path, and a thread's header the cursor cell, `▾`, and the circle, so
the two chevrons stood one over the other; in the pane the file's `▾`
and a thread's circle both sat in the second cell. Nothing in the
geometry said the threads belonged to the file above them.

The list says which thread the keys act on with the `▎` bar in
`thread.cursor` on the header of the cursor's thread and on its
message ([0071](0071-author-stripes.md)), and the file row drew the
same bar only while the cursor rested on the row itself, on a file
row as a stop or over a folded file. With the cursor inside an open
file the file row said nothing.

The user asked on 2026-09-11 for the threads to read as children of
their file, and for the file the cursor is within to carry a yellow
bar as the header of its thread does. Settled in one round:

- *Which surface?* Both. The indent applies to the list and the pane,
  since it is one rule; the file row's bar is the list's, where the
  cursor bar already lives. The pane goes on marking the current file
  with the `thread.focus` tint and the cursor's entry with the selected
  surface, and gains no bar.
- *How far?* Two cells, one level, as the files pane nests a
  directory's children, so a thread's chevron sits under the first
  letter of the path. One cell would have put it under the gap after
  the file's `▾`.
- *File scope.* `f` in the list and file scope in the pane drop the
  file rows; the threads stay indented all the same, so toggling the
  scope does not reflow the rows and the chevron's click column stays
  fixed.
- *The file's bar.* The bar alone, never bold on its own: bold stays
  for the row the cursor rests on, where the bar and bold already
  appear together.

## Decision

### A thread's rows sit one level in

- **The nest.** `app/draw/nest.rs`, which this record backs, names the
  nest: two cells, `NEST`, that every row of a thread in the list and
  in the pane sits in from the column's edge, under the file row's
  path. The list draws it after the cursor cell: a thread's header,
  its folded row, its message rows, and its body rows all move two
  cells right, and a message body wraps two cells narrower. The pane
  draws it before the circle: a thread's first row and its summary
  row move two cells right, and the summary keeps its place under the
  place.
- **The file row does not move.** The file row is as it was: the
  cursor cell, `▾` or `▸`, a space, the path, the count at the edge,
  and in the pane the same after its leading space. A thread's
  chevron in the list and its circle in the pane now sit under the
  path's first letter.
- **File scope keeps the nest.** With the file rows gone the threads
  keep their indent; the header's ` · path` names the file over them.
- **The chevron's cell.** The chevron click of 0076 reads the cell
  after the nest, cell `1 + NEST` of the column, on a thread's header
  or folded row.

### The file row marks the cursor's file

- **The bar.** A file row in the list draws `▎` in `thread.cursor` in
  its edge cell whenever the cursor's thread is one of the file's,
  whether the cursor rests on the thread's header, its folded row, or
  one of its messages, as the header draws it while the cursor is on
  a message. `Row::File` carries `inside` for this beside `selected`,
  which still says the cursor rests on the row and reads bold with the
  bar.
- **The pane** draws no bar; its current-file tint and its selected
  surface stay as 0066 set them.

## Consequences

- `threads/list.rs` widens `BODY_INDENT` by the nest and gives
  `Row::File` its `inside`; `draw/mod.rs` draws the nest on the list's
  thread rows and the file row's bar; `draw/header.rs` gives the entry
  header its nest after the cursor cell; `draw/threads_pane.rs` nests
  the pane's thread rows and widens the summary indent.
- `input/mouse.rs` reads the chevron from the cell after the nest.
- 0066's pane-rows and review-list sections and 0071's cursor section
  carry dated notes pointing here; [0011](0011-theme-schema.md)'s table
  says `thread.cursor` marks the cursor's file row too; `docs/guide.md`
  describes the nest and the file row's bar.
