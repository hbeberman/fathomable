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
- **Keymap remapping** through KDL config. Origin: 0007.
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
  [0015](decisions/0015-follow-mode.md).
- **macOS / Windows support.** Origin: charter; Linux only for now.

- **Editing a thread in `$EDITOR`.** Render a thread to a writable file,
  open the user's editor, read the result back as replies. Origin: comment
  box discussion, [0005](decisions/0005-annotations.md). The draft-only
  hatch shipped in [0018](decisions/0018-comment-editor.md).
- **Discouraging agent force-resolve.** Beyond the `auto_resolved` flag,
  whether to warn or rate-limit. Origin: 0005.

## Milestone 5 follow-ups

- **Side-by-side diff view.** [0006](decisions/0006-git-access.md) allows
  it; only unified shipped. The last-seen base, changed-file jumping, and
  base refresh on commit moved to
  [0015](decisions/0015-follow-mode.md).

## Milestone 1 scaffolding follow-ups

## Open investigations

- **"Last seen" recency heuristic.** When a view counts as read, with
  hysteresis so brief glances and rapid agent edits do not churn snapshots.
  Origin: [0006](decisions/0006-git-access.md).

- **Snapshot bounds** for "last seen" diff bases. Origin: 0006.
