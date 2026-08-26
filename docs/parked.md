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
  [0003](decisions/0003-sessions-and-mcp.md).
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

## Milestone 1 scaffolding follow-ups

- **TODO: remaining approved dependencies.** `pulldown-cmark`,
  `unicode-width`, `unicode-segmentation`, `serde`, `serde_json` (core) and
  `ratatui`, `tokio`, `notify` (binary) are approved by
  [0001](decisions/0001-dependency-policy.md) but were not added during
  scaffolding: the `unused-dependencies` gate (`cargo udeps`) rejects a
  dependency with no consumer, and bypassing hooks is not allowed. Add each
  one in the commit that first uses it. Origin: milestone-1 scaffolding
  handoff (2026-08-26).
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
- **TODO: session id for log file names.** Logs are written to
  `$XDG_STATE_HOME/fathomable/log/<id>.log` where `<id>` is currently a
  process-local `<unix-seconds>-<pid>` placeholder. Replace it with the
  session id [0003](decisions/0003-sessions-and-mcp.md) defines once session
  records exist, so log, record, and socket share one name. Origin:
  milestone-1 scaffolding.
- **TODO: `--doctor` remaining checks.** 0009 also lists git and live
  sessions; those wait on the git and session subsystems.
  Origin: milestone-1 scaffolding.

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
