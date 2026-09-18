---
type: Decision
title: Crate layout
description: Split Fathomable into a terminal-free core crate and a single binary crate.
resource: crates/fathomable-core/src/lib.rs
related_resources:
  - crates/fathomable-core/src/document.rs
  - scripts/check-boundaries.sh
  - scripts/check-public-api.sh
tags:
  - decision
  - architecture
---

# 0002 Crate layout

Status: accepted (2026-08-26)

Amended 2026-09-03 by [0048](0048-modules-by-concept.md): a third
workspace member, `fathomable-testing`, holds the test scaffolding shared
by the other crates' tests (a dev-dependency of both, permitted by Cargo
even though it depends on the core); it is never published.

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
- `fathomable`: the binary. `fathomable [PATH]` runs the TUI and
  `fathomable --mcp` the stdio MCP server (the flag form is fixed by
  [0009](0009-cli-and-diagnostics.md); amended 2026-08-26 from an earlier
  `mcp` subcommand). Owns `ratatui`, `crossterm`, and the event loop; the MCP
  mode links `rmcp` and talks to a running TUI session over a Unix socket.

The `boundaries` gate forbids `fathomable-core` from depending on `ratatui`,
`crossterm`, or `rmcp`.
Since 2026-09-05 it also rejects `env!("CARGO_MANIFEST_DIR")` anywhere but
`fathomable_testing::repo_file`. Tests locate tracked repository files at
run time so cached test binaries remain robust across worktrees and reused
build artifacts, rather than retaining a compile-time checkout path.
Native [prek checks](../commit-hooks.md) now run at a stable source root;
the runtime lookup rule remains in force.

The `public-api` gate rejects `gix` and its implementation crates in
production library signatures. Only the never-published
`fathomable-testing` crate may expose raw Git types for fixture construction;
the existing general API tripwires still apply to it.

## Consequences

- Layout produces an intermediate "rendered lines with spans and source
  ranges" structure that `fathomable` converts to ratatui widgets. That
  conversion is thin and the layout is unit-tested as plain data.
- A future HTTP MCP transport or a second frontend only touches the binary.
