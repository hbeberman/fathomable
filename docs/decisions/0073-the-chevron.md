---
type: Decision
title: The chevron
description: An expanded thread's header row draws a ▾ in the thread's gutter and a stub's first row a ▸ in the same column; a click on either folds or expands the thread, and a double-click anywhere on the header folds it, so the mouse opens and closes a thread without a key.
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
same.

[0071](0071-author-stripes.md) gave the expanded thread a gutter of
two cells after the global gutter, the cursor bar or a space and then a
space; the second cell was empty on every row. A stub had no gutter of
its own: its edge cell, the circle, and the name.

## Decision

- **The header's chevron.** An expanded thread's header row draws `▾`
  in the second cell of the thread's gutter, after the cursor bar or
  its space, in the info colour: `▎▾ ● open · watched by coder`. The
  circle and the words move one cell right of where the author rows'
  names begin. A draft block's header (`comment on L3-5`,
  [0054](0054-the-draft-is-written-in-the-thread.md)) draws none: it
  does not fold.
- **The stub's chevron.** A collapsed stub's first row draws `▸` in
  the same column, so a folded thread and an expanded one show their
  chevrons under each other. The stub gains the two cells: its edge
  cell, the chevron or a space, a space, then the circle and the name
  as before. The circle stays on the newest message's row
  ([0066](0066-one-circle-language.md)); on a two-row stub the chevron
  is on the older row and the circle on the newer.
- **A click on the chevron.** A left press on the thread's gutter of
  the header row, the bar's cell or the chevron's, folds the thread.
  A click anywhere on a stub expands it, as 0049 says; the chevron
  is the cell that says so.
- **A double-click on the header.** Two presses on one cell of the
  header row within the multi-click window fold the thread, wherever
  on the row they land. One press places the cursor as it did. The
  expanding press on a stub and the folding press on a header both
  end the gesture, so a double-click on a stub expands it and stops:
  the second press is a first press on the header, not a fold.
- **The gutter's presses are unchanged.** A press in the global
  gutter of the header row still selects the line it settles on, as a
  gutter press does everywhere.

## Consequences

- The mouse can open and close a thread without the keyboard, and the
  glyph names the affordance, as the hints on the bars do
  ([0064](0064-hints-you-can-press.md)).
- `expanded_header` draws the chevron after the cursor tone; `stub_line`
  gains the two cells and its text is two cells narrower.
- `text_mouse` reads the header row before the general press: a press
  in the thread's gutter folds, a second press on one cell folds, and
  both clear the press record.
- 0049's stub bullet, 0050's gestures, 0067's header bullet, and
  0071's stub note carry dated notes pointing here.
- The guide's mouse and threads passages say the chevron folds and
  unfolds and a double-click on the header folds.
