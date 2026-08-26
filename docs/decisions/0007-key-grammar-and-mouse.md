---
type: Decision
title: Key grammar and mouse
description: Vim grammar by default with first-class mouse support; Helix mode deferred.
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
- A which-key style hint bar shows pending key sequences.
- Keymap is fixed in v1; remapping through KDL config comes later.
- Helix selection-first grammar is deferred and would be a config switch.
- One pane in v1. The layout tree is written so panes can be split later.

## Consequences

- `crossterm` mouse capture is on for the whole session; terminal-native
  selection needs the terminal's own modifier (Shift in Ghostty).
