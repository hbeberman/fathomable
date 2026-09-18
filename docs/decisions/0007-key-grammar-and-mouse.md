---
type: Decision
title: Key grammar and mouse
description: Vim grammar by default with first-class mouse support; Helix mode deferred.
resource: crates/fathomable/src/app/input/keys.rs
tags:
  - decision
  - input
---

# 0007 Key grammar and mouse

Status: accepted (2026-08-26)

Amended 2026-09-18: bare `q` in a normal pane opens a compact confirmation;
only `Enter` or its explicit control quits, while `Esc`, cancel, or an outside
click dismisses it. Text-entry surfaces, menus, pickers, and existing popups
retain precedence. `:q`, `:quit`, `:q!`, and `:quit!` remain immediate.

Amended 2026-09-04 by [0050](0050-mouse-menus-and-gestures.md): the right button opens a context menu of the actions that apply under the pointer, the `Space` menu takes clicks, and the gutter, double- and triple-click, and Shift-click select.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

Fathomable is a viewer, not an editor, so the key grammar is about motion,
search, selection, and commands. The user selects most annotation ranges with
the mouse.

## Decision

- Vim grammar: `hjkl`, `gg`/`G`, `Ctrl-d/u`, `/` and `?` search with `n`/`N`,
  `:` command line, visual line mode for selecting annotation ranges, marks.
- The thread pane and workspace thread list are lateral contexts (amended
  2026-08-30): `h`/`l` page threads, `j`/`k` select messages within the
  thread, and `e` edits a selected user-authored message. (Amended
  2026-09-03 by [0046](0046-one-thread-cursor.md): the pane, the
  file-threads pane, and the list share one thread cursor; `Tab`,
  `PageUp`, `PageDown`, and the pane's `Left` are gone; lowercase
  motions step within the file and uppercase across the workspace;
  `Ctrl-d`/`Ctrl-u` page everywhere; `o` resolves.)
- Mouse is enabled always: click to focus and place the cursor, drag to select
  lines, scroll wheel to scroll, click on tree entries and links. Mouse and
  keyboard share one selection model.
- The mouse goes to the pane under the pointer, not the focused one
  (2026-08-27): the wheel scrolls the text or the thread pane it is over
  — over the tree it steps one row per tick, showing the file it lands on
  (amended 2026-08-27, [0023](0023-sidebar-paging.md)) — and a click
  focuses the pane it is over. On the tree that pane is the tree itself,
  even when the click lands on a file and shows it in the main pane
  ([0023](0023-sidebar-paging.md)).
  A press on the tree's divider column or on the thread pane's top rule
  starts a drag that resizes it; the tree keeps at least 8 columns and
  leaves the text 20, the thread pane keeps at least 3 rows and leaves
  the text one. Sizes last for the session.
- A which-key style hint bar shows pending key sequences. (Amended
  2026-09-03: the bindings are one table, [0045](0045-bindings-are-data.md);
  the hint bar, the `Space` menu, and `Space ?` render from it, and the
  picker moves on `Ctrl-j`/`Ctrl-k`.)
- Keymap is fixed in v1; remapping through KDL config comes later.
- Helix selection-first grammar is deferred and would be a config switch.
- Bare `q` is guarded in normal panes: it opens a compact Quit confirmation
  whose action-first `quit Enter` and `cancel Esc` controls share keyboard,
  hover, and click targets. Outside left-click cancels and is consumed.
  Confirmation popups take precedence over panes and accept no arbitrary
  affirmative keys. Colon quit commands remain immediate.
- Cursor model, gutter, status line, clipboard, search, and Esc/quit details
  are fixed in [0010](0010-viewer-ux.md) (2026-08-26): a real row/column
  cursor, mouse release copies the source Markdown via OSC 52, `/` is regex,
  only `:q` quits (`Ctrl-c` was dropped 2026-08-26 to match vim/helix). [0013](0013-annotation-storage-and-ux.md)
  (2026-08-26) then removed copy-on-release: a mouse selection stays in
  `SEL` mode where `y` copies and `c` comments. With nothing selected,
  `c` comments on the cursor line (2026-08-27).
- One Helix key rides along despite the vim grammar (2026-08-27): `x`
  selects the whole cursor line and each further press takes in one more
  line below; a `v` selection widens to whole lines first.
- Layout: a toggleable tree sidebar on the left plus a fuzzy file picker
  popup; one bottom line shared by status, `:` and `/` input, and key hints.
- Follow mode: when an agent marks a file via `follow` or edits one, the
  bottom line shows a hint and a key jumps there. Auto-jump is a config
  option, off by default.
- One pane in v1. The layout tree is written so panes can be split later.

## Consequences

- `crossterm` mouse capture is on for the whole session; terminal-native
  selection needs the terminal's own modifier (Shift in Ghostty).
