---
type: Decision
title: Gutter brackets rendered rows, focus in a second colour
description: The note cell brackets neighbouring rendered rows, the git bar bridges matching marks across synthetic Markdown rows, and the original focus tint uses a second colour.
resource: crates/fathomable/src/app/draw/gutter.rs
tags:
  - decision
  - annotations
  - rendering
---

# 0036 Gutter brackets rendered rows, focus in a second colour

Status: accepted (2026-08-28); the focus colour half was superseded
2026-09-09 by [0074](0074-the-bracket-marks-the-focused-thread.md):
the rows carry no tint, and the bracket cells light up instead.
`thread.focus` lives on in the threads pane.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

Two things the reader noticed on 2026-08-28 while reviewing a rendered
markdown file with the thread pane open.

The note cell of [0027](0027-revisiting-threads.md)
decided its glyph from the source lines a rendered row holds: a thread
that starts and ends in the row's lines is a dot. A markdown paragraph
is one source line, and a thread on it wraps over several rows, each
holding that same line; every row said "starts and ends here" and drew
`•`, one over the other, where a bracket would show the extent.

[0033](0033-open-thread-lines.md) tinted the open thread's rows in
`annotation.focus`, but chose it as a stronger version of
`annotation.line`: the same yellow, a little brighter. Beside the other
annotated rows it read as the same tint, so the reader still could not
see which rows the pane was about.

Settled in a question round on 2026-08-28; the recommended options were
taken:

- *The colour.* A different hue, blue, not a wider gap within yellow.
  A distinct hue reads at a glance under any syntax colouring; a wider
  gap in one hue depends on the terminal's palette. Drawing the gutter
  glyph in the focus colour as well was offered and not taken: the
  tint is enough, and the glyph's colour already says the thread's
  state.
- *The wrapped rows.* Bracket rendered rows: `╭` on the first, `│`
  between, `╰` on the last, `•` only when the thread fits one rendered
  row. Dotting the first row and leaving the rest empty was rejected
  because the range's extent is what the bracket is for.

## Decision

- `app/draw/gutter.rs`, which this record backs, takes over the note cell
  from `threads.rs`. `App::note_on_row(row)` reads the source lines of
  the row and of the rows above and below it, and `App::note_in(lines,
  above, below)` decides the glyph: a thread *starts* on the row when it
  does not overlap the row above, *ends* when it does not overlap the
  row below, and is a dot when both hold. The shortest-range and
  bracket-beats-dot rules of 0027 are unchanged; only the question they
  are asked about moved from "the row's lines" to "the neighbouring
  rows".
- A neighbouring row with no source (the edge of the document, or a
  rendered row that no source line backs) does not continue a thread,
  so a bracket closes at it; that row draws no tint either, so the two
  agree.
- `annotation.focus` in the bundled themes becomes a blue tint in the
  dark theme (`#1c2a3f`) and a pale teal in the light one (`#d6f0ef`):
  the light selection is already blue, and the two must not be confused. The theme key and its meaning in
  the [0011](0011-theme-schema.md) table are unchanged; a user theme
  keeps whatever it set.

## Consequences

- A thread on one wrapped source line looks like a thread on several
  source lines: the bracket spans the text it is about. In the source
  view, where nothing wraps, nothing changes.
- The selection is patched over the row style, so a selected row inside
  the open thread shows the selection, as it did over the yellow.
- `note_in` gains two parameters; it is only called from `note_on_row`
  and the tests.

## Git gutter continuity (2026-09-14)

In rendered Markdown, a row without source mapping (paragraph spacing or a
table border) carries the git bar when the nearest source-backed rows above
and below have the same added or modified status and staging state. Both
the colour and the thin unstaged or thick staged glyph are preserved.
Neighbours are found in the full layout, not only the visible viewport.

Source-backed rows keep their own status, including unchanged blank lines.
Deletion ticks, document edges, thread rows, and source or unified diff
displays are not bridged. This is a drawing rule only: source mappings,
diff counts, and hunk navigation are unchanged.
