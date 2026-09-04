---
type: Decision
title: Sidebar paging
description: The tree highlight is the file the main pane shows, and the sidebar wheel steps one row per tick, so the tree pages the viewer through files.
resource: crates/fathomable/src/app/rail.rs
tags:
  - decision
  - input
---

# 0023 Sidebar paging

Status: accepted (2026-08-27); amended 2026-09-04 by
[0049](0049-inline-threads-and-the-rail.md): the sidebar is the rail's
**tree pane** and the module is `app/rail.rs`; the paging rule is
unchanged.

## Context

Opening a file from the tree took an `Enter` per file
([0012](0012-workspace-mode.md)): moving the highlight showed nothing, so
comparing a handful of files meant a stream of `j Enter Space e j Enter`.
The wheel moved the highlight three rows per tick
([0007](0007-key-grammar-and-mouse.md)'s `WHEEL_LINES`), which reads fine
for a passive cursor but would skip two files per tick the moment the
highlight drives the pane.

## Decision

- The highlight is what the main pane shows. A tree key or wheel tick that
  lands the highlight on a *file* opens that file in the main pane without
  taking focus: `j`/`k`, `gg`/`G`, the wheel, and the step `l` takes into
  an expanded directory all page the viewer. A directory row leaves the
  pane on the file it already shows.
- Landing keeps `open`'s semantics: paged-through files join the open-file
  history and the recent list ([0012](0012-workspace-mode.md)), and the
  file left behind is snapshotted as seen ([0015](0015-follow-mode.md)),
  exactly as an `Enter`-open always did.
- `Enter` still commits: the same open, and focus moves to the view. A
  click activates the row it hits — expanding a directory or showing a
  file — but leaves focus in the tree, so clicking is paging too
  (amended 2026-08-27; a click first committed like `Enter`).
- The wheel over the sidebar steps **one row per tick**, so a tick is a
  page turn. Every other pane keeps the three-line wheel.
- Only a highlight the user *moved* opens a file: the key and wheel
  handlers compare the highlighted path before and after, so `R`, `I`, and
  `Esc` never re-show a highlight the reader has since left through `[o`,
  the picker, or an agent open. The watcher's own tree refresh bypasses the
  handlers entirely and never opens anything.

## Consequences

- The tree's app-side operations move from `app/mod.rs` into
  `app/sidebar.rs` (`app/rail.rs` since 0049), which this record backs.
- Paging past an unreadable file shows the status-line notice an
  `Enter`-open showed, and stays on the current document.
- Wheel-paging a long directory writes real entries into the history and
  the seen snapshots. That is the intent: every file paged through was
  shown.
