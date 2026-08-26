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
  planning; stretch goal.
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
- **Diff-view animation** to show an agent's edits as they land. Origin:
  initial planning.
- **macOS / Windows support.** Origin: charter; Linux only for now.

- **Editing a thread in `$EDITOR`.** Render a thread to a writable file,
  open the user's editor, read the result back as replies. Origin: comment
  box discussion, [0005](decisions/0005-annotations.md).
- **Discouraging agent force-resolve.** Beyond the `auto_resolved` flag,
  whether to warn or rate-limit. Origin: 0005.

## Milestone 5 follow-ups

- **"Last seen" diff base.** [0006](decisions/0006-git-access.md) names a
  second base, the content snapshotted when the user last viewed the file;
  milestone 5 shipped HEAD only. Needs the recency heuristic and snapshot
  bounds below. Origin: milestone-5 scope decision (2026-08-26).
- **Side-by-side diff view.** 0006 allows it; only unified shipped.
- **Jumping between changed files** (`]f`-style) needs a workspace status
  pass over the tree, and the sidebar could colour changed entries with the
  same keys. Origin: 0006.
- **Refreshing the diff base on commit.** The base is re-read on open,
  switch, and reload; a commit while viewing shows until then. A watch on
  `.git/HEAD` and the index would make it immediate. Origin: 0006.

## Milestone 1 scaffolding follow-ups

- **TODO: remaining approved dependencies.** `syntect` (core) is approved by
  [0001](decisions/0001-dependency-policy.md) but not yet added: the
  `unused-dependencies` gate (`cargo udeps`) rejects a dependency with no
  consumer, and bypassing hooks is not allowed. Add it in the commit
  that first uses it. Origin: milestone-1 scaffolding handoff (2026-08-26);
  list trimmed after milestone 2 added `gix`, `serde`, `serde_json`,
  `nucleo-matcher`, `notify`, `tokio`, and `ratatui`, and milestone 3
  added `sha2`.
- **TODO: horizontal scroll for code blocks.** The layout leaves code lines
  unwrapped ([0004](decisions/0004-markdown-rendering.md)); the viewer
  truncates them at the pane edge. Origin: milestone-1 layout engine.
- **TODO: OSC 8 hyperlinks for links.** 0004 wants clickable links;
  `ratatui` 0.30 has no hyperlink support in its buffer, so the viewer only
  colours `Face::Link` spans. Needs either a ratatui feature or raw escape
  output around the backend. Origin: milestone-1 viewer.
- **TODO: `syntect` highlighting for code blocks.** Layout emits
  `Face::CodeBlock` spans without a language; the theme names the syntect
  theme via `code.syntect` ([0011](decisions/0011-theme-schema.md)) but
  nothing reads it yet. Origin: milestone-1 layout engine.
- **TODO: automatic tree refresh.** The sidebar re-reads directories only
  on `R` or expand; a recursive workspace watch was skipped for inotify
  budget reasons. Origin: [0012](decisions/0012-workspace-mode.md).
- **TODO: `--dump-state` and `--replay-log`.** Wait on a log reader;
  `--sessions` and `--config-show` work. Origin: 0012.
- **TODO: annotations path override.** Threads live only under XDG state
  ([0013](decisions/0013-annotation-storage-and-ux.md)); a `config.kdl`
  node pointing at an in-workspace file would let a repo share threads.
  Origin: 0013 question round.
- **TODO: comment box editing keys.** The box only appends and
  backspaces; no cursor movement inside the text, no paste handling.
  Origin: 0013.

## Open investigations

- **Server-side annotation cursor per agent.** Agents poll with `since`
  ([0014](decisions/0014-mcp-server-and-socket-v1.md)); revisit if agents
  prove bad at tracking timestamps.

- **"Last seen" recency heuristic.** When a view counts as read, with
  hysteresis so brief glances and rapid agent edits do not churn snapshots.
  Origin: [0006](decisions/0006-git-access.md).

- **Re-anchoring modified lines.** When an annotated line is edited rather
  than deleted, how should the anchor move? Candidates: neighbor-context
  hashes, nearest-heading fallback, diff-based mapping via `gix`. Origin:
  [0005](decisions/0005-annotations.md).
- **Lazy follow heuristics.** How long after the agent touches a file the
  viewer should jump, and how to avoid jumping while the user is reading.
  Origin: [0006](decisions/0006-git-access.md) and charter.
- **Snapshot bounds** for "last seen" diff bases. Origin: 0006.
