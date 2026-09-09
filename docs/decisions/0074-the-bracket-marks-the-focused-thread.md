---
type: Decision
title: The bracket marks the focused thread
description: Annotated rows no longer carry a tint, and the thread the cursor is on no longer tints its rows blue; instead the gutter cells that draw its bracket sit on a brighter yellow, so the text's only backgrounds are the threads' own surfaces.
resource: crates/fathomable/src/app/draw/note.rs
tags:
  - decision
  - annotations
  - rendering
---

# 0074 The bracket marks the focused thread

Status: accepted (2026-09-09)

## Context

The text column had come to carry several backgrounds at once. Every
row a thread covers sat on a yellow tint (`thread.line`,
[0013](0013-annotation-storage-and-ux.md)); the rows of the thread the
cursor is on sat on a blue one instead (`thread.focus`,
[0033](0033-open-thread-lines.md), made a second hue by
[0036](0036-gutter-rows-and-focus-colour.md)); under those rows a
stub or an expanded thread sits on `thread.inline`, each message on
its author's stripe ([0071](0071-author-stripes.md)), and a draft on
its warm surface. The selection and the search matches paint over all
of it. The row tints were there before the threads moved into the file
([0049](0049-inline-threads-and-the-rail.md)); now that a thread's
surface stands under its lines, the tints on the lines compete with it
rather than help.

The gutter already says where a thread's lines are: the note cell
brackets them, `╭` on the first row, `│` between, `╰` on the last
([0027](0027-revisiting-threads.md), 0036), the circle for a thread on
one row ([0066](0066-one-circle-language.md)).

The user asked on 2026-09-09 to pull the backgrounds back to the
inline threads alone: drop the yellow and the blue on the lines, and
instead, while a thread is selected, light the gutter cells where its
bracket is drawn in a slightly brighter yellow. Settled with the
recommended options; no round was held. The choices:

- *Which rows light up?* The rows the blue tint marked until now: the
  thread cursor's thread, while the text cursor rests on its lines or
  its own rows (0033, 0049), and never a detached thread. Lighting the
  bracket of every thread on the cursor row was rejected: the point is
  to say which one the keys act on.
- *Which cell?* The note cell, on rows where it draws a glyph. A row
  the thread covers whose cell is blank, such as its own stub or
  expanded rows below its last text row, draws no highlight: a lit
  empty cell would look like a stray.
- *Whose glyph?* Whatever the cell draws. Inside a nested pair the
  inner thread's corners sit on the outer's line (0027); the highlight
  follows the selected thread's extent, not its glyphs, so the lit
  cells run from its first row to its last whichever thread's glyph
  each holds.
- *The key.* A new one, `thread.bracket`, rather than repurposing
  `thread.focus`: the threads pane draws the current file's rows on
  `thread.focus` (0066) and should keep its blue. `thread.line` goes;
  nothing draws it any more, and a dead key would mislead a theme
  author. A user theme that still sets it fails to load with the key
  named, as [0039](0039-gutter-colour-and-detached-rows.md)'s removals
  did.

## Decision

- **No tint on the lines.** `text_lines` no longer patches a row with
  `thread.line` or `thread.focus`. A row of the text draws on the text
  background alone unless the selection or a search match covers it.
- **The bracket lights up.** `app/draw/note.rs`, which this record
  backs, draws the note cell for the text rows, the stub rows, and the
  expanded rows: the glyph `note_on_row` gives in its state colour over
  the row's surface, and when the thread cursor's thread covers the row
  (`open_thread_on_row`) and the cell holds a glyph, the cell's
  background is `thread.bracket`. The glyph's colour is still the
  thread's state.
- **The key.** `thread.bracket` joins the [0011](0011-theme-schema.md)
  table: the background of the note cell on the rows of the thread the
  cursor is on. The bundled dark theme sets `#4a4420`, the light one
  `#ffe08a`: the old `thread.line` yellows, brighter, since one cell
  has to carry what a whole row did. `thread.line` leaves the table,
  the key enum, and both bundled themes. `thread.focus` stays, for the
  threads pane's current-file rows.

## Consequences

- The text column's only backgrounds are the threads' own: the inline
  surface, the author stripes, the draft, the selection, and the
  search. Which thread the keys act on is read from the gutter, where
  its extent already was.
- A thread on one row shows one lit cell, its circle; a range shows
  its corners and the line between lit.
- `Theme` in `app/draw` loses `thread_line` and gains `thread_bracket`;
  the three call sites that built the note span share `note_cell`.
- 0013, 0033, 0036, and 0049 carry dated notes pointing here; the
  guide's threads passage says the bracket lights instead of the
  lines.
