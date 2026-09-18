---
type: Decision
title: The comment box is an editor
description: A cursor-bearing text buffer in core, the keys that drive it, bracketed paste, mouse placement, a draggable box, and the $EDITOR escape hatch.
resource: crates/fathomable-core/src/editor.rs
related_resources:
  - crates/fathomable/src/app/run/draft.rs
tags:
  - decision
  - annotations
  - input
---

# 0018 The comment box is an editor

Status: accepted (2026-08-27)

The "Drawing and mouse" section is superseded on 2026-09-04 by
[0054](0054-the-draft-is-written-in-the-thread.md): the draft is written
in the thread's rows in the text, with no box, cap, or drag.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

[0013](0013-annotation-storage-and-ux.md) gave Fathomable one text entry,
the comment box, and left it a `String` with `push` and `pop`: Backspace
eats the last character, there is no cursor, Up and Down scroll the thread
pane, a paste arrives as a burst of key events (bracketed paste was never
enabled, so a terminal that sends it lost the text), and a click in the
box does nothing while the mouse works everywhere else (0013, amended
2026-08-27). The [charter](../charter.md) makes the annotation side-car the
product, so writing a comment has to be at least as good as a chat input.
These choices were settled in a question round on 2026-08-27.

## Decision

### Buffer

- `fathomable_core::editor::Buffer` is a plain text buffer with a cursor
  and no terminal types: the text, a byte offset for the cursor, and a
  wanted column so vertical motion keeps its place across short lines.
  `insert` takes a `&str` (a typed character or a paste, newlines kept);
  the other operations are the variants of `Edit`: `Newline`,
  `DeleteBack`, `DeleteForward`, `DeleteWordBack`, `DeleteToLineStart`,
  `DeleteToLineEnd`, and `Move(Motion)` with `Left`, `Right`, `Up`, `Down`,
  `LineStart`, `LineEnd`, `WordBack`, `WordForward`. Left and Right step
  by grapheme (`unicode-segmentation`, already in core); a word is a run
  of non-whitespace.
- The buffer also owns its own layout: `rows(width)` wraps each line at a
  display width by grapheme (comments are prose and a box is narrow, so
  wrapping beats horizontal scrolling; word-aware wrapping is a later
  refinement, made on 2026-09-09: the word crossing the edge moves down
  whole, the whitespace before it hangs off the row it ends, and only a
  word wider than the box splits between graphemes), `cursor_cell(width)`
  says which wrapped row and column the cursor is on, and
  `place_cursor(width, row, column)` is the inverse for a mouse click. Keeping the wrap in core means the drawn text, the
  terminal cursor, and the click target come from one function and are
  tested without a terminal.

### Keys (amends 0013)

- In the box, Up and Down move within the comment. The thread pane above
  a reply scrolls with the wheel, `PageUp`/`PageDown`, or `Alt-Up`/
  `Alt-Down`. Left/Right, `Home`/`End`, and `Ctrl-a` (line start) move
  along a line; `Alt-b`/`Alt-f` move by word. `Ctrl-w` deletes the word
  before the cursor, `Ctrl-u` to the line start, `Ctrl-k` to the line
  end, `Delete` forward. `Ctrl-e` is the editor hatch below rather than
  line end, since `End` already does that. None of these is a zellij
  lock (`Ctrl-g/p/t/n/h/s/o/q/b`, 0012).
- Enter submits; `Ctrl-Enter` or `Alt-Enter` adds a line (swapped from
  the original 0013 binding: most comments are one line).
- Esc on an empty box closes it as before. Esc on a non-empty draft shows
  `Esc again to discard`; a second Esc discards, any other key keeps the
  draft and clears the prompt.
- `Ctrl-e` opens the draft in `$VISUAL`, else `$EDITOR`: the TUI leaves
  the alternate screen, pauses its input thread so the editor owns the
  terminal, runs the editor on a temporary file seeded with the draft,
  and on a successful exit loads the file back into the buffer with the
  cursor at the end. The comment is never submitted by the editor; the
  user still presses Enter. With neither variable set the key says so and
  does nothing.
- Each editor invocation exclusively creates a fresh mode-0700 directory
  under the system temporary directory and a mode-0600 `comment.md` within
  it, independent of a permissive umask. Existing names, including symlinks,
  are never reused. Temporary ancestors must satisfy the
  [private-state ownership rules](0009-cli-and-diagnostics.md#persistent-state-privacy);
  foreign-owned, symlinked, or non-sticky shared writable parents are refused
  before a draft is created. The enclosing directory also protects editor replacement
  files and backups placed beside the draft. Both normal and error returns
  attempt to remove that directory and its contents, with explicit cleanup errors and
  a drop guard as a backstop. Abrupt process termination can leave private
  scratch data behind; cleanup is not secure erasure. An editor configured
  to save elsewhere remains outside Fathomable's control.
- The same buffer edits existing messages (amended 2026-08-30): `e` in
  the thread pane or workspace thread list opens the selected
  user-authored message, seeded with its current body; Enter saves it as
  an append-only annotation event. Esc closes an unchanged edit at once
  and asks twice only after the body changes. Agent-authored messages
  cannot enter the editor.

### Paste

- The TUI enables bracketed paste beside mouse capture. A
  `Paste` event with the box open inserts the text at the cursor with
  `\r\n` folded to `\n`; anywhere else it is dropped, as a viewer has
  nothing to paste into.

### Drawing and mouse

- The box draws the wrapped rows, scrolled so the cursor's row is
  visible, and puts the terminal cursor on `cursor_cell`. Its height is
  the wrapped text plus the rule and header, capped at eight rows, until
  its top rule is dragged (`Border::Compose`), after which it keeps the
  dragged height for the rest of the session like the thread pane
  (0007). It never takes more than the pane minus one row.
- A click inside the box places the cursor at that cell; a click on its
  rule starts the drag. Clicks elsewhere still go to the pane under the
  pointer and leave the box open (0013).

## Consequences

- `Compose` holds a `Buffer`; `compose_char`, `compose_newline`, and
  `compose_backspace` are replaced by `compose_insert` and
  `compose_edit(Edit)`. `App::compose_rows` is the single source of the
  box geometry for drawing and the mouse.
- The input thread polls with a timeout and honours a pause flag rather
  than blocking in `read` forever, so the editor hatch can hand the
  terminal over without a stray keystroke landing in the wrong process.
- The key table in the [guide](../guide.md) covers both new comments and
  seeded message edits. 0013's "Up/Down scroll it" for a reply is
  superseded by this record.
