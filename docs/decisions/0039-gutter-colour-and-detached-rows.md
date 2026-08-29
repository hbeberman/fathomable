---
type: Decision
title: Gutter colour says status, detached threads get a row
description: The note cell's colour is the thread's status alone (amber open, teal waiting, grey resolved); placement words stay in the panes; a detached thread draws on a blank row inserted where its lines were; and a bracket bridges rendered rows that no source line backs.
resource: crates/fathomable/src/app/detached.rs
tags:
  - decision
  - annotations
  - rendering
---

# 0039 Gutter colour says status, detached threads get a row

Status: accepted (2026-08-28)

## Context

Three things the reader noticed on 2026-08-28 in a rendered markdown
file with two threads, one on line 5 and one on lines 5–10.

The 5–10 thread drew `•` on lines 5, 7, and 10 instead of `╭ │ ╰`.
[0036](0036-gutter-rows-and-focus-colour.md) decides the bracket
from the neighbouring rendered rows and said that a row with no source
does not continue a thread. A blank line in rendered markdown is such a
row, so each of the thread's sourced rows had sourceless neighbours and
read as a one-row thread.

The note cell had six colours — open, waiting, resolved, auto-resolved,
edited, and detached — chosen by "most urgent" wherever they met, so a
resolved thread whose lines were gone was as loud as an open one, and
the reader saw yellow beside orange without a rule for which was which.
[0032](0032-placement-and-state.md) had already separated the two
questions in the panes (the *placement* word, then the *state* word)
but the gutter still folded them into one hue.

Settled in a discussion on 2026-08-28:

- *Colour.* Three, all status: amber for open, a distinct colour for
  waiting (it is the state that most wants noticing: an agent replied
  and it is the reader's turn), grey for resolved so open work alone
  draws the eye. Auto-resolved folds into resolved; the pane header
  still says which.
- *Edited.* Leaves the gutter. The re-anchoring of
  [0019](0019-reanchoring-edited-lines.md) stays, and *edited* stays
  in the thread pane's header and the file-threads pane as a flag; it
  is no longer a colour, and the placement glyphs (`~`, `?`) offered
  in their place were not taken: the timeline should not carry it at
  all.
- *Detached.* A blank row inserted at the place the lines were last
  known to be, carrying the thread's mark. The row is the signal, so
  detached needs no colour or glyph of its own; the real lines that
  now sit at the old range are no longer tinted or bracketed for a
  thread that is not about them. The row is not a place to start a
  thread: `C` on it is refused, `c` opens the detached thread.
- *Blank rows in a range.* The bracket bridges them: a sourceless row
  between two rows of the same thread draws `│` and the annotation
  tint, and the corners are decided by the nearest sourced rows.

## Decision

### Colour

- `MarkKind` is the status alone: `Resolved`, `AutoResolved`, `Open`,
  `Waiting`, in that urgency order. `Mark::placement()` says where the
  thread is; `Words` ([0032](0032-placement-and-state.md)) builds its
  placement word from the placement, not the kind.
- The theme keys `annotation.resolved.auto`, `annotation.detached`, and
  `annotation.edited` are removed from the [0011](0011-theme-schema.md)
  schema; a user theme naming them is rejected as an unknown key, as
  any other unknown key is. `annotation.open`, `annotation.waiting`,
  and `annotation.resolved` remain and colour the note cell, the
  file-threads pane, and the thread list by status.
- The bundled themes set `annotation.open` to an amber (`#e0a030`
  dark, `#b06a00` light), `annotation.waiting` to bold teal (dark) or
  bold blue (light) as before, and `annotation.resolved` to
  `bright-black`. `annotation.line` and `annotation.focus` are
  unchanged.

### Bridging

- `App::note_on_row(row)` takes as neighbours the nearest rows above
  and below that have source lines, not the adjacent rows, so 0036's
  "a neighbouring row with no source does not continue a thread" no
  longer holds: only the edges of the document close a bracket.
- A row with no source lines, lying between two sourced rows that one
  thread covers, draws `│` in the colour of that thread's status and
  takes the `annotation.line` tint (and `annotation.focus` when the
  thread is the open one). A sourceless row that no thread spans draws
  nothing, as before.

### The detached row

- `app/detached.rs`, which this record backs, places detached marks. A
  detached thread's *anchor line* is the start of its last known range,
  clamped to one past the last line of the file. `Layout` gains
  `Layout::with_rows_before(lines)`: for each anchor line, in order,
  one blank line with no source and no number is inserted before the
  first rendered row whose source begins at or after it, or appended
  when there is none; `Line::stands_before()` returns the anchor of
  such a row. Anchors that coincide share one row.
- `View::set_detached_anchors(lines)` records the anchors and lays the
  document out again when they change; `App::refresh_marks` calls it,
  so a reload, a reply that moves a thread, or a relocation on start
  adds or removes rows as threads detach and re-anchor.
- On a detached row `note_on_row` reports `•` in the colour of the most
  urgent detached thread anchored there and the row takes the
  `annotation.line` tint. Everywhere else — `mark_in`, `threads_at_cursor`,
  `open_thread_in`, the bracket — detached marks are ignored, so the
  real lines at a stale range are not marked for a thread that is not
  about them.
- `threads_at_cursor` on a detached row returns the threads anchored
  there, so `c`, `Space a`, and `]c`/`[c` reach them; `goto_thread` on
  a detached thread lands on its row. `C` on a detached row, and `c`
  when nothing is anchored there, are refused with the notice
  "no lines here to annotate": the row is not text.

## Consequences

- The gutter says one thing per cell: the glyph is the extent, the
  colour the status. Where the lines are is the row itself — a
  detached thread sits on a row of its own, and whether that row is
  amber or grey says whether it still needs an answer.
- A user theme that set `annotation.edited`, `annotation.detached`, or
  `annotation.resolved.auto` stops loading until the keys are removed;
  the error names the key.
- Rows shift by one below each detached row, so line-number navigation
  (`G`, `:n`) still lands on the source line, which is what the rows
  are addressed by; the cursor can rest on a detached row as it can on
  a blank markdown row.
- The diff view keeps 0006's rule for removed hunks (a `▔` on the line
  below, no row): a hunk is not something the reader navigates to by
  identity, a thread is. The two look different on purpose.
- The `edited` flag on the thread record, the session socket, and MCP
  is unchanged.
