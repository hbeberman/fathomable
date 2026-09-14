---
type: Decision
title: All keys stays reachable
description: Space ? is a compact grouped action list with two columns at an ordinary terminal and one when narrow; the complete binding table remains reachable by scrolling, filtering matches keys and words, the mouse scrolls and clicks the current layout, and the built-ins give help and menus a shared overlay treatment through separate semantic roles.
resource: crates/fathomable/src/app/input/help.rs
tags:
  - decision
  - documentation
  - input
  - rendering
---

# 0078 All keys stays reachable

Status: accepted (2026-09-14)

## Context

[0045](0045-bindings-are-data.md) made `Space ?` a rendering of the
binding table, and [0050](0050-mouse-menus-and-gestures.md) made each
drawn binding clickable. The implementation flowed every row into as
many fixed-width columns as the table required. At 80 by 24 the later
columns were clipped; making the terminal larger exposed more columns
but still did not make the final commands reachable. Every key also
closed the popup, so the help could neither scroll nor narrow the list.

The complete table must remain the source of truth. The compact screen
is a presentation of that table, not another hand-written list of
bindings or a command palette.

## Decision

- **A grouped action list.** Help derives every key spelling, action
  label, and group from `BINDINGS`. It uses concise section headings and
  presents the common reading actions first: movement, search, and
  selection in the left lane; threads and view/diff in the right lane.
  Aliases already attached to one binding stay on one row. Descriptions
  and unusually long key spellings wrap; neither is silently truncated,
  and one long key cannot widen every row.
- **Responsive columns.** An ordinary 80-column terminal has two compact
  lanes. Below a useful two-lane width the same sections collapse to one
  lane. The popup stays inside the current terminal, with a fixed title
  and footer and as many body rows as remain.
- **Nothing is omitted to fit.** Each lane is a scrollable sequence of
  visual rows. `j` / `k` and `Up` / `Down` move one row;
  `PgUp` / `PgDn` move a page, with `Ctrl-u` / `Ctrl-d` as the familiar
  aliases; `Home` / `End` and `g` / `G` reach the ends. The title says
  when content remains above or below. A resize reflows wrapping and
  columns and clamps the scroll position, rather than keeping stale
  geometry.
- **Filtering is local to help.** `/` enters filter editing. Text matches
  key spellings, action descriptions, and section names without regard
  to case. `Backspace` edits, `Enter` applies the query and returns the
  motion keys, and `Esc` clears the query and exits filter editing; the
  next `Esc` closes help. A query with no result says so explicitly.
  Typed filter characters are consumed and never dispatch to the
  document.
- **The mouse is a peer.** The wheel scrolls help, not the pane beneath
  it. A click on any wrapped row of a binding closes help and runs that
  action when it applies to the focused pane. Drawing and hit-testing
  both rebuild the same layout from the current size, query, and offset,
  so filtering, wrapping, scrolling, or resizing cannot leave stale
  click targets.
- **Help is an overlay, not navigation.** Opening, filtering, scrolling,
  and closing do not alter document position, focus, or selection.
  Unrecognised keys are swallowed while help is open.
- **Shared treatment, separate roles.** Help stays on `ui.popup`; the
  `Space` and right-click menus stay on `ui.menu`. The built-ins give
  both surfaces the same overlay colour, while a custom theme may
  intentionally separate them or leave either transparent. Keys use
  `ui.popup.key`, each overlay's headings use its own foreground in
  bold, filter matches use `ui.picker.match`, hover uses
  `ui.picker.selected`, and restrained secondary text uses the existing
  info face. Every explicit span is patched onto its active surface, and
  drawing does not force keys bold. This preserves
  [0056](0056-the-leader-trimmed.md)'s semantic surface split while
  superseding its original exact palette; [0011](0011-theme-schema.md)
  owns the current built-in colours.

## Consequences

- `app/input/help.rs` owns help state, filtering, responsive layout,
  wrapping, scrolling, and the geometry shared by drawing and clicks.
- `Popup::Help` carries that state. The key and mouse routers give help
  first refusal, so no help input leaks to the view.
- `app/draw/mod.rs` renders the help layout on the popup surface;
  the status popup keeps its independent centred table.
- Tests cover 80-column and narrow layouts, every binding becoming
  reachable, bottom scrolling, keyboard and wheel motion, filtering and
  no-match feedback, resize reflow, wrapped click targets, restored view
  state, and theme styles without a forced key weight.
