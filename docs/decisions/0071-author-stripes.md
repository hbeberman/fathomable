---
type: Decision
title: Author stripes and the cursor bar
description: Every message in an expanded thread and in the review list sits on a faint tint of its author's kind, blue for the user and green for agents, with the author's name in the same hue; the thread cursor is a bar in the key colour down the left edge of its message's rows with the name in bold, the entry header marked too, instead of the selected surface; the expanded thread in the file has a two-cell gutter of its own after the global gutter; a draft is written on a third, warm surface.
resource: crates/fathomable/src/app/draw/author.rs
related_resources:
  - crates/fathomable-core/themes/default-dark.kdl
  - crates/fathomable-core/themes/default-light.kdl
tags:
  - decision
  - annotations
  - rendering
  - configuration
---

# 0071 Author stripes and the cursor bar

Status: accepted (2026-09-09)

## Context

A thread is a conversation between the user and one or more agents, and
the viewer drew every message of it the same way: on the block's one
tint (`thread.inline`, [0049](0049-inline-threads-and-the-rail.md)),
the author's name in the key yellow, the body in the text colour. The
only thing that said who wrote a message was the name on its author
row, and a long agent reply pushed that row off the screen. The user
asked on 2026-09-09 to tell their own comments from the agents' at a
glance, in the expanded thread and in the review list of
[0025](0025-thread-list.md) alike, and chose faint background striping
with a name colour over the alternatives (an edge bar per author, a
name colour alone, an indent).

The same day the cursor's message took the whole row on
`ui.picker.selected` ([0066](0066-one-circle-language.md)), and its
entry header too. Once the background carries the author, a second
full-row surface for the cursor hides the stripe of the very message
the reader is looking at. The cursor needed a mark that sits beside the
stripe rather than over it.

In the file, an expanded thread's rows begin right after the global
gutter of mark, line number, diff bar, and space. A cursor bar in that
last gutter cell would stand next to the diff bar of
[0017](0017-git-status-navigation.md), the same glyph in another
colour. The user asked for the thread to have a gutter of its own.

A question round on 2026-09-09 over six mockups settled the rest: blue
for the user and green for agents, the name in the stripe's own hue and
legible on it; the bar in the key yellow, on every row of the cursor's
message and on the thread's entry header while the cursor is inside;
two cells for the thread's gutter; a warm surface for the draft; the
folded file row marked as an entry header is.

## Decision

### Four theme keys

- `thread.user` is the user's messages: its `fg` is the author's name,
  its `bg` the stripe under every row of the message. `thread.agent`
  is the same for an agent's messages. In the built-in themes the user
  is the palette's blue and agents its green, the stripes those hues a
  few percent over the ground.
- `thread.draft` is the background of a draft's author row and text
  rows while it is being written ([0054](0054-the-draft-is-written-in-the-thread.md)):
  a warm tint in both built-in themes, so the draft is neither the
  user's yet nor an agent's; on submit its rows take the user's stripe.
- `thread.cursor` is the bar: the palette's yellow, the key colour, in
  both built-in themes. `ui.picker.selected` keeps the pickers and the
  files pane and no longer marks a message or an entry.
- A theme that leaves a key unset gets no stripe and no name colour for
  it, as an unset key gives no style anywhere ([0011](0011-theme-schema.md)).

### The stripe and the name

- In the expanded thread and in the review list, a message's author row
  and body rows sit on the author kind's `bg`, and the name on the
  author row takes its `fg`; the age, the badge, and the body keep
  their colours. A resolved thread's rows in the list keep the stripe
  under dimmed text. The header row keeps `ui.header`, the file rows
  and the blank rows keep the block's surface.
- `app/draw/author.rs`, which this record backs, holds the mapping from
  an author to its name style and its row style, and the cursor bar.

### The cursor

- The cursor's message draws `▎` in `thread.cursor` on the left edge of
  every row it has, and its name bold. The entry header of the cursor's
  thread in the list, and the header row of the expanded thread in the
  file while the cursor is in it, draw the same bar in their first cell
  and read bold. A file row folded over the cursor's thread
  ([0066](0066-one-circle-language.md)) draws the bar and reads bold
  too. No row of a thread takes `ui.picker.selected` any more.

### The thread's gutter

- In the file, an expanded thread's rows and its header begin with a
  gutter of two cells after the global gutter: the bar or a space, then
  a space. The author row is two cells in, the body four, so the
  message indent of [0037](0037-markdown-in-threads.md) grows from
  three to four and the text wraps two cells narrower. The draft's
  rows and its terminal cursor move with it. Stubs, which are one row
  and carry the state glyph, are unchanged.
- The review list has no global gutter, so its rows are as they were:
  the bar in the first cell, the author three cells in, the body five.

## Consequences

- A thread reads as a conversation: the reader's own messages are blue
  rows, the agents' green, and the name colour repeats the stripe's
  hue, so the two cues never disagree.
- The cursor is one yellow bar, apart from both stripes and from the
  green, orange, and red diff bars, and the stripe under the cursor's
  message stays visible.
- Every expanded thread in the file is two cells narrower than before.
  Sideways scrolling of code blocks is still not offered
  ([0044](0044-wrap-all-lines.md)); the cost is two cells of wrap.
- A user theme written before this record shows no stripes and no name
  colour until it sets the keys, or inherits a built-in that does.
- The stub rows under a thread's lines keep `thread.inline` and the key
  yellow for the author: a stub is a one-row summary, not a message,
  and its glyph and dimming already say what state it is in. Striping
  the stubs is left until it is missed.
