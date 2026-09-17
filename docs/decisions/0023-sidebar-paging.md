---
type: Decision
title: Sidebar paging
description: The tree highlight previews a file or directory summary in the main pane, and the sidebar wheel steps one row per tick.
resource: crates/fathomable/src/app/files_pane.rs
tags:
  - decision
  - input
---

# 0023 Sidebar paging

Status: accepted (2026-08-27); amended 2026-09-04 by
[0049](0049-inline-threads-and-the-rail.md): the sidebar is the rail's
**tree pane** and the module is `app/rail.rs`; the paging rule is
unchanged. Amended 2026-09-05 by [0057](0057-the-sidebar.md): the
column is the sidebar again, the pane is the **files pane**, and the
module is `app/files_pane.rs`. Amended 2026-09-14: `l` / Right only
navigates directories; `Enter` is the file-row key that gives the text
focus. Amended 2026-09-14: a highlighted directory replaces the prior
file with a brief directory summary.

Amended 2026-09-17: sidebar paging updates the current file behind an open
Reviews view without closing it. File-scoped Reviews therefore follows the
highlight. Explicit `Enter` or a row-menu **Open** still commits to File view.

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
  an expanded directory all page the viewer.
- While the files pane owns the keys, a highlight on a *directory*
  replaces the prior file with a read-only summary: the root-relative path;
  counts of its direct files and subdirectories under the files pane's
  active filters; then, when nonzero, the changed-file and `+n -m` totals
  and the open and waiting thread counts across its whole subtree. The
  summary has no navigation hints: the files pane owns directory navigation.
- Landing keeps `open`'s semantics: paged-through files join the open-file
  history and the recent list ([0012](0012-workspace-mode.md)), and the
  file left behind is snapshotted as seen ([0015](0015-follow-mode.md)),
  exactly as an `Enter`-open always did.
- `Enter` still commits: the same open, and focus moves to the view. A
  click activates the row it hits — expanding a directory or showing a
  file — but leaves focus in the tree, so clicking is paging too
  (amended 2026-08-27; a click first committed like `Enter`).
- When Reviews owns the main column, ordinary file highlighting and clicks
  still update the current file but preserve that view and its focus. An
  explicit `Enter` or **Open** retains the committing behavior above and
  switches to File view.
- `l` / Right on a file does nothing, so directory navigation cannot
  unexpectedly leave the files pane; `h` / Left still goes to its parent.
  Descending into a directory can preview its first file, but keeps focus
  in the files pane like other paging moves (amended 2026-09-14).
- The wheel over the sidebar steps **one row per tick**, so a tick is a
  page turn. Every other pane keeps the three-line wheel.
- Only a highlight the user *moved* opens a file: the key and wheel
  handlers compare the highlighted path before and after, so `R`, `I`, and
  `Esc` never re-show a highlight the reader has since left through `[o`,
  the picker, or an agent open. The watcher's own tree refresh bypasses the
  handlers entirely and never opens anything.

## Consequences

- The tree's app-side operations move from `app/mod.rs` into
  `app/sidebar.rs` (`app/rail.rs` since 0049, `app/files_pane.rs` since
  0057), which this record backs.
- Paging past an unreadable file shows the status-line notice an
  `Enter`-open showed, and stays on the current document.
- Wheel-paging a long directory writes real entries into the history and
  the seen snapshots. That is the intent: every file paged through was
  shown.
- `Tree::current_directory_counts` reads one directory level without
  expanding it, so a collapsed directory can describe its immediate
  contents without recursively walking the workspace.
