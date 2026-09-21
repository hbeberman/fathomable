---
type: Decision
title: Crate layout
description: Separate the terminal-free core, binary, and unpublished test support.
resource: crates/fathomable-core/src/lib.rs
related_resources:
  - Cargo.toml
  - crates/fathomable/Cargo.toml
  - crates/fathomable-core/Cargo.toml
  - crates/fathomable-testing/Cargo.toml
  - crates/fathomable-core/src/document.rs
  - scripts/check-boundaries.sh
  - scripts/check-public-api.sh
tags:
  - architecture
  - decision
---

# 0002 Crate layout

Status: accepted (2026-08-26)

MCP access amended 2026-09-18 by [0089](0089-store-only-mcp.md):
the binary's MCP mode uses the core store directly rather than talking to a
viewer socket. Crate boundaries and the stdio MCP transport remain unchanged.

Distribution amended 2026-09-19: the first public alpha is prepared as
separate `fathomable-core` and `fathomable` crates.io source packages.
`fathomable-testing` remains unpublished, and publishing and GitHub tag
creation remain manual maintainer actions.

Amended 2026-09-03 by [0048](0048-modules-by-concept.md): a third
workspace member, `fathomable-testing`, holds the test scaffolding shared
by the other crates' tests (a dev-dependency of both, permitted by Cargo
even though it depends on the core); it is never published.

## Context

Rendering, annotation anchoring, session state, and the MCP server must be
tested without a terminal, and the `boundaries` gate should be able to enforce
that the terminal never leaks into that logic.

## Decision

Three workspace crates:

- `fathomable-core`: document model, Markdown layout with source mapping,
  syntax highlighting, annotations and anchors, sessions, git diffing,
  configuration. Depends on no terminal crate. Its public API is the boundary
  the `public-api` gate tracks.
- `fathomable`: the binary. `fathomable [PATH]` runs the TUI and
  `fathomable --mcp` the stdio MCP server (the flag form is fixed by
  [0009](0009-cli-and-diagnostics.md); amended 2026-08-26 from an earlier
  `mcp` subcommand). Owns `ratatui`, `crossterm`, and the event loop; the MCP
  mode links `rmcp` and talks to a running TUI session over a Unix socket.
- `fathomable-testing`: repository-only temporary workspace and Git fixtures
  shared by tests in the product crates.

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

### Package metadata

All three crates inherit the shared repository URL and homepage
(`https://github.com/hbeberman/fathomable`) and license metadata from
`[workspace.package]`. Each member explicitly opts into those workspace
values; defining them only at the workspace does not populate package
metadata.

The shared author is Henry Beberman. The application uses the repository
README, while `fathomable-core` ships its own short library README.
The application excludes `examples/seed.rs` from its source package; the
example remains available in the checkout for `scripts/demo-repo.sh`.

The MIT SPDX identifier remains in `license`, while `license-file` points
to the root `LICENSE`. Members inherit both so Cargo includes that same
copyright and permission notice as `LICENSE` in each package without
maintaining duplicate source files. The binary separately embeds the
[third-party notice bundle](0088-bundled-licenses.md).

The product crates permit only the crates.io registry. Their shared test
support is a path-only dev-dependency, which Cargo omits from the published
manifests, and `fathomable-testing` remains `publish = false`. Package both
product crates in one workspace command so Cargo's temporary local registry
verifies the application against the packaged core before either exists on
crates.io. `just package` uses fresh temporary package and build directories:
Cargo's temporary-registry verification can otherwise reuse core sources or
compiled artifacts from an earlier package with the same version. The
normal build cache is left untouched; verification rebuilds dependencies from
their cached downloads. Verified, scanned archives move
unchanged to the configured target's `package/` directory.

Actual publication is deliberately not automated. A maintainer publishes
`fathomable-core` first, waits for that exact version to become available,
and then publishes `fathomable`. The maintainer separately creates the GitHub
tag and release at the same clean commit.

## Consequences

- Layout produces an intermediate "rendered lines with spans and source
  ranges" structure that `fathomable` converts to ratatui widgets. That
  conversion is thin and the layout is unit-tested as plain data.
- A future HTTP MCP transport or a second frontend only touches the binary.
- `just package` is the local source-distribution preflight: it isolates
  Cargo's temporary registry, runs workspace packaging verification, and
  scans the resulting archives. It uploads nothing.
