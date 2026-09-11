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
`just perf`; `just` is a thin frontend over Make and optional.

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
scripts/setup-build-deps.sh   # nightly, cargo-{nextest,audit,deny,udeps,mutants,public-api}, lychee
just install-commit-hooks     # writes .git/hooks/commit-msg
```

The setup script pins every tool version and installs PyYAML with
`pip --user` only when the `yaml` module is missing; on Ubuntu pip
refuses that under PEP 668, which is why the distro package is listed
above.

## 2. The gate

`scripts/gates.sh` is the canonical local gate. The commit hook runs it
against a snapshot of the staged tree, so a commit is refused until it
passes. `make gates` invokes it and `just gates` forwards to Make; set
`FATHOMABLE_HOOK_VERBOSE=1` (or run `make gates-verbose`) to see the
command output of a failing step. Do not bypass hooks with `--no-verify`.

| Step | Command |
| --- | --- |
| formatting | `cargo fmt --check` |
| linting | `cargo clippy --all-targets --all-features -- -D warnings -F unsafe-code` |
| tests | `cargo nextest run --all-targets --all-features` |
| doctests | `scripts/test-doctests.sh` (all library targets) |
| OKF documentation | `python3 scripts/okf-lint.py --repo-root . docs` |
| documentation links | `lychee --offline` over `docs`, the root Markdown, and the OKF skill |
| source boundaries | `scripts/check-boundaries.sh` |
| rustdoc | `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --all-features` |
| public API scan | `scripts/check-public-api.sh` |
| dependency audit | `cargo audit` |
| licenses and sources | `cargo deny check` |
| unused dependencies | `cargo +nightly udeps --all-targets --all-features` |

`Makefile` is the command source of truth; `justfile` calls the matching
Make target. Do not claim the full gate passed after running only one of
its steps.

Heavier diagnostics that are not part of the gate:

```sh
make mutants                        # mutation testing across the workspace
make mutants-file FILE=path/to.rs   # one file
just perf path/to/file.md           # profile the release binary until it exits
scripts/perf-record.sh --bin fathomable -- path/to/file.md
```

Perf artifacts land under `target/perf/`; the script prints hints when
the kernel refuses `perf_event_open`.

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
- `scripts/demo-repo.sh` builds a throwaway workspace with seeded threads
  for smoke-testing the MCP server and the agent hooks.

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
