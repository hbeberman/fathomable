---
type: Decision
title: Dependency policy
description: Which third-party crates Fathomable takes on, and how the policy is enforced.
resource: deny.toml
related_resources:
  - rust-toolchain.toml
  - scripts/rust-toolchain.py
tags:
  - decision
  - dependencies
---

# 0001 Dependency policy

Status: accepted (2026-08-26)

Toolchain policy amended 2026-09-19: the supported compiler floor, stable
development channel, and recorded release compiler are independent. This
replaces the earlier rule that advanced `rust-version` with every toolchain
pin.

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
| `tokio` | Tokio org | async runtime for file watching and MCP stdio |
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
| `cap-std` | bytecodealliance org | race-resistant, checkout-confined annotation reads ([0061](0061-agents-start-threads.md#checkout-confined-reads), added 2026-09-17) |

`cap-std` is explicitly approved for annotation reads. The standard library
and existing dependencies do not provide a safe capability-relative file
open that confines both symlink targets and concurrent path replacements.
Checking a canonical path before an ordinary read would leave a
check/open race; the application continues to forbid unsafe code.
Its Windows-only transitive dependency `winx` **0.36.4** is explicitly
approved under `Apache-2.0 WITH LLVM-exception`. The cargo-deny license
exception is restricted to that package and version; the global allowlist
is unchanged. Upstream gates `winx` with `cfg(windows)`, so GNU/Linux builds
do not compile or link it. It remains in the all-platform lockfile and
cargo-deny graph; `cap-std` has no feature that removes Windows dependencies.
The bundled notice inventory remains GNU/Linux-only as
specified by [0088](0088-bundled-licenses.md).

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
- Compiler compatibility follows the independent roles below.

### Rust toolchain roles

`[workspace.package].rust-version` declares the minimum supported Rust version
(MSRV), currently **1.95**. Members inherit it. This is a supported floor for
building and testing the Linux workspace with the committed `Cargo.lock`,
not a claim that older compilers cannot possibly compile the source. The
floor rises only when an intentional source or dependency change requires it,
with the manifest, CI, and documentation updated together. A release-compiler
update alone does not raise the MSRV. Lowering the floor likewise requires
evidence from the source and locked dependency graph.

The current floor matches the declared requirement of `kdl` 6.7.1 in the
committed dependency graph. All workspace targets, tests, and doctests support
Rust 1.95.0; the former 1.97 floor came from the development-toolchain pin,
not a source or dependency requirement.

`rust-toolchain.toml` selects **stable** for normal development and source
installation. Rustup's installed stable channel is updated explicitly with
`rustup update stable`; entering the checkout does not install a specific
numbered release. An explicit `cargo +VERSION` or `RUSTUP_TOOLCHAIN` can
select another supported compiler.

CI builds all workspace targets and runs tests, including doctests, with the
locked dependencies on both the declared MSRV and current stable. These jobs
use ordinary Cargo, not the contributor tool suite: the application's MSRV
does not constrain the compilers needed to install nextest, cargo-about,
public-API tooling, or other external tools.

`licenses/manifest.json` records the exact release compiler in
`rust_standard_library.release`, with its commit identity and reviewed
runtime notices. This is the single release-version source, independent of
the development channel and MSRV. Formatting, Clippy, and warnings-denied
rustdoc use this controlled compiler so their results do not drift with
stable updates. Nightly remains separate for the public-API gate and optional
pre-release unused-dependency inspection.

`scripts/rust-toolchain.py msrv` and `scripts/rust-toolchain.py release`
(invoked with `python3`) print the respective numeric version. With command
arguments, the helper executes them through `rustup run` on that version,
without changing rustup defaults or installing a compiler. Missing tools,
invalid version declarations, and command failures are explicit errors.

Contributor setup installs stable, the release compiler with rustfmt/Clippy,
and nightly. External Cargo tools are installed explicitly using stable,
regardless of a caller's toolchain override. Optional `cargo-udeps` is installed
separately with nightly and run intentionally before releases, not by the
commit gate or CI; see [release builds](../../CONTRIBUTING.md#release-builds).
The MSRV is installed by its CI job or explicitly by a contributor when needed.

`just release` checks the committed notices, then builds the locked x86_64
GNU/Linux executable using the recorded release compiler. Notice generation
also selects that compiler explicitly and verifies its release and commit
against the manifest. Changing the release compiler requires reviewing and
updating the runtime attribution inventory, not just changing a number.
Ordinary source builds may use another supported compiler, but their embedded
runtime inventory still describes the recorded release; redistribution with
another compiler requires matching runtime-notice review. See
[bundled notices](0088-bundled-licenses.md).

## Consequences

- Some conveniences (`directories`, `tui-markdown`, `ratatui-image`) are out;
  the project owns its Markdown layout and image rendering stays deferred.
- `gix` is a large dependency tree; it is accepted because the alternative is
  shelling out to host git, which the project rejects, or `git2`'s C build.
- Adding a dependency is slower than in a typical project. That is the point.
