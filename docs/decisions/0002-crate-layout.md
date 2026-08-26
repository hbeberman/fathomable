---
type: Decision
title: Crate layout
description: Split Fathomable into a terminal-free core crate and a single binary crate.
tags:
  - decision
  - architecture
---

# 0002 Crate layout

Status: accepted (2026-08-26)

## Context

Rendering, annotation anchoring, session state, and the MCP server must be
tested without a terminal, and the `boundaries` gate should be able to enforce
that the terminal never leaks into that logic.

## Decision

Two workspace crates:

- `fathomable-core`: document model, Markdown layout with source mapping,
  syntax highlighting, annotations and anchors, sessions, git diffing,
  configuration. Depends on no terminal crate. Its public API is the boundary
  the `public-api` gate tracks.
- `fathomable`: the binary. Subcommands `fathomable [PATH]` (the TUI) and
  `fathomable mcp` (stdio MCP server). Owns `ratatui`, `crossterm`, and the
  event loop; the MCP subcommand links `rmcp` and talks to a running TUI
  session over a Unix socket.

The `boundaries` gate forbids `fathomable-core` from depending on `ratatui`,
`crossterm`, or `rmcp`.

## Consequences

- Layout produces an intermediate "rendered lines with spans and source
  ranges" structure that `fathomable` converts to ratatui widgets. That
  conversion is thin and the layout is unit-tested as plain data.
- A future HTTP MCP transport or a second frontend only touches the binary.
