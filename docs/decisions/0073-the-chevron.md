---
type: Decision
title: The chevron
description: An expanded thread's header row draws a bold ▾ in the thread's gutter and a stub's first row a bold ▸ in the same column; a click in that gutter, a double-click anywhere on the row, or Enter while the text cursor rests there toggles the thread, while a single click elsewhere rests the cursor on the clicked header or stub.
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

Amended 2026-09-16 by
[0086](0086-one-thread-summary-and-its-actions.md): the shared inline/review
summary owns the disclosure geometry. The arrow and its three following
cells are one fold target; visible action regions begin on their first
character, separators are inert, and action hits win over double-click
folding. The folding behavior below remains.

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
  show their chevrons under each other. The one-row stub gains the two
  cells: its edge cell, the chevron, a space, then the circle and the
  newest message's name ([0066](0066-one-circle-language.md)).
- **Clicks and double-clicks.** A left press in either cell of the
  thread's two-cell gutter folds an expanded header or expands a stub.
  Two presses on one cell within the 400 ms multi-click window fold or
  expand wherever on that header or stub row they land. The toggling
  press clears the gesture, so a double-click opens a stub and stops.
  A single press elsewhere rests the cursor on the clicked header or
  stub row; a press in the global gutter instead selects that row's
  underlying source line. In the review list, the chevron cell or a
  double-click toggles the thread. A stub is a stop for `j`/`k`
  ([0076](0076-threads-fold-in-the-list.md)).
- **After a mouse or `z` toggle.** Expanding a stub by its chevron,
  double-click, or `z` moves to its newest message. Folding from a
  thread row by the chevron, a double-click, or `z` leaves the cursor
  on the resulting stub. Message rows retain their ordinary text-click
  behavior.
- **Enter on the heading.** A text cursor resting on an expanded
  header or folded stub can toggle it with Enter, staying on the
  heading. Source and message rows keep Enter inert. The key bars do
  not hint Enter; `z` remains their explicit fold action.

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

## Amendment history

- **2026-09-09 — stub cursor.** A click on a stub's words was changed
  from expanding it to resting the cursor on the stub's own row; the
  global gutter still selects the source line represented by the stub.
  The stub was also reduced to one row, with its chevron and circle
  sharing that row ([0049](0049-inline-threads-and-the-rail.md)).
- **2026-09-10 — header and fold cursor.** A click on header words now
  rests on the header row itself, and folding leaves the cursor on the
  resulting stub rather than the line above.
- **2026-09-11 — review-list parity.** Review-list thread rows gained
  the same chevron and double-click behavior, and stubs became stops
  for `j`/`k` ([0076](0076-threads-fold-in-the-list.md)).
- **2026-09-16 — Enter on a heading.** Enter now folds or unfolds an
  inline thread while its expanded header or folded stub is under the
  text cursor, without adding a key-bar hint ([0065](0065-z-folds-and-unfolds.md)).
