---
type: Decision
title: The text's key bar
description: The text column gets a permanent key bar on its bottom row, on `ui.header`, carrying the thread cursor's keys, the draft's keys while one is open, `Z` for the file, and a focus tip while another pane has the keys; the thread header, the stub, the draft's author row, and the review list's entry header give up their keys to the bars, the cursor's stub is marked bold instead, the diff header alone keeps its keys, and `ui.hint` retires.
resource: crates/fathomable/src/app/draw/bar.rs
related_resources:
  - crates/fathomable/src/app/draw/header.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/input/mouse.rs
  - crates/fathomable/src/app/mod.rs
  - crates/fathomable-core/src/theme.rs
tags:
  - decision
  - rendering
  - input
---

# 0067 The text's key bar

Status: accepted (2026-09-05)

## Context

[0059](0059-headers-and-the-key-bar.md) moved the review list's keys
from its header to a bar on its bottom row, because a header on the
user's half-width terminal cannot hold both words and a list of keys.
[0066](0066-one-circle-language.md) gave the threads pane a bar of its
own. The text did not follow: an expanded thread's header row still
carried `r reply · e edit · o resolve · z fold` at its right edge, a
stub's last row ended with `(z expand)`, and the draft's author row
carried the draft keys, each where [0049](0049-inline-threads-and-the-rail.md)
and [0054](0054-the-draft-is-written-in-the-thread.md) put them.
[0064](0064-hints-you-can-press.md) narrowed those to the thread
cursor's thread while the text has focus, which is right, but left the
keys scattered over three kinds of row inside the text while every
other pane says its keys in one place.

The user saw it on 2026-09-05: with the cursor on one thread its header
showed the keys and the stub below it showed none, and asked why the
`r reply · o resolve · z fold` line was not on the bottom bar as the
other panes have it.

Two things the earlier records chose stand in the way. The stub's dim
`(z expand)` is also the mark of which stub is the thread cursor's when
two stack under one line (0049, [0065](0065-z-folds-and-unfolds.md)).
And 0066 put the same four keys on the cursor's entry header in the
review list, though the list's bar already names reply, edit, and
resolve. This record undoes both, the same day, for one rule.

## Decision

- **One rule for keys.** A header is its words. Keys live on a bar
  along the bottom row of the pane they act in. The text column joins
  the review list and the threads pane in this.
- **The text's bar.** The text column's bottom row, on `ui.header`, is
  a key bar whenever a document is open and the review list is not,
  built from the binding table like the others. The bar is a permanent
  row of the column, as the review list's is: layout never moves when
  focus changes. While another pane has the keys it reads `click or
  Space w l to focus`, and a click on it focuses the text. In a diff
  the checkpoint strip sits on the row above the bar.
- **What it says** while the text has the keys, under 0064's rule that
  every hint drawn works now:
  - With a draft open, the draft's keys: `Enter submit` (`save` for an
    edit), `Alt-Enter newline`, `Alt-k/j scroll`, `Ctrl-e $EDITOR`,
    `Esc`; or `Esc again to discard · any key keeps the draft` after an
    Esc on a changed draft.
  - Else the thread cursor's keys when the cursor line has a thread:
    `r reply`, `e edit` when the cursor's message is the user's,
    `o resolve` or `o reopen`, then `z fold` on an expanded thread or
    `z expand` on a stub.
  - Then `Z fold all` while any thread in the file is expanded, or
    `Z unfold all` while the file has stubs and none is; nothing when
    the file has no thread.
  - Hints drop from the end when the column is narrow, as every bar's
    do.
- **The rows give up their keys.** An expanded thread's header row is
  its words alone. A stub's last row ends with its text. The draft's
  author row reads ` user  draft` and nothing more. The review list's
  cursor entry header loses the keys 0066 gave it; the list's bar
  already carries them.
- **The cursor's stub is marked bold.** With `(z expand)` gone, the
  thread cursor's stub reads bold, in the text colour it already took,
  whether or not the text has the keys, as the threads pane's cursor
  entry is bold whether or not the pane has them. Other stubs the
  cursor line covers keep the text colour, unbolded.
- **The diff header keeps its keys.** `h/l page · b base · t target ·
  w whitespace · Esc close` stay at the right edge of the diff header,
  the one exception to the rule. They fit there on the user's 68-column
  text column today; after the thread keys on the bar the last two
  would drop. A view's keys on its header, the cursor's on the bar.
- **`ui.hint` retires.** Nothing draws it once the stub's hint is gone;
  a theme that sets it is refused as it would be for any unknown key
  ([0062](0062-one-version-no-compatibility.md)).
- **The review list's tip** reads `click or Space w l to focus`. 0059
  said `Space w h`, which goes left to the sidebar; `Space w l` is the
  key that reaches the text column.

### Amendments

- 0011's key table loses `ui.hint` and its `ui.header` row names the
  text's key bar.
- 0049's expansion and stub bullets, 0054's author row, 0064's applied
  bullet, 0065's hints bullet, and 0066's review list bullet carry
  dated notes pointing here.
- 0050's *Chrome takes clicks* counts the text's key bar among the
  headers that take clicks and drops the expanded thread's header and
  the draft's row from the list.

## Consequences

- `app/draw/bar.rs`, which this record backs, builds the text's bar:
  `text_bar` reads the focus, the draft, the thread cursor, and the
  file's stubs and returns a `Header::bar`. `Header::bar`, the hint
  constructors, and the thread and draft hint lists in `header.rs` open
  to the module.
- `App::text_bar_rows` is the row the bar takes; `text_rows` subtracts
  it. `draw` carves the bar off the text column before the diff chrome
  so the strip lands above it. The review list's `list_rows` is
  unchanged: its bar was already counted.
- `expanded_header` and `entry_header` take no focus and no cursor;
  `draft_header` is words alone and `draft_hints` is the list the bar
  reads. `stub_line` loses `hinted` and gains the bold mark.
- The mouse: a left press on the bottom row of the text column runs
  the hint under the pointer, focusing the text first; the expanded
  thread's header and the draft's author row take no hint clicks any
  more, since they draw none.
- `Key::UiHint` leaves `fathomable-core::theme`; both built-in themes
  drop the line; the draw `Theme` loses `hint`.
- The guide says where the text's keys are in its threads, draft,
  review list, and mouse passages.
