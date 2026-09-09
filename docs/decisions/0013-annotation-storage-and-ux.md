---
type: Decision
title: Annotation storage and UX
description: The JSONL event log, the line-hash anchor, the comment box, the gutter mark, the thread panel, and the keys that drive them.
resource: crates/fathomable-core/src/annotations.rs
related_resources:
  - crates/fathomable/src/app/threads/mod.rs
tags:
  - decision
  - annotations
  - input
  - rendering
---

# 0013 Annotation storage and UX

Status: accepted (2026-08-26)

Amended 2026-09-04 by [0050](0050-mouse-menus-and-gestures.md): a press in the gutter, a double- or triple-click, and Shift-click select as a drag does, ending in `SEL` mode; a right-click on the selection offers `c`, `C`, and `y` as a menu.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

[0005](0005-annotations.md) fixes what an annotation is and where threads
live, but leaves the file format, the anchor algorithm, and the whole
viewer side open. Milestone 3 of the [roadmap](../roadmap.md) needs all of
them. Two things in earlier records also had to give: 0010's
copy-on-release meant every mouse selection wrote to the clipboard, which
is wrong once a selection can also start a comment, and the gutter had one
cell for the git diff bar and nowhere for an annotation. The choices below
were captured in a question round on 2026-08-26.

## Decision

### Store

- One file per workspace:
  `$XDG_STATE_HOME/fathomable/workspaces/<hash>/threads.jsonl`, where
  `hash` is the first 16 hex characters of the SHA-256 of the workspace
  root path (`XdgDirs::threads_file`). The workspace tree is never written
  to, as 0005 requires; an in-tree or committed file was considered and
  rejected for now because agents will read threads through MCP
  (milestone 4), and a config override can be added later without
  changing the format.
- The file is an append-only event log. Every line is one JSON object with
  `"v": 1` and an `event` of `annotate`, `reply`, `resolve`, or `reopen`;
  `fathomable_core::annotations::Store` folds it into threads on open and
  appends after every change. A line that is not a known event, or that
  refers to a thread the file never created, is an error naming the line
  number; the TUI then runs with annotations disabled and says so. An
  `annotate` line carries id, path, range, snippet, anchor, created, and
  comment; a `reply` carries thread, author, created, body, and an optional
  `proposed_resolved`; a `resolve` carries thread, `by` (author), and
  created; a `reopen` carries thread and created. `by` other than `user`
  is the 0005 `auto_resolved` case.
- Thread ids are `<unix-seconds>-<pid>-<n>`; authors are the string `user`
  or an agent-supplied name; timestamps are Unix seconds supplied by the
  caller so the store stays pure.
- Editing a message (amended 2026-08-30) appends an `edit` event naming
  the opening comment or a zero-based reply, its replacement body, and
  the edit time. Only the opening comment and replies authored by the
  user are editable; an agent message or an unknown reply is rejected
  before anything is appended.

### Anchor

- `Anchor::capture` hashes each annotated line, plus the line above and the
  line below when they exist. The hash is the first 16 hex characters of
  SHA-256 over the line with trailing whitespace removed; `sha2`
  (approved in [0001](0001-dependency-policy.md)) joins `fathomable-core`
  with this consumer.
- `Anchor::locate` searches the current text for a window whose line
  hashes equal the annotated ones. Among matches, the one whose neighbours
  also match wins; ties go to the window nearest the last known start. No
  match means the exact lines are gone and the thread is `Detached` at its
  last known range. Nothing smarter is attempted for edited lines; that
  stays an open investigation in [parked ideas](../parked.md).

### Selection and the clipboard (amends 0010)

- Releasing a mouse drag no longer copies. The selection stays highlighted
  in `SEL` mode, exactly as after `V`; `y` copies the source Markdown via
  OSC 52, `c` opens the comment box, and Esc clears. Mouse and keyboard
  selections are therefore indistinguishable once made.
- `c` on a selection annotates the source lines it touches; a wrapped
  paragraph that renders as one row annotates all of its source lines.
  With no selection, `c` on a row that already carries a thread opens
  it instead, and `C` always starts a new thread
  ([0027](0027-revisiting-threads.md), 2026-08-28).

### Comment box

- `c` opens a box anchored to the bottom of the text pane, above the status
  line, titled with the range (`comment on L3-5`) or `reply`. Enter submits;
  Ctrl-Enter or Alt-Enter adds a line; Esc cancels. The box grows to
  eight rows, then scrolls. An empty comment is discarded with a notice.
- Ctrl-Enter is only distinguishable from Enter when the terminal supports
  the kitty keyboard protocol, so the TUI pushes
  `DISAMBIGUATE_ESCAPE_CODES` when `supports_keyboard_enhancement` says it
  can, and Alt-Enter is always accepted as the fallback.
- The same box is used for replies, so there is still exactly one text
  entry in Fathomable.

### Rendering

- The gutter gains a fourth cell: `[line number][space][diff bar][note]`
  (moved to the far left by [0006](0006-git-access.md), 2026-08-26).
  The note cell shows `▎` coloured by thread state (superseded by
  [0027](0027-revisiting-threads.md), 2026-08-28: a rounded bracket
  `╭ │ ╰` over the thread's rows, `•` for one row); the diff bar keeps its
  own cell so git and annotation information never hide each other. A
  thread's rows also get the `annotation.line` background.
  (Removed 2026-09-09 by
  [0074](0074-the-bracket-marks-the-focused-thread.md): no row carries
  a tint; the gutter's bracket lights up on the focused thread.)
- Theme keys (added to the [0011](0011-theme-schema.md) table):
  `annotation.open`, `annotation.resolved`, `annotation.resolved.auto`,
  `annotation.detached`, and `annotation.line`. When several threads
  overlap a row the most urgent colour wins: detached, then open, then
  auto-resolved, then resolved.
- The status line's right block shows `open/total threads` for the current
  file when there are any.
- Marks are re-located on every reload, so an agent's edit moves or
  detaches them immediately.

### Reading and replying

- `Space a` opens the thread pane along the bottom third of the text
  column for the thread(s) touching the cursor row. It is a pane, not a
  popup (amended 2026-08-27): it takes rows from the text rather than
  covering it, draws on the text background with no highlight of its own,
  holds focus (the pill reads `THREAD`) while its keys apply, and a click
  on the text or the tree moves focus there and leaves it open; its top
  rule is draggable ([0007](0007-key-grammar-and-mouse.md)). It shows a rule, a header with the
  position (`thread 1/2`), range, status in its gutter colour, and the
  keys right-aligned; the quoted snippet with line numbers (three lines,
  then `…`); the comment; and each reply with the author's short name,
  its age (`5m ago`, `yesterday 06:38`, then the UTC date), and a
  `[proposes resolving]` badge when set. Bodies are indented under their
  author. When the text overflows the last row reads `▼ N more`, and
  scrolling stops at the end (since [0034](0034-deleting-threads.md),
  2026-08-28, the pane opens at its end under a dim `─── END ───` row).
  The newest message starts highlighted (amended 2026-08-30):
  `j`/`k` move that highlight between the comment and replies and keep it
  visible; `e` opens a user-authored selection in the comment editor,
  seeded with its body, and Enter appends the edit. `h`/`l` page every
  thread in the selected scope, moving the cursor; `Tab` switches between
  local (this file) and global (the work), with each scope remembering
  its selected thread and message. `PageUp`/`PageDown` scroll long
  content, `Left` returns to the file-threads pane, `r` replies through
  the comment box, `x` resolves or reopens, and Esc closes. (Amended
  2026-09-04 by [0049](0049-inline-threads-and-the-rail.md): the thread pane and
  `Space a` are gone; a thread is a stub under its lines that `c`
  expands in place, with `r`, `e`, `o`, and `dd` on its rows.)
- While replying the pane stays on screen above the box, pushed up by
  the box's rows, so the thread can be read; Up/Down scroll it
  (superseded by [0018](0018-comment-editor.md), 2026-08-27: Up/Down
  move in the comment, PageUp/PageDown scroll the pane), and Esc
  returns focus to the pane rather than closing everything. The box takes
  the keys but not the mouse (2026-08-27): the wheel, clicks, and border
  drags still work in the other panes and leave the box open.
- `]a` and `[a` jump to the next and previous thread in the file, wrapping
  with a notice. `Space A` opens the picker over the file's threads
  (`L3-5  open  first line of the comment`); choosing one jumps there and
  opens the panel (superseded by [0025](0025-thread-list.md), 2026-08-27:
  `Space A` opens the workspace thread list and the picker is gone). Both
  space entries follow 0012's rule that new
  commands live in the Space menu; none of the keys are zellij locks.

## Consequences

- 0010's "releasing the button copies immediately" and its two-cell gutter
  are superseded by this record; 0011's key table grows by five keys; the
  0005 storage path is now concrete.
- `check-public-api.sh`'s no-bool-parameter rule shaped the core API:
  `Reply::proposing_resolution()` instead of a flag, `Placement` instead of
  a detached bool.
- Agents get their MCP entry points (`annotations_list`, `thread_reply`) in
  milestone 4; until then a thread file is plain JSONL an agent can read.
- Discouraging agent force-resolve stays parked.
