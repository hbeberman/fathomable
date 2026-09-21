---
type: Software
title: Commit hooks and staged gates
description: Native prek checks, staged validation, transient run history, and explicit hook installation.
resource: prek.toml
related_resources:
  - scripts/install-commit-hooks.sh
  - scripts/check-commit-message.sh
  - scripts/commit-hook-history.py
  - scripts/setup-build-deps.sh
  - justfile
  - .github/workflows/ci.yml
tags:
  - git
  - onboarding
---

# Commit hooks and staged gates

[prek](https://github.com/j178/prek) generates the Git hook shim and runs
the repository's checks directly. A thin observer retains its native logs;
it does not execute or schedule individual checks. `prek.toml` is their single source of
truth: each of the 14 checks is a local `language = "system"` hook.
Setup and CI pin prek to **0.5.3**, and the config requires at least that
version. No remote hook repositories, managed hook environments, or
additional Rust crate dependencies are used.

Rust has [independent toolchain roles](decisions/0001-dependency-policy.md#rust-toolchain-roles).
Normal Cargo commands use the stable development channel. Formatting, Clippy,
and warnings-denied rustdoc select the compiler recorded in the license
manifest through `scripts/rust-toolchain.py release`; `just fmt` uses the same
compiler as the formatting gate. License generation explicitly selects and
verifies that release compiler too. The public-API check uses nightly. The test
gates use the caller's active compiler; separate CI jobs enforce the declared
MSRV and current-stable compatibility with locked dependencies.

## Installation and migration

Install prerequisites with `scripts/setup-build-deps.sh`, then run
`just install-commit-hooks` (or `scripts/install-commit-hooks.sh`).
Installation is explicit opt-in: build, product installation, and tool
setup commands do not install Git hooks automatically. The installer
validates `prek.toml` and installs only the `commit-msg` shim, explicitly
bound to that config. It replaces the recognized bootstrap hook rather
than leaving it in prek's legacy chaining mode, which would run the full
gate twice. It prepares a native shim in a temporary directory, adds the
history observer, and atomically replaces the installed hook. Reinstalling
refreshes the shim and enables history on an existing installation.

The installer refuses an existing `core.hooksPath`, an unrecognized,
symlinked, or non-regular `commit-msg`, and any `commit-msg.legacy` entry.
It does not delete or chain a user's unknown hook. Resolve such conflicts
explicitly before retrying. It leaves other hook types alone.

Git's default hooks directory is shared by linked worktrees. Installing
from any worktree therefore affects all worktrees; do this only after
they contain the config and observer script. A checkout without
`prek.toml` or `scripts/commit-hook-history.py` fails closed, not silently
without checks.

To evaluate a migration branch without changing the shared hook, install
a private shim in that linked worktree's Git directory and select it for
individual commits:

```sh
git_dir=$(git rev-parse --absolute-git-dir)
prek install --config prek.toml --hook-type commit-msg --git-dir "$git_dir"
python3 scripts/commit-hook-history.py --instrument-shim "$git_dir/hooks/commit-msg"
git -c core.hooksPath="$git_dir/hooks" commit
```

Use this only in a **linked worktree**, where `git_dir` is its private
administrative directory, not the main checkout's `.git`. It does not
change persistent Git configuration.

## Execution contract

Only the `commit-msg` shim is installed, including for Git-created merge
commits. This runs the message policy first, then all checks exactly once
per normal commit. A `pre-commit` shim would duplicate the checks and run
before the message can be rejected; a `pre-commit`-only gate would need
separate merge handling.

| Priority | Hook ID | Check |
| --- | --- | --- |
| 0 | `commit-message` | Conventional Commits, 72-character header, and forbidden assistant metadata, with existing merge/revert/autosquash exemptions |
| 1 | `commit-hooks` | Hook regression tests |
| 2 | `fmt` | Formatting check |
| 3 | `clippy` | Linting |
| 4 | `nextest` | Tests |
| 5 | `doctest` | Library doctests |
| 6 | `okf` | Documentation bundle |
| 7 | `links` | Local links and anchors |
| 8 | `boundaries` | Source boundaries |
| 9 | `rustdoc` | Documentation build |
| 10 | `public-api` | Public API shape |
| 11 | `audit` | Dependency vulnerabilities |
| 12 | `deny` | Dependency licenses, sources, and bans |
| 13 | `licenses` | Bundled notice freshness and generator tests |
| 14 | `secrets` | Pinned offline Betterleaks scan of tracked contents |

Distinct priorities and `fail_fast` enforce this order and reject an
invalid message before running expensive checks. All hooks are
`always_run`, with no path or file-type restrictions. Every check is
mandatory on every commit, even empty, deletion-only, documentation-only,
and merge commits; a failure stops the run and refuses the commit.
The message checker receives Git's message filename and runs only at
`commit-msg`. The 14 check hooks use `pass_filenames = false` and declare
`pre-commit`, `commit-msg`, and `manual` stages. Declaring those stages
does not install extra Git shims.

Prek never replaces a check with a cheaper file-filtered equivalent or
automatically formats, fixes, or stages source files. Native verbosity
controls successful check output; failures retain their diagnostics.

## Staged contents and checkout safety

Commits and staged checks use prek's native save/restore of unstaged
tracked edits. Checks see staged tracked contents, not unstaged
replacements of tracked source or scripts. `GIT_INDEX_FILE` remains
effective, including alternate indexes and the temporary index used by
`git commit --only`; linked worktrees use their own index.
Stage `prek.toml` when changing hook definitions: staged runs refuse an
unstaged config change.

The old exported clean snapshot has deliberately been removed.
**Untracked and ignored files remain visible to tools.** They can mask
missing staged files or otherwise affect checks. Local checks are not
clean-filesystem isolation or a sandbox: installed tools, environment
variables, caches, and the network can also influence results. CI's clean
checkout is the independent validation of committed files without local
untracked substitutes.

The `secrets` hook deliberately copies only tracked entries to a private scan
directory. Thus it scans staged tracked bytes during save/restore and current
tracked bytes under `--all-files`, never ignored scratch or untracked files.
Separate [secret-scanning](secret-scanning.md) commands cover fetched history
and explicitly selected distribution artifacts.

Native save/restore temporarily mutates the working tree. Do not edit the
same worktree concurrently with a commit or staged check; use separate
worktrees for parallel agents. The regression suite checks restoration
after successful and failed checks. `--all-files` avoids the save/restore
and does not mutate or stash the checkout, but tools can still see any
files present.

Checks run at a stable source root and preserve the checkout's `target/`.
The former snapshot runner already shared that build directory; native
execution retains build-cache reuse while keeping source paths stable.

## Transient commit-hook history

The installed hook saves each attempt under the current worktree's ignored
`.tmp/commit-hook-history/` directory. A UTC timestamp and unique suffix
pair two files:

- `YYYYMMDDTHHMMSS.ffffffZ-unique.prek.log`: prek's native `--log-file`
  trace, including its version, preparation steps, and individual
  `run{hook_id=...}` execution spans. A closing span's `time.busy` plus
  `time.idle` measures that phase's elapsed time, including subprocess waits.
- `YYYYMMDDTHHMMSS.ffffffZ-unique.output.log`: the console output, including
  failure diagnostics and failed hook IDs. A small header records trigger
  time, worktree, and observer PID; a footer records finish time, total
  monotonic duration in seconds, exit code, and success/failure or signal.

This measures the **commit-msg attempt**, not time an agent spent preparing
changes, composing a message, or staging files. A successful hook is not
proof Git subsequently created a commit. Message rejection and native prek
setup failures are retained, even if no expensive check ran.

The observer streams output normally, forwards interrupt/termination/hangup
signals to prek's process group, and preserves its exit status. Failure to
create or write history produces a warning rather than replacing a gate's
result. A forcibly killed process or machine shutdown can leave logs without
a footer; treat that as an incomplete attempt, not success.

Files are created owner-readable/writable only. Diagnostics may contain
source snippets and local paths: keep them transient and do not commit or
upload them automatically. There is no rotation or cleanup policy; retain or
remove old attempts as needed. Linked worktrees share the installed hook
but keep their histories separate. Direct `prek run`, `just gates`, and CI
do not use this observer, so they do not pollute commit-attempt history.

## Running and verifying

Check the **current checkout**, including unstaged tracked edits:

```sh
just gates
# Or, without just:
prek run --config prek.toml --all-files
# Native verbose output:
just gates-verbose
prek run --config prek.toml --all-files --verbose
```

Check **staged tracked contents** without committing:

```sh
prek run --config prek.toml
# Or select the manual stage:
prek run --config prek.toml --stage manual
```

Both commands exclude the message checker. The first selects prek's
default `pre-commit` stage; the second selects `manual`. The stage alone
does not choose staged versus checkout contents: `--all-files` selects
checkout semantics. Add `--verbose` to either command for full output.

`justfile` directly invokes prek for check recipes; it does not duplicate
the commands from `prek.toml`. Individual aliases, including
`fmt-check` -> `fmt`, `test` -> `nextest`, `doc` -> `rustdoc`,
and `test-commit-hooks` -> `commit-hooks`,
all use `--all-files`. Other individual check aliases use the same name
as their hook ID. `just docs-check` runs `okf` and `links` through prek.
`just fmt` is an explicit, non-hook `cargo fmt` operation on the release
compiler. `just udeps` directly runs optional, locked, nightly
unused-dependency analysis across all workspace targets and features.
`cargo-udeps` is not installed by contributor setup or run by hooks, CI, or `just release`;
invoke it intentionally [before a release](../CONTRIBUTING.md#release-builds).
`just install` explicitly runs `cargo +stable install` with locked dependencies.
`just package` checks the license bundle, locally verifies the two
publishable crates, and scans their exact `.crate` archives without uploading
either one. `just release` scans the executable after building it. Neither
creates tags or releases. Contributor setup also installs checksum-pinned
Betterleaks 1.8.1 into the current worktree's `.tmp/tools/`.
Other recipes run their substantive commands directly; see
[Contributing](../CONTRIBUTING.md#2-the-gate).

CI uses the same hooks with `--all-files`, retaining named steps in one
main job. It does not depend on a locally installed Git hook.

Additional MSRV and stable jobs build all workspace targets and run tests
and doctests with `--locked`, without requiring the external gate tools to
support the product's MSRV. The main job also runs the demo, profiler, and
toolchain-helper and secret-scanner regression tests. The license gate includes the
toolchain-helper tests as well as notice-generation tests.

`just test-commit-hooks` exercises real Git commits and prek shims in
fixture repositories with cheap stand-in checks.
It covers installation/migration safety, message rejection, failure
propagation, native staged-content handling and its untracked-file limits,
empty/deletion/documentation/merge commits, alternate indexes,
`git commit --only`, linked worktrees, native timing/error logs, and signal
forwarding. This suite runs as a check in
the gate and CI; the actual Rust and documentation checks still run
separately.

Hooks are local guardrails, not an authorization boundary. Do not bypass
them with `--no-verify`, `SKIP`, `PREK_SKIP`, `PREK_ALLOW_NO_CONFIG`,
selector-based installations, or missing-config allowances. CI remains
independent of the locally installed shim.
