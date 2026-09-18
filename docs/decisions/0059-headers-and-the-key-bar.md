---
type: Decision
title: Headers and the key bar
description: Pane and surface headers share `ui.header`; title words own their menus, passive scope, filename, and lifecycle state stay beside them, and actionable key hints live on each pane's bottom key bar.
related_resources:
  - crates/fathomable/src/app/draw/header.rs
  - crates/fathomable-core/src/theme.rs
  - crates/fathomable-core/themes/default-dark.kdl
  - crates/fathomable-core/themes/default-light.kdl
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/input/mouse.rs
  - crates/fathomable/src/app/threads/list.rs
tags:
  - decision
  - annotations
  - configuration
  - rendering
---

# 0059 Headers and the key bar

Status: accepted (2026-09-05). Amended 2026-09-05 by
[0066](0066-one-circle-language.md): the review list's header counts by colour and
has no sort word; the threads pane's keys sit on a bar of its own.
Amended 2026-09-05 by [0067](0067-the-texts-key-bar.md): the text column
has a bar of its own, and the unfocused tip reads `click or Space w l
to focus`, the key that reaches the text column.
Selection amended 2026-09-14 by [0079](0079-list-focus-language.md):
pane headers and key bars remain neutral `ui.header`; selected review
entry headers use shared active/remembered list styles instead of the
retired `ui.picker.selected`. Message rows keep 0071's author stripes.

Amended 2026-09-16 by
[0086](0086-one-thread-summary-and-its-actions.md): inline and review thread
headers now use one summary layout. A later 2026-09-17 amendment keeps
auto-resolve and resolve/reopen on the bottom bar at all times; headers show
only passive dim `autoresolve` or `resolve proposed` status. Direct Archive
and Restore cleanup controls remain row-specific. Every bar reads action
then hotkey, and the hovered action-hotkey region composes
`ui.header.patch(ui.list.hover)`. The older metadata-only entry-header and
count wording remains below.

Amended 2026-09-17: the normal review header is `Reviews` at the left, with
subdued `workspace`/`file` scope and lifecycle counts at the right. Scope
shortens to `w`/`f` before it is omitted. Its title menu begins with
**Open File** and separates that command from its two checked view settings;
scope and counts are passive. The bottom key bar remains unchanged.

Amended later 2026-09-17: the document surface gains a stable File header.
Only `File` is a hovered title button; the current root-relative path follows
passively, and current-file lifecycle counts sit at the right. Its menu owns
cross-navigation to Reviews plus rendered, inline-thread, and resolved-thread
settings. Comparison labels remain in the global menu bar, so unified diffs
use this same File row rather than stacking a second local header.

Amended later 2026-09-17: a directory highlighted in Files keeps that same
File header, with the selected root-relative directory path beside `File`.
Directory statistics sit in the body below it; there is no second,
directory-specific header or pane identity.

## Context

The review list of [0049](0049-inline-threads-and-the-rail.md) drew its
header and every entry's header as plain text rows: `review  3 open  3
proposed  resolved hidden  by newest agent reply`, then `Cargo.lock  L3
waiting · proposed  37m ago` above each thread's messages. With the
messages indented under them, the rows still ran together; nothing but
the path colour said where one thread ended and the next began.

The header also carried the list's keys at its right edge, eleven of
them once [0050](0050-mouse-menus-and-gestures.md) made them clickable:
`s sort · x resolved · f file · z fold · Enter open · r reply · e edit ·
o resolve · k/j threads · l/h messages · Esc`. That is about 115 cells,
after a left part of about 65. The header drops hints from its end when
the row is too narrow, so on the user's usual half-screen terminal two
or three survived and the rest were not there to click.

Two smaller things came up in the same review on 2026-09-05. A stub's
first row reads `● User now  What are your thoughts on this?` with the
age in the author's bold yellow, because the author and the age were
one string; the age wanted the info colour every other row gives it.
And the user asked whether `waiting · proposed` should say what is
proposed, then decided `proposed` was fine as it is; the wording is
unchanged.

## Decision

### A surface for headers

- The theme vocabulary of [0011](0011-theme-schema.md) gains
  `ui.header`: the background of a pane's header rows. Both built-in
  themes set it to the palette's `surface`, the status line's colour,
  so the chrome of a pane reads as one thing with the chrome of the
  app. A theme that sets no `bg` leaves the terminal showing through,
  as `ui.menu` does.
- Every row built as a `Header` draws on it: the review list's header
  and key bar, the checkpoint header, an expanded thread's header row
  in the text, the draft's author row, and the `comment on L3` row of a
  draft block. The threads pane's title row in the sidebar draws on it
  too. In the text, the header's surface wins over `thread.inline`
  under it, so an expanded thread opens with a bar.
- The review list's entry headers (`Cargo.lock  L3  waiting · proposed
  37m ago`) draw on `ui.header` as well, padded to the column's width;
  the selected entry keeps `ui.picker.selected`, which wins over the
  surface. The message and body rows under a header stay on the text
  background, so each thread is a band with a bar over it.

### The key bar

- The review list's key hints leave its header for a **key bar** on
  the list's bottom row, on `ui.header`, left-aligned from the second
  cell: ` s sort · x resolved · f file · z fold · Enter open · r reply ·
  e edit · o resolve · k/j threads · l/h messages · Esc`. The list is
  the same eleven, in the same order, with the same rules for when
  `edit`, `threads`, and `messages` appear; the bar drops hints from
  its end when it is too narrow, as the header did. Unfocused, the bar
  reads `click or Space w h to focus`.
- The bar is a `Header` with an empty left part and left-aligned hints,
  so the mouse of [0050](0050-mouse-menus-and-gestures.md) reads it as
  it read the header: a click on a hint runs it, a pair splits at its
  slash, and a hint the bar was too narrow to show is not clickable.
  The column has two chrome rows now, so the list scrolls its entries
  through the rows between them; a column of one row shows the header
  alone.
- The header keeps the words: `review  3 open · 3 proposed · resolved
  hidden`, the proposal count only while there is one, then ` ·
  <path>` while `f` narrows the list. The counts are joined by ` · `
  as a thread's state words are. The sort word, `by newest agent
  reply` or `by file`, sits at the right edge and is the first thing to
  go when the row is narrow; a click on it switches the sort, as `s`
  does and as a click on the threads pane's title switches its reach.

### The stub's age

- A stub's first row styles the author in `ui.popup.key` and the age
  after it in `ui.statusline.info`, as the review list's rows and an
  expanded thread's message rows already do.

### Amendments

- 0011's key table gains `ui.header`, and its status line carries a
  dated note.
- 0049's review list section reads "the header reads `review  4 open ·
  resolved hidden` with the sort word at the right; the keys are on a
  bar at the bottom of the list" for its header bullet.
- 0050's *Chrome takes clicks* names the review list's key bar among
  the headers that take clicks, and the sort word's click.
- 0053's count bullet reads `review  4 open · 1 proposed · resolved
  hidden` for the review list's header.

## Consequences

- The pane headers and their hints move from `app/draw/mod.rs` into
  `app/draw/header.rs`, which this record backs: `Header`, `HintOf`,
  `Tone`, the builders for each surface, and the review list's
  `review_footer`. The file is the one place the drawing and the mouse
  read a header's layout from.
- `fathomable-core::theme::Key` gains `UiHeader`; both built-in themes
  set it; the draw `Theme` carries `header`.
- `draw_review` lays out header, entries, and key bar; the list's
  visible-row count and scroll take two chrome rows; `review_mouse`
  reads the bottom row as the key bar and the top row's sort word as
  a click on the sort.
- The guide's review list section and its mouse paragraph say where
  the keys are, and §7 names `ui.header` among the theme keys.
- The wording `proposed` stays everywhere it was; a later record may
  revisit it if the ambiguity bites again.
