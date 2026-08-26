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
- **Multiple panes inside Fathomable.** Origin: initial planning; the layout
  tree is designed for it.
- **HTTP transport for the MCP server.** Origin:
  [0003](decisions/0003-sessions-and-mcp.md).
- **16-color theme fallback.** Origin: 0004; true-color first.
- **Diff-view animation** to show an agent's edits as they land. Origin:
  initial planning.
- **macOS / Windows support.** Origin: charter; Linux only for now.

- **Editing a thread in `$EDITOR`.** Render a thread to a writable file,
  open the user's editor, read the result back as replies. Origin: comment
  box discussion, [0005](decisions/0005-annotations.md).
- **Discouraging agent force-resolve.** Beyond the `auto_resolved` flag,
  whether to warn or rate-limit. Origin: 0005.

## Open investigations

- **Agent identity and impersonation.** What is deterministic from the MCP
  `initialize` handshake (client name, version, process ancestry) versus a
  self-declared persona; how replies show provenance when several agent
  types collaborate on one workspace. Origin:
  [0003](decisions/0003-sessions-and-mcp.md).
- **"Last seen" recency heuristic.** When a view counts as read, with
  hysteresis so brief glances and rapid agent edits do not churn snapshots.
  Origin: [0006](decisions/0006-git-access.md).

- **Re-anchoring modified lines.** When an annotated line is edited rather
  than deleted, how should the anchor move? Candidates: neighbor-context
  hashes, nearest-heading fallback, diff-based mapping via `gix`. Origin:
  [0005](decisions/0005-annotations.md).
- **MCP to session binding details.** Exact socket protocol, versioning,
  auth on the Unix socket, and how an agent learns a session exists. Origin:
  0003; needs a dedicated session.
- **Annotation consumption flow.** Whether the agent polls only on request or
  Fathomable offers a "since last read" cursor per agent. Origin: 0005.
- **Lazy follow heuristics.** How long after the agent touches a file the
  viewer should jump, and how to avoid jumping while the user is reading.
  Origin: [0006](decisions/0006-git-access.md) and charter.
- **Snapshot bounds** for "last seen" diff bases. Origin: 0006.
