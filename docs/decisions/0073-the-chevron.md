---
type: Decision
title: The chevron
description: An expanded thread's header row draws a bold ▾ in the thread's gutter and a stub's first row a bold ▸ in the same column; a click on either, or a double-click anywhere on the header or the stub, folds or expands the thread, while one click elsewhere on the row only places the cursor.
resource: crates/fathomable/src/app/input/mouse.rs
related_resources:
  - crates/fathomable/src/app/draw/header.rs
  - crates/fathomable/src/app/draw/mod.rs
tags:
  - decision
  - input
  - rendering
  - annotations
---

# 0073 The chevron

Status: accepted (2026-09-09)

## Context

A thread in the file opens and closes from the keyboard: `z` folds and
unfolds it ([0065](0065-z-folds-and-unfolds.md)), `c` walks on
([0049](0049-inline-threads-and-the-rail.md)). The mouse had half of
that: a click on a stub expands it (0049), but nothing on an expanded
thread folds it back. A click on its header placed the cursor, a
double-click selected a word, and the right-click menu's `fold thread`
was the only way to close a thread without a key. The header carried
no hint that said it could close, since its keys moved to the bar
([0067](0067-the-texts-key-bar.md)) and the stub's `(z expand)` with
them.

The user asked on 2026-09-09 for a clickable `v` / `>` indicator in the
gutter of the thread's top row, so the mouse has a way to expand and
collapse a thread, and for a double-click on the header to do the
same. With those in place they asked the same day that one click on a
stub no longer expand it, so a click on a stub is a click like any
other, and that the arrows read bold.

[0071](0071-author-stripes.md) gave the expanded thread a gutter of
two cells after the global gutter, the cursor bar or a space and then a
space; the second cell was empty on every row. A stub had no gutter of
its own: its edge cell, the circle, and the name.

## Decision

- **The header's chevron.** An expanded thread's header row draws `▾`
  in the second cell of the thread's gutter, after the cursor bar or
  its space, in the info colour and bold: `▎▾ ● open · watched by
  coder`. The
  circle and the words move one cell right of where the author rows'
  names begin. A draft block's header (`comment on L3-5`,
  [0054](0054-the-draft-is-written-in-the-thread.md)) draws none: it
  does not fold.
- **The stub's chevron.** A collapsed stub's first row draws `▸` in
  the same column, bold too, so a folded thread and an expanded one
  show their chevrons under each other. The stub gains the two cells: its edge
  cell, the chevron or a space, a space, then the circle and the name
  as before. The circle stays on the newest message's row
  ([0066](0066-one-circle-language.md)); on a two-row stub the chevron
  is on the older row and the circle on the newer. (A stub has been one
  row since later on 2026-09-09, [0049](0049-inline-threads-and-the-rail.md):
  the chevron and the circle share it.)
- **A click on the chevron.** A left press on the thread's gutter of
  the header row, the bar's cell or the chevron's, folds the thread;
  the same press on the chevron's column of a stub's rows expands it.
- **A double-click on the row.** Two presses on one cell within the
  multi-click window (400 ms, as the word and line gestures of
  [0050](0050-mouse-menus-and-gestures.md)) fold the header's thread
  or expand the stub's, wherever on the row they land. One press
  elsewhere on either row only places the cursor: on the header as it
  did, on a stub on the row the stub hangs under, as 0049 gives the
  right button. 0049's *a click on a stub expands it* no longer holds.
  (Since 2026-09-11 the review list's thread rows take the same
  chevron click and double-click, and a stub is a stop for `j`/`k`,
  [0076](0076-threads-fold-in-the-list.md).)
  The expanding press and the folding press both end the gesture, so
  a double-click on a stub opens the thread and stops: a third press
  is a first press on the header, not a fold. (Amended 2026-09-09: one
  press on a stub's words rests the text cursor on the stub's own row,
  not on the row it hangs under. The user clicked a stub and saw the
  block cursor jump to the line above, a line the click was not on; a
  stub has no column for a block cursor, so the terminal cursor hides
  there as on the expanded rows and the stub's `▎` bar marks the place,
  as [0049](0049-inline-threads-and-the-rail.md)'s stub bullet and
  [0071](0071-author-stripes.md)'s bar rule now say. A press in the
  global gutter of the row still selects the line it hangs under.
  Amended 2026-09-10: the expanded header's words rest the cursor the
  same way, on the header row itself. *On the header as it did* had
  settled the click to the row above, the header being no stop for a
  motion, so the block cursor jumped off the row the user clicked
  just as it had on a stub; the terminal cursor hides there as on the
  thread's other rows, the header's `▎` bar marks the place, and `j`
  steps on to the first message, `k` back to the line above. And a
  fold from the thread's rows, by the chevron, a double-click, or `z`,
  rests the cursor on the stub the header becomes, not on the line
  above, as [0065](0065-z-folds-and-unfolds.md) now says.)
- **The gutter's presses are unchanged.** A press in the global
  gutter of the header row still selects the line it settles on, as a
  gutter press does everywhere.

## Consequences

- The mouse can open and close a thread without the keyboard, and the
  glyph names the affordance, as the hints on the bars do
  ([0064](0064-hints-you-can-press.md)). A click on a stub's words
  no longer opens it; a reader who clicks to place the cursor near a
  thread is not surprised by the thread unfolding.
- `expanded_header` draws the chevron after the cursor tone; `stub_line`
  gains the two cells and its text is two cells narrower.
- `text_mouse` reads a stub's rows and the header row before the
  general press: a press in the thread's gutter or a second press on
  one cell opens or closes, and both clear the press record.
- `Tone::Chevron` in `header.rs` is the info colour, bold.
- 0049's stub bullet, 0050's gestures, 0067's header bullet, and
  0071's stub note carry dated notes pointing here.
- The guide's mouse and threads passages say the chevron folds and
  unfolds and a double-click on the header folds.
