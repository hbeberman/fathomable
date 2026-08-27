---
type: Decision
title: Dependency policy
description: Which third-party crates Fathomable takes on, and how the policy is enforced.
resource: deny.toml
related_resources:
  - rust-toolchain.toml
tags:
  - decision
  - dependencies
---

# 0001 Dependency policy

Status: accepted (2026-08-26)

## Context

Fathomable is a TUI that reads arbitrary workspaces and exposes an MCP endpoint
to agents. The supply chain is part of the attack surface. The project has a
small team and heavy agent-assisted development, so dependency review must be a
mechanical gate, not a memory.

## Decision

Take dependencies only at the scale where a compromise would be ecosystem news:
crates that are large, widely depended on, and owned by an organization or a
well-known long-tenured individual maintainer (dtolnay, BurntSushi, and peers
qualify). Prefer the standard library or an existing dependency when it is
enough. Every direct dependency requires explicit user approval and a rationale
in the pull request, per `AGENTS.md`.

Approved core set:

| Crate | Owner | Purpose |
| --- | --- | --- |
| `ratatui`, `crossterm` | ratatui org, crossterm-rs | terminal UI and input, including mouse |
| `tokio` | Tokio org | async runtime for file watching, sockets, MCP stdio |
| `notify` | notify-rs org | filesystem change notification |
| `pulldown-cmark` | pulldown-cmark org | CommonMark + GFM parsing with byte offsets |
| `syntect` (`parsing`, `default-syntaxes`, `default-themes`, `regex-fancy`, `dump-load`; no `onig`) | trishume; used by bat, delta, zola | syntax highlighting, themes (added 2026-08-26, [0016](0016-syntax-highlighting.md)) |
| `gix` | GitoxideLabs org | git status, diff, blob access |
| `rmcp` | modelcontextprotocol org | MCP server SDK |
| `serde`, `serde_json` | dtolnay | annotation and session serialization |
| `kdl` | kdl-org | configuration parsing |
| `clap` | clap-rs org | CLI |
| `anyhow`, `thiserror` | dtolnay | errors |
| `tracing`, `tracing-subscriber` | Tokio org | diagnostics |
| `unicode-width`, `unicode-segmentation` | unicode-rs org | text layout |
| `sha2` | RustCrypto org | content hashes for anchors ([0013](0013-annotation-storage-and-ux.md), added 2026-08-26) |
| `regex` | rust-lang org (BurntSushi) | `/` and `?` search patterns ([0010](0010-viewer-ux.md)) |
| `nucleo-matcher` | helix-editor org | fuzzy file picker matching ([0012](0012-workspace-mode.md), added 2026-08-26) |

Rules:

- Versions are locked in `Cargo.lock`; bumps are deliberate commits.
- `cargo deny` runs in the local gate with an explicit license allowlist
  (MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0, Zlib,
  MPL-2.0, and CC0-1.0 for `notify`, added 2026-08-26), a `crates.io`-only
  source list, and `wildcards = deny`.
- Crates that build C code (`*-sys`, `openssl`, `onig`, `libgit2`) are not
  banned but each requires its own decision record before it lands.
- Advisory ignores in `deny.toml` are for "unmaintained" notices only, each
  with a reason and the decision record that accepted it; a vulnerability is
  never ignored. (Added 2026-08-26: RUSTSEC-2025-0141 for `bincode` 1.3.3,
  which syntect's bundled dumps require; see
  [0016](0016-syntax-highlighting.md).)
- XDG directory resolution is implemented from environment variables in the
  standard library rather than adding a crate.
- The toolchain is pinned to a specific stable release in
  `rust-toolchain.toml` and bumped deliberately. Fathomable is an application
  and makes no MSRV promise.

## Consequences

- Some conveniences (`directories`, `tui-markdown`, `ratatui-image`) are out;
  the project owns its Markdown layout and image rendering stays deferred.
- `gix` is a large dependency tree; it is accepted because the alternative is
  shelling out to host git, which the project rejects, or `git2`'s C build.
- Adding a dependency is slower than in a typical project. That is the point.
