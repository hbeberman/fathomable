# Contributing

Thanks for looking. This page is what to install and what to run before
a change is ready; the reasoning behind the project lives in the
[documentation bundle](docs/index.md), starting with the
[charter](docs/charter.md) and the [design decisions](docs/decisions/index.md).
Agents and people share one rule book, [AGENTS.md](AGENTS.md).

## 1. Prerequisites

Everything in the [README install section](README.md#install), plus the
tools the commit gate runs. Two of the cargo tools compile native code:
`cargo-udeps` links OpenSSL and libssh2 through the `cargo` crate, and
`cargo-public-api` links libcurl, so `pkg-config` and the OpenSSL headers
must be present before `cargo install` builds them. `perf` is only for
`just perf`; `just` is an optional command runner. Check recipes call prek
directly, and other recipes run their substantive commands without Make.

```sh
# Fedora
sudo dnf install gcc git make ripgrep python3 python3-pyyaml \
    pkgconf-pkg-config openssl-devel perf just

# Azure Linux 3 (no ripgrep, perf, or just package)
sudo tdnf install build-essential git python3 python3-pyyaml \
    pkgconf-pkg-config openssl-devel ca-certificates
cargo install ripgrep --locked

# Azure Linux 4 (no just package)
sudo tdnf install gcc git make tar ripgrep python3 python3-pyyaml \
    pkgconf-pkg-config openssl-devel perf ca-certificates

# Ubuntu 24.04
sudo apt install build-essential git curl make ripgrep python3 python3-yaml \
    pkg-config libssl-dev linux-tools-$(uname -r) just
```

`just` is `cargo install just --locked` where it is not packaged. Each
line was run in the distribution's official container image, followed by
the setup script and the full gate.

Then, with rustup already installed:

```sh
scripts/setup-build-deps.sh   # nightly, cargo tools, lychee, prek 0.5.3
just install-commit-hooks     # installs or migrates the prek commit-msg shim
# Without just: scripts/install-commit-hooks.sh
```

The setup script pins every tool version and installs PyYAML with
`pip --user` only when the `yaml` module is missing; on Ubuntu pip
refuses that under PEP 668, which is why the distro package is listed
above.

Hook installation is explicit opt-in: building or installing the product
does not install hooks. The installer replaces the recognized bootstrap
hook without chaining it and refreshes a recognized prek shim. It refuses
unknown, symlinked, or non-regular `commit-msg` hooks, any
`commit-msg.legacy` entry, or an existing `core.hooksPath`; other hook types
are untouched. Default hooks are shared across linked worktrees;
installing from one affects the others, and a checkout missing `prek.toml`
fails closed. For an isolated trial before migration, see
[Commit hooks and staged gates](docs/commit-hooks.md).

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
| `unused-dependencies` | `just udeps` | Unused dependencies, using nightly |

Each individual check recipe calls `prek run --config prek.toml --all-files`
with the corresponding hook ID; for example,
`prek run --config prek.toml --all-files fmt`. `just docs-check` selects
`okf` and `links` through prek. `just fmt` is deliberately different:
it runs `cargo fmt` to format the checkout on explicit request.

CI runs the same hooks in named steps in one main job, including unused
dependencies. Its clean checkout catches missing committed files that
local untracked files might mask.

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
```

`just perf` accepts positional `path` (default `.`) and `bin` (default
empty, no binary override). Perf artifacts land under `target/perf/`;
the script prints hints when the kernel refuses `perf_event_open`.

`just install` runs `cargo install --path crates/fathomable --locked`;
end users can run that Cargo command directly without installing `just`.

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
- `scripts/demo-repo.sh` builds a throwaway workspace with seeded
  discussions for smoke-testing the MCP server and its tools.

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
