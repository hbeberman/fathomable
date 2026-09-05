---
type: Concept
title: Parked ideas and open investigations
description: Deferred features and unresolved questions, with where they came from, so they can be picked up later.
tags:
  - charter
---

# Parked ideas and open investigations

Things deliberately not done yet. Each entry names its origin so the reasoning
can be recovered. Move an entry out when it becomes a decision record or is
rejected for good (record rejections in the [charter](charter.md)).

## Deferred features

- **Sixel / Kitty image rendering** in Markdown. Origin: initial planning,
  [0004](decisions/0004-markdown-rendering.md). Depends on a graphics crate
  that meets the [dependency policy](decisions/0001-dependency-policy.md) or
  on hand-rolled escape output.
- **Rendered Mermaid diagrams** instead of a code block. Origin: initial
  planning. Likely needs image rendering first.
- **Syntax highlighting inside Markdown inline code.** Origin: initial
  planning; stretch goal. Reaffirmed parked in
  [0016](decisions/0016-syntax-highlighting.md).
- **Helix-style selection-first key grammar** as a config switch. Origin:
  [0007](decisions/0007-key-grammar-and-mouse.md).
- **Keymap remapping** through KDL config. Origin: 0007. Since
  [0045](decisions/0045-bindings-are-data.md) the binding table is the
  input; remapping is a KDL overlay on it.
- **Relative line numbers** (`rnu`) as a config option. Origin:
  [0010](decisions/0010-viewer-ux.md); absolute source lines in v1.
- **Multiple panes inside Fathomable.** Origin: initial planning; the layout
  tree is designed for it.
- **HTTP transport for the MCP server.** Origin:
  [0003](decisions/0003-sessions-and-mcp.md); reaffirmed stdio-only in
  [0014](decisions/0014-mcp-server-and-socket-v1.md) despite stateless HTTP
  in MCP 2026-07-28.
- **16-color theme fallback.** Origin: 0004; true-color first. ANSI colour
  names already work in themes ([0011](decisions/0011-theme-schema.md)).
- **Automatic light/dark theme choice** from the terminal background
  (OSC 11). Origin: 0011; `default-dark` unless configured.
- **Loading `.tmTheme` files** for code blocks. Origin: 0011; syntect's
  bundled themes only.
- **Diff-view animation** to show an agent's edits as they land: hunks lit
  in the diff faces and fading by age. Origin: initial planning; the edit
  deltas it needs are specified in
  [0015](decisions/0015-follow-mode.md). The per-document delta store
  that record described was computed on every reload and never read, so
  it was removed on 2026-09-03; a reload now yields one `Diff` for the
  change hint and nothing is kept. The timestamped delta window comes
  back with the animation.
- **macOS / Windows support.** Origin: charter; Linux only for now.

- **Editing a thread in `$EDITOR`.** Render a thread to a writable file,
  open the user's editor, read the result back as replies. Origin: comment
  box discussion, [0005](decisions/0005-annotations.md). The draft-only
  hatch shipped in [0018](decisions/0018-comment-editor.md).

## Mouse follow-ups

- **A scrollbar on the text pane**, clickable and draggable. Origin:
  [0050](decisions/0050-mouse-menus-and-gestures.md); the wheel and
  `Ctrl-d`/`Ctrl-u` cover scrolling and a scrollbar takes a column.
- **A key that opens the context menu at the text cursor**, for a
  terminal that keeps the right button for itself. Origin: 0050; the
  clickable `Space` menu is the fallback for now.

## Checkpoint follow-ups

- **Pruning checkpoints.** Origin:
  [0049](decisions/0049-inline-threads-and-the-rail.md); nothing expires
  in the first version and `--doctor` counts the store. A retention
  rule (age, count, or size) can come once the store has been used.
- **"New since checkpoint" marks** on stub and threads-pane rows, and a
  gutter or `]g` toggle for the checkpoint base. Origin: 0049; offered
  in the design round and not chosen.

## Milestone 5 follow-ups

- **Side-by-side diff view.** [0006](decisions/0006-git-access.md) allows
  it; only unified shipped. The last-seen base, changed-file jumping, and
  base refresh on commit moved to
  [0015](decisions/0015-follow-mode.md).

## Open investigations

- **"Last seen" recency heuristic.** When a view counts as read, with
  hysteresis so brief glances and rapid agent edits do not churn snapshots.
  Origin: [0006](decisions/0006-git-access.md).

- **Snapshot bounds** for "last seen" diff bases. Origin: 0006.
