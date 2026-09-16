---
type: Decision
title: The draft is written in the thread
description: A comment, reply, or edit is written inline in the text, in the rows of the thread it belongs to, rather than in a box along the bottom of the viewer; a new comment gets a draft block under its lines, an edit replaces the message it edits, and the review list opens the file to write and comes back after.
resource: crates/fathomable/src/app/threads/draft.rs
related_resources:
  - crates/fathomable/src/app/threads/stubs.rs
  - crates/fathomable/src/app/threads/mod.rs
  - crates/fathomable/src/app/threads/cursor.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/draw/message.rs
  - crates/fathomable/src/app/input/mouse.rs
  - crates/fathomable/src/app/input/keys.rs
  - crates/fathomable/src/app/input/bindings.rs
  - crates/fathomable/src/app/view.rs
  - crates/fathomable/src/app/mod.rs
tags:
  - decision
  - annotations
  - input
  - rendering
---

# 0054 The draft is written in the thread

Status: accepted (2026-09-04). Amended 2026-09-05 by
[0067](0067-the-texts-key-bar.md): the author row is ` user  draft`
alone; the draft keys are on the text's key bar.

Amended 2026-09-16 by
[0085](0085-thread-lifecycle-and-auto-resolve.md): `Enter` submits or saves
normally, `Ctrl-Enter` submits or saves and enables one-shot auto-resolve,
and `Alt-Enter` alone inserts a newline. If another writer resolves the
thread during a reply or edit draft, the first submit writes nothing and
keeps the draft intact; the bar offers `Enter` to atomically reopen and
submit with the original Ctrl-Enter intent, or `Esc` to keep editing while
the thread remains resolved.

## Context

[0013](0013-annotation-storage-and-ux.md) gave the viewer a comment box
along the bottom of the text column and [0018](0018-comment-editor.md)
made it an editor: a rule, a header naming what is written and the box
keys, then the wrapped draft on the popup background, growing up from
the status line to a cap of eight rows unless its rule is dragged.
[0049](0049-inline-threads-and-the-rail.md) then moved reading a thread
into the text, expanded in place under its lines, and kept the box for
writing: "`r` replies (the comment box as today, focus returns to the
message)". So a reply is read where its lines are and written somewhere
else, in a grey pane whose header repeats the placement the expanded
header already shows, with the thread it answers pushed up behind it.

The user asked on 2026-09-04 for the entry field to "behave inline,
like the text entry field is just at the bottom of the list of
thread-items being added instead of a grey-background pane on the bottom
of the viewer". A question round the same day settled the four points
the request left open; each took the recommended answer.

## Decision

### Vocabulary

- The **draft** is the comment, reply, or edit being written. It is not
  a box, a pane, or a popup; it is rows of the text. "Comment box" is
  not used.
- A **draft block** is the provisional expanded block a new comment
  gets under its lines while it is written, before the thread exists.

### Where the draft sits

- **A reply** is written at the bottom of its thread's expanded block:
  after the last message come an author row and the draft's wrapped
  rows. The thread expands if it was folded, and the text cursor lands
  on its newest message as `r` from the text does today; from the
  threads pane the keys stay with the pane, as before, while the thread
  shows expanded in the text.
- **An edit** replaces the message it edits: that message's author row
  and body rows give way to the author row and the draft's rows, seeded
  with the message's text, so the edit is made where the message is.
  The messages after it keep their rows and their stops.
- **A new comment** gets a draft block under the last row of the lines
  it is on (the selection, or the cursor line): a header row reading
  `comment on L3-5` in the header's key tone, then the author row and
  the draft's rows. On submit the block goes and the thread's stub
  appears; on cancel the block goes and nothing else changes.
- **Drafts belong to their files.** Leaving a file parks its draft in
  the open document, keeping its target, text, editor cursor, and discard
  prompt. Other files neither draw it nor route keys or submission to it.
  Returning restores it, and different files can each keep a draft.
  This applies to line comments, file comments, replies, and edits,
  including navigation requested by an agent. Drafts remain in memory
  for the lifetime of their open documents; they are not saved to disk.
- **From the review list**, `c`, `e`, `Space c r`, and `Space c e` do
  what `Enter` does first: the list closes, the file opens with the
  thread expanded and the cursor on the message, and the draft is
  written there. When the draft closes, by submit, cancel, or clearing
  an empty one, the review list comes back with the keys and its cursor
  on the thread. The list never shows a draft of its own.

### The rows

- The **author row** reads ` user  draft` as a message's author row
  reads ` user  2m ago`, with the draft keys at the right edge in the
  header style: `Enter submit` (`save` for an edit), `Alt-Enter
  newline`, `Alt-k/j scroll`, `Ctrl-e $EDITOR`, `Esc`. After an Esc on a
  changed draft the row's right edge asks `Esc again to discard · any
  key keeps the draft` instead, as the box header did. A click on a
  hint runs it ([0050](0050-mouse-menus-and-gestures.md)).
- The **draft's rows** are the draft wrapped at the text width less the
  message indent, one row per wrapped row and at least one, indented as
  a message body is. The terminal cursor sits on the draft's cursor
  cell; a click on a draft row places it there. The rows are on the
  `thread.inline` background with the gutter blanks of any expanded row,
  the bracket of an enclosing thread included. They are not stops:
  `j`/`k` never rest on them, and the thread cursor does not read them.
- The draft grows and shrinks with its text; there is no cap and no
  drag. Whenever the draft changes, the view scrolls just enough to
  show the row the draft's cursor is on, without moving the text
  cursor. `Alt-k`/`Alt-j` (`Alt-Up`/`Alt-Down`) still scroll the text by
  the wheel's step, so the lines above can be read; the next keystroke
  brings the draft's cursor back into view.

### Keys and the mouse

- The keys in the draft are 0018's, unchanged, and reach it however the
  keys were focused, as they reached the box. The bindings group is
  `Draft` and the place is `Where::Draft`.
- The rule along the top of the box and its drag (`Border::Compose`)
  go, with the box's cap and its dragged height. The right button is
  still ignored while a draft is open ([0050](0050-mouse-menus-and-gestures.md));
  the wheel and left clicks elsewhere in the text work as they do with
  no draft.

### What stays

- 0018's `Buffer`, the `$EDITOR` hatch, bracketed paste, the two-Esc
  discard, `Ctrl-c` to clear, and the empty-draft rules keep their
  meaning; only where the text is drawn and how much of it shows
  change.
- `Popup::Compose` remains the state that routes keys to the draft;
  nothing pops up.

## Consequences

- `app/threads/draft.rs`, which this record backs, holds `Compose`,
  its target, opening and closing a draft, and the draft's place in
  the rows: which block it is in, where its rows and its cursor are,
  and what a click on them does. The comment-box section of
  `app/threads/mod.rs` moves there.
- The active draft takes keys through `Popup::Compose`; while parked,
  it belongs to its `Doc`. Showing a document restores its draft before
  further typing or submission, rather than carrying the previous
  file's draft into the new file.
- A stub's subject is a thread or a draft block: `stubs()` adds the
  draft's rows to the block that holds it, and a draft block for a new
  comment, so the view lays the rows out as it lays out any expanded
  thread and the drawing asks back for the words.
- `App::compose_rows`, `compose_first_row`, `compose_width`, and
  `compose_click` go, with the `Border::Compose` drag and the box
  height constants; `View` gains `reveal_row`, a scroll that shows a
  row without moving the cursor.
- `draw_compose` goes; the expanded block's lines carry the draft, and
  the text places the terminal cursor on it.
- Amended by this record where they describe the box: 0013 (the box),
  0018 ("Drawing and mouse"), 0049 ("`r` replies (the comment box as
  today ...)"), 0050 (the box header's hints, now the author row's).
- `docs/guide.md` renames the key table and describes the draft in the
  same change.

Since [0071](0071-author-stripes.md) the draft's author row and text
rows sit on `thread.draft` rather than `ui.header` and the block's
tint, the author row names the user by `user.name` in the user's colour,
and the rows begin after the thread's own two-cell gutter.
