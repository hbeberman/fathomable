---
type: Decision
title: Key grammar and mouse
description: Vim grammar by default with first-class mouse support; Helix mode deferred.
resource: crates/fathomable/src/app/keys.rs
tags:
  - decision
  - input
---

# 0007 Key grammar and mouse

Status: accepted (2026-08-26)

## Context

Fathomable is a viewer, not an editor, so the key grammar is about motion,
search, selection, and commands. The user selects most annotation ranges with
the mouse.

## Decision

- Vim grammar: `hjkl`, `gg`/`G`, `Ctrl-d/u`, `/` and `?` search with `n`/`N`,
  `:` command line, visual line mode for selecting annotation ranges, marks.
- Mouse is enabled always: click to focus and place the cursor, drag to select
  lines, scroll wheel to scroll, click on tree entries and links. Mouse and
  keyboard share one selection model.
- The mouse goes to the pane under the pointer, not the focused one
  (2026-08-27): the wheel scrolls the tree, the text, or the thread pane
  it is over; a click focuses that pane (the tree's header row included).
  A press on the tree's divider column or on the thread pane's top rule
  starts a drag that resizes it; the tree keeps at least 8 columns and
  leaves the text 20, the thread pane keeps at least 3 rows and leaves
  the text one. Sizes last for the session.
- A which-key style hint bar shows pending key sequences.
- Keymap is fixed in v1; remapping through KDL config comes later.
- Helix selection-first grammar is deferred and would be a config switch.
- Cursor model, gutter, status line, clipboard, search, and Esc/quit details
  are fixed in [0010](0010-viewer-ux.md) (2026-08-26): a real row/column
  cursor, mouse release copies the source Markdown via OSC 52, `/` is regex,
  only `:q` quits (`Ctrl-c` was dropped 2026-08-26 to match vim/helix). [0013](0013-annotation-storage-and-ux.md)
  (2026-08-26) then removed copy-on-release: a mouse selection stays in
  `SEL` mode where `y` copies and `c` comments.
- Layout: a toggleable tree sidebar on the left plus a fuzzy file picker
  popup; one bottom line shared by status, `:` and `/` input, and key hints.
- Follow mode: when an agent marks a file via `follow` or edits one, the
  bottom line shows a hint and a key jumps there. Auto-jump is a config
  option, off by default.
- One pane in v1. The layout tree is written so panes can be split later.

## Consequences

- `crossterm` mouse capture is on for the whole session; terminal-native
  selection needs the terminal's own modifier (Shift in Ghostty).
