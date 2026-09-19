---
type: Decision
title: Git access
description: Use gix for gutter status and diff views instead of shelling out or binding libgit2.
resource: crates/fathomable-core/src/diff.rs
tags:
  - decision
  - git
---

# 0006 Git access

Status: accepted (2026-08-26)

Comparison model superseded 2026-09-16 by
[0087](0087-global-comparisons-and-board-history.md): Git still uses `gix`
and the owned line diff, but one checkout-wide direct endpoint pair now
drives paths, content, gutters, counts, and navigation. The comparison retains
the target's complete path set so historical file navigation cannot read past
that endpoint. Last-seen is removed.

Presentation amended 2026-09-18 by
[0087](0087-global-comparisons-and-board-history.md): the historical `gd` /
`:diff` toggle below is removed without an alias. Normal, Unified, and Off
are explicit session-global modes; Off reads Target only. The milestone text
below remains historical implementation context.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

Fathomable needs per-line change status for a gutter strip and whole-file
diffs. Shelling out to host git is rejected; `git2` builds libgit2 C code.

## Decision

- Use `gix` (GitoxideLabs) for repository discovery, HEAD blob access, status,
  and diffing, behind an internal `Vcs` trait in `fathomable-core`.
- Two diff bases are supported: **HEAD** (working tree vs last commit) and
  **last seen** (working tree vs the content Fathomable snapshotted when the
  user last viewed the file). When a view counts as "seen" is a recency
  heuristic with hysteresis, still open (see [parked ideas](../parked.md)).
  Snapshots live in the session state directory.
- The gutter strip colors added, modified, and removed lines like Zellij and
  editors do; diff views render side-by-side or unified in the same layout
  engine.
- Navigation commands jump between hunks and between changed files so a user
  can walk through what an agent has done.

### Milestone 5 scope (2026-08-26)

The first implementation ships the HEAD base only; "last seen" waits on the
open recency and snapshot-bound questions in [parked ideas](../parked.md).

- **Diff base.** `Workspace::head_text` (in the
  [0012](0012-workspace-mode.md) workspace module) reads the blob for a root-relative
  path from the `HEAD` tree through `gix`. Outside git there is no base;
  inside git a file `HEAD` does not have (unborn branch, untracked, newly
  added) has an empty base, so every line counts as added. The base is
  re-read when a document is opened, switched to, or reloaded after a
  change on disk, so a commit made while viewing is picked up at the next
  edit or file switch, not instantly.
- **Line diff.** `fathomable_core::diff` is an in-house linear-space Myers
  diff over lines. `gix`'s `blob-diff` feature was not enabled: it pulls the
  attribute and filter pipeline in for a viewer that only ever compares two
  UTF-8 texts, and a line diff is a hundred lines to own. Hunks keep 0-based
  old and new ranges; a pure removal has an empty new range whose start is
  the line it sits before.
- **Gutter.** The order becomes `[note][line number][space][diff bar]`, so
  the diff bar sits against the text and the annotation cell is at the far
  left (amending [0010](0010-viewer-ux.md) and
  [0013](0013-annotation-storage-and-ux.md)). The bar shows `▎` in
  `diff.plus` for added lines and `diff.delta` for lines that replace old
  ones. A removal has no line of its own, so it draws as a thin rule `▔` in
  `diff.minus` along the top of the cell of the line after it (the last
  line when the removal was at the end); the removed content is only shown
  in the diff view. Wrapped continuation rows repeat their line's colour.
- **Diff view.** `gd` (or `:diff`) toggles a unified diff of the current
  file against the base in place of the rendered view, three lines of
  context per hunk, hunks under one header when their context touches.
  Added and context rows keep their source range in the working-tree text,
  so gutter numbers, the cursor, search, selection, and annotation marks
  keep working; removed rows and headers have none. The sign column wears
  the row's face (`diff.plus`, `diff.minus`; headers use `diff.delta`). The
  status pill reads `DIFF`. Side-by-side is deferred.
- **Navigation.** `]c` and `[c` jump between hunks in either view, wrapping
  with a status message; the target of a removal is the line after it. The
  status line shows `+a -r` line counts while the file differs from HEAD.
  Jumping between changed files waits on a workspace-wide status pass.
- **Doctor.** `--doctor` reports whether the current directory is a git
  work tree and whether `HEAD` is readable.

## Consequences

- `gix` is the heaviest dependency in the tree; it is accepted under the
  dependency policy as organization-owned pure Rust.
- "Last seen" snapshots grow state; they are bounded per workspace and pruned.
- The layout engine gains `Layout::diff` and the faces `DiffAdded`,
  `DiffRemoved`, and `DiffHeader`; no theme keys are added.
