# Contributing

Thanks for looking. This page is what to install and what to run before
a change is ready; the reasoning behind the project lives in the
[documentation bundle](docs/index.md), starting with the
[charter](docs/charter.md) and the [design decisions](docs/decisions/index.md).
Agents and people share one rule book, [AGENTS.md](AGENTS.md).

## 1. Prerequisites

Everything in the [README install section](README.md#install), plus the
tools the commit gate runs. The `cargo-public-api` tool compiles native code
and links libcurl, so `pkg-config` and the OpenSSL headers must be present
before `cargo install` builds it. `perf` is only for
`just perf`; `just` is an optional command runner. Check recipes call prek
directly, and other recipes run their substantive commands without Make.

```sh
# Fedora
sudo dnf install gcc git curl ca-certificates tar make ripgrep \
    python3 python3-pyyaml pkgconf-pkg-config openssl-devel

# Azure Linux 3 (no ripgrep, perf, or just package)
sudo tdnf install build-essential git curl ca-certificates tar python3 \
    python3-pyyaml pkgconf-pkg-config openssl-devel
cargo install ripgrep --locked

# Azure Linux 4 (no just package)
sudo tdnf install gcc git curl ca-certificates make tar ripgrep \
    python3 python3-pyyaml pkgconf-pkg-config openssl-devel

# Ubuntu 24.04
sudo apt-get update
sudo apt-get install build-essential git curl ca-certificates tar make \
    ripgrep python3 python3-yaml pkg-config libssl-dev
```

`perf` is optional and its package must match the running kernel; install it
separately only for `just perf`. `just` is also optional: Fedora and Ubuntu
package it, while `cargo install just --locked` works where it is not
packaged. Neither is needed to run the gate.

Then, with rustup already installed:

```sh
. "$HOME/.cargo/env"
scripts/setup-build-deps.sh   # nightly, cargo tools, lychee, prek 0.5.3
scripts/install-commit-hooks.sh
# With just: just install-commit-hooks
```

The setup script pins the Cargo tool versions it installs and requests
stable, the recorded release compiler (with rustfmt and Clippy), and nightly.
It installs PyYAML with
`pip --user` only when the `yaml` module is missing; on Ubuntu pip refuses
that under PEP 668, which is why the distro package is listed above.

### Rust compiler support

The workspace supports Rust **1.95 or newer** with its committed lockfile.
`Cargo.toml` declares this minimum independently of the development channel
(`stable` in `rust-toolchain.toml`) and the exact release compiler
(`rust_standard_library.release` in `licenses/manifest.json`). Updating the
release compiler does not automatically raise the minimum. CI builds and
tests on both the minimum and current stable; contributor tools may need
newer or nightly Rust without changing the product's minimum.

`rustup update stable` explicitly updates your development compiler. To
exercise the minimum locally:

```sh
rustup toolchain install "$(python3 scripts/rust-toolchain.py msrv)" --profile minimal
python3 scripts/rust-toolchain.py msrv cargo build --workspace --all-targets --all-features --locked
python3 scripts/rust-toolchain.py msrv cargo test --workspace --all-features --locked
```

Formatting, Clippy, and warnings-denied rustdoc run with the recorded release
compiler, keeping their output consistent across developer machines. Ordinary
`cargo build`, `cargo test`, and `cargo install` use your selected compiler.
`just install` explicitly selects stable, including when a toolchain override
is set in the environment.
See the [toolchain policy](docs/decisions/0001-dependency-policy.md#rust-toolchain-roles).

Hook installation is explicit opt-in: building or installing the product
does not install hooks. The installer replaces the recognized bootstrap
hook without chaining it and refreshes a recognized prek shim. It refuses
unknown, symlinked, or non-regular `commit-msg` hooks, any
`commit-msg.legacy` entry, or an existing `core.hooksPath`; other hook types
are untouched. Default hooks are shared across linked worktrees;
installing from one affects the others, and a checkout missing `prek.toml`
fails closed. For an isolated trial before migration, see
[Commit hooks and staged gates](docs/commit-hooks.md).

Reinstalling also enables transient commit-hook history. Each Git attempt
saves a timestamped native prek trace and console log in the current
worktree's `.tmp/commit-hook-history/`. The trace records phase timings;
the console log includes failure diagnostics, total duration, and exit
status. Logs are ignored, local, and may contain source snippets. Direct
`just gates` and `prek run` do not add commit-attempt history.

## 2. The gate

`prek.toml` is the single source of truth for all 13 checks, each a local
system hook. Only `commit-msg` is installed: prek checks the message
first, then runs every check once against staged tracked contents.
Failures refuse the commit. No checks are filtered by changed filenames,
including on empty, deletion-only, documentation-only, and merge commits.
Hooks never automatically format, fix, or stage source files.

```sh
just gates                                # current checkout
prek run --config prek.toml --all-files     # same, without just
just gates-verbose                        # same, with native --verbose
prek run --config prek.toml                # staged tracked contents
prek run --config prek.toml --stage manual  # staged tracked contents
```

Native prek staged runs temporarily save and restore unstaged tracked
edits; they do **not** create a clean filesystem snapshot. Untracked and
ignored files remain visible to tools and can affect results. Do not edit
the same worktree concurrently with a commit or staged check; use separate
worktrees for parallel agents. Stage `prek.toml` when changing check
definitions. `--stage manual` alone does not select checkout semantics:
`--all-files` does, without stashing or mutating the checkout. Tools in
that mode can also see any files present.

The check order and individual checkout commands are:

| Hook ID | Checkout command | Check |
| --- | --- | --- |
| `commit-hooks` | `just test-commit-hooks` | Hook installation and native prek regression tests |
| `fmt` | `just fmt-check` | Formatting, without writing |
| `clippy` | `just clippy` | All targets and features, warnings and unsafe code forbidden |
| `nextest` | `just test` | All targets and features |
| `doctest` | `just doctest` | All library doctests |
| `okf` | `just okf` | Open Knowledge Format bundle |
| `links` | `just links` | Maintained local documentation links and anchors, offline |
| `boundaries` | `just boundaries` | Source boundaries |
| `rustdoc` | `just doc` | Documentation build, warnings denied |
| `public-api` | `just public-api` | Public API shape |
| `audit` | `just audit` | Dependency vulnerabilities |
| `deny` | `just deny` | Dependency licenses, sources, and bans |
| `licenses` | `just licenses` | Bundled notice freshness and generator tests |

Each individual check recipe calls `prek run --config prek.toml --all-files`
with the corresponding hook ID; for example,
`prek run --config prek.toml --all-files fmt`. `just docs-check` selects
`okf` and `links` through prek. `just fmt` is deliberately different:
it runs `cargo fmt` with the release compiler to format the checkout on
explicit request.

CI runs the same hooks in named steps in one main job. Its clean checkout
catches missing committed files that local untracked files might mask.

### Dependency monitoring

[Dependency monitoring](docs/dependency-monitoring.md) documents the
checked-in weekly version-update configuration, the independent daily
RustSec audit, and the repository and notification settings maintainers must
enable separately.

### Bundled license notices

The executable embeds `crates/fathomable/assets/licenses.txt`, displayed by
**Help > Licenses**. Installing the product does not run its
generator or need Python. Contributors need Python 3.11 or newer and
`cargo-about` 0.9.2, installed by `scripts/setup-build-deps.sh`:

```sh
cargo install cargo-about --locked --version 0.9.2 --features cli
```

After changing dependencies, `Cargo.lock`, the first-party license, or the
recorded release compiler, refresh the committed bundle:

```sh
cargo fetch --locked
python3 scripts/generate_licenses.py
just licenses
# Without just: scripts/check-licenses.sh
```

Generation uses `cargo-about` with `--frozen --fail` and cached package
sources, explicitly selecting the recorded release compiler rather than
the caller's active development compiler. `about.toml` holds license preferences, target selection, and
hash-checked clarifications. Cargo-about handles the dependency graph,
license recognition and deduplication; the Python adapter adds the
first-party license and supplemental attribution. Missing-file SPDX
templates are rejected without a reviewed, pinned replacement; obtain the
actual upstream license and copyright notices rather than accepting a
generic template. Separate package NOTICE/COPYRIGHT files are retained.
`licenses/manifest.json` pins supplemental notices, syntect's embedded
syntax/theme provenance, and the Rust standard-library inventory. Review and
update those records when their owners or the toolchain change; regenerating
alone is not a legal review of new dependencies. Update `about.toml`
clarification hashes only after reviewing the changed upstream terms.

The bundle covers the x86_64 GNU/Linux normal/build graph and Rust runtime
inventory, not system linker/startup objects or dynamic OS libraries. Review
additional obligations when distributing binaries, changing targets, or
statically linking native components. Keep the embedded notices and MPL
source-availability references in redistributions. See
[Bundled license notices](docs/decisions/0088-bundled-licenses.md).

Do not claim the full gate passed after running only one check. Do not
bypass hooks with `--no-verify`, `SKIP`, `PREK_SKIP`, or
`PREK_ALLOW_NO_CONFIG`. See [Commit hooks and staged gates](docs/commit-hooks.md)
for ordering, staged-content guarantees, and their limits.

Heavier diagnostics that are not part of the gate:

```sh
just mutants                        # mutation testing across the workspace
just mutants-file path/to.rs         # one file (positional path)
just perf path/to/file.md            # profile the release binary until it exits
just perf path/to/file.md fathomable # select a binary explicitly
scripts/perf-record.sh --bin fathomable -- path/to/file.md
python3 scripts/test-perf-record.py       # helper command/quoting regression test
python3 scripts/test-demo-repo.py         # demo isolation and argument regressions
python3 scripts/test-rust-toolchain.py    # compiler selection and setup regressions
```

`just perf` accepts positional `path` (default `.`) and `bin` (default
empty, no binary override). It makes an optimized release build in
`target/perf-build/`, appending `-Cforce-frame-pointers=yes` to either
`RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` without discarding caller settings,
then records `--call-graph fp`. This separate Cargo target does not replace the
normal `target/release/` binary.

Each private `target/perf/<timestamp>-<pid>/` directory contains `perf.data`,
the Cargo JSON log, terminal transcript, generated reports, and an exact copy
of the executable that perf sampled. Keep that executable with `perf.data`:
later builds cannot reconstruct missing stacks and may no longer match the
recorded build ID. The terminal transcript and reports can contain source
paths or displayed workspace content; review them before sharing. The script
prints hints when the kernel refuses `perf_event_open`.

Frame pointers make Rust caller recovery reliable without the 8 KiB user-stack
snapshot limit of perf's default DWARF mode, but they slightly change the
profiled build and do not undo compiler inlining. An 8 KiB sample that fills
the configured snapshot only proves that perf captured the requested bytes; it
does not by itself prove that unwinding a particular sample was truncated.
The controlled fixture can compare both capture methods against the same
optimized, frame-pointer-enabled binary:

```sh
proof=.tmp/perf-unwind-proof
mkdir -p "$proof"
rustc scripts/fixtures/perf-deep-stack.rs -Copt-level=3 \
    -Cdebuginfo=line-tables-only -Cforce-frame-pointers=yes \
    -o "$proof/perf-deep-stack"
perf record -q -F 997 --call-graph dwarf,8192 \
    -o "$proof/dwarf-8k.data" -- "$proof/perf-deep-stack"
perf record -q -F 997 --call-graph fp \
    -o "$proof/fp.data" -- "$proof/perf-deep-stack"
perf script --no-inline -i "$proof/dwarf-8k.data" >"$proof/dwarf-8k.script"
perf script --no-inline -i "$proof/fp.data" >"$proof/fp.script"
```

The leaf workload has 13 non-inlined `descend` frames, each retaining a 4 KiB
stack array. A chain can still stop in a native or system library built without
frame pointers, at perf's configured maximum depth, or where symbols are
unavailable. This CPU profile does not add kernel symbols or measure off-CPU
waiting.

`just install` runs `cargo +stable install --path crates/fathomable --locked`;
end users can run that Cargo command directly without installing `just`.
Replace `+stable` with another supported version to install using that compiler.

### Release builds

Run unused-dependency analysis intentionally before a release. It is not part
of the commit gate, CI, contributor setup, or `just release`. Install the
optional tool once, then run it against the workspace:

```sh
cargo +nightly install cargo-udeps --locked --version 0.1.61
just udeps
# Without just:
cargo +nightly udeps --workspace --all-targets --all-features --locked
```

Review its findings before removing dependencies; indirect uses can produce
false positives. Nightly is already installed for the public-API gate.

Prepare the two crates.io source packages from a clean release commit:

```sh
just package
# Without just:
scripts/check-licenses.sh
python3 scripts/rust-toolchain.py release cargo package \
    --workspace --exclude fathomable-testing --locked
```

This creates and builds `fathomable-core` and `fathomable` together through
Cargo's temporary local registry, proving that the application package uses
the packaged core rather than the workspace path. The package manifests omit
the path-only `fathomable-testing` dev-dependency; that fixture crate remains
unpublished. Inspect the two `.crate` archives under `target/package/`.
Packaging is local and does not upload, tag, or create a GitHub release.

Publishing and tag creation are manual maintainer actions. When the maintainer
is ready, publish the core first, wait until crates.io serves that exact
version, and then publish the application:

```sh
python3 scripts/rust-toolchain.py release cargo publish \
    --locked --registry crates-io -p fathomable-core
python3 scripts/rust-toolchain.py release cargo publish \
    --locked --registry crates-io -p fathomable
```

The GitHub tag and release must point to the same clean commit used to build
the packages. Agents and local release helpers stop after package preflight;
they do not upload crates or create the tag.

Build the attributed release executable separately:

```sh
just release
# Without just:
scripts/check-licenses.sh
python3 scripts/rust-toolchain.py release cargo build --release --locked --bin fathomable --target x86_64-unknown-linux-gnu
```

This checks the notice bundle before building with the exact compiler
recorded in `licenses/manifest.json`. With the default Cargo target directory,
the executable is `target/x86_64-unknown-linux-gnu/release/fathomable`.
The compiler must already be installed (contributor setup installs it).

Normal development, source installation, and compatibility tests can use
other supported compilers. Their embedded Rust-runtime inventory still
describes the recorded release, as the notice bundle states. Do not
redistribute a binary built with another compiler without reviewing and
updating the runtime notices to match. An MSRV test demonstrates compiler
compatibility, not notice coverage for that alternative compiler.

## 3. Working in the tree

- Product code lives under `crates/` in an edition 2024 workspace: the
  `fathomable` binary, `fathomable-core` (no terminal or MCP
  dependencies), and `fathomable-testing`. See the
  [crate layout decision](docs/decisions/0002-crate-layout.md).
- Unsafe code is forbidden compiler-wide.
- New direct dependencies need a decision record and a rationale; the
  approved set and the rules are in the
  [dependency policy](docs/decisions/0001-dependency-policy.md).
- Public APIs avoid leaking third-party types, raw synchronization or
  channel types, glob re-exports, bare `bool` mode parameters, and
  `get_` getters that are not keyed retrieval. The public API scan
  enforces these mechanically.
- Prefer behavior-level tests at public boundaries. A test that reads a
  tracked repository file locates it with `fathomable_testing::repo_file`,
  never `env!("CARGO_MANIFEST_DIR")`.
- `scripts/demo-repo.sh [DIR]` builds a throwaway workspace with seeded
  discussions for smoke-testing the MCP server and its tools. It runs the
  repository-only `seed` Cargo example; seeding is not part of the installed
  Fathomable binary. State and configuration are always isolated under the
  demo directory; there is no `--isolated` mode switch.

## 4. Documentation

Durable knowledge is an Open Knowledge Format 0.2 bundle under `docs/`;
`docs/index.md` is the entry point and `docs/okf.md` explains the format.
Before changing documented behavior, read the nearest decision record.
Every source file that backs a document carries one `@okf-doc:` backlink.
`docs/guide.md` is the human onboarding guide: update it in the same
change as any flag, key, file path, config node, or MCP tool it names.
Validate with `just docs-check`, which runs the OKF lint and the link
check.

## 5. Commits

One task per commit, committed once the gate passes. Messages follow
[Conventional Commits](https://www.conventionalcommits.org/) with a
header of at most 72 characters:

```text
<type>[optional scope][!]: <description>
```

Allowed types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`,
`build`, `ci`, `chore`, `revert`. The hook rejects `Co-Authored-By:`
lines naming Copilot, Claude, or Codex and any `Claude-Session:` line.

## 6. Agent skills

AGENTS.md asks agents to load the `rust-api-guidelines`,
`rust-msft-guidelines`, and `open-knowledge-format` skills. Only the last
is tracked, under `.agents/skills/`; the others are provided per user,
through global installs or Git-excluded local links. Do not commit
copies or machine-specific links.
