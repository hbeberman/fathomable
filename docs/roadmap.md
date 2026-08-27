---
type: Concept
title: Roadmap
description: Ordered milestones toward the charter, each small enough to ship and use.
tags:
  - charter
---

# Roadmap

Milestones are ordered; each is usable on its own. Details live in the
[decisions](decisions/index.md); deferred work lives in
[parked ideas](parked.md).

1. **Single-file Markdown view.** `fathomable README.md` renders Markdown
   with the layout engine ([0004](decisions/0004-markdown-rendering.md)),
   live-reloads on change preserving position, Vim scrolling and `/` search,
   bottom-line status, `--doctor` and file logging
   ([0009](decisions/0009-cli-and-diagnostics.md)). Default light and dark
   themes.
2. **Workspace mode.** Toggleable tree sidebar plus a fuzzy file picker,
   `fathomable [DIR]`, session records and socket
   ([0003](decisions/0003-sessions-and-mcp.md),
   [0012](decisions/0012-workspace-mode.md)).
3. **Annotations.** Mouse drag and visual selection, comment box, JSONL
   threads, anchors ([0005](decisions/0005-annotations.md)).
4. **MCP server.** `fathomable --mcp` with session tools, `open`, `follow`,
   `annotations_list`, `thread_reply`.
5. **Git.** Gutter strip, HEAD and last-seen diff views, hunk navigation
   ([0006](decisions/0006-git-access.md)).
6. **Follow mode.** Change hints, badges, toasts, jump keys, debounced
   auto-jump, and the last-seen diff base
   ([0015](decisions/0015-follow-mode.md)).
