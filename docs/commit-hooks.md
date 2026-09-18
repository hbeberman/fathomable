---
type: Software
title: Commit hooks and staged gates
description: Prek manages commit hooks without weakening message policy or staged-tree validation.
resource: prek.toml
related_resources:
  - scripts/install-commit-hooks.sh
  - scripts/check-commit-message.sh
  - scripts/run-staged-gates.sh
  - scripts/gates.sh
  - scripts/setup-build-deps.sh
tags:
  - git
  - onboarding
---

# Commit hooks and staged gates

[prek](https://github.com/j178/prek) manages the Git hook shim. The
repository owns the checks. Setup and CI pin prek to **0.5.3**, and
`prek.toml` requires at least that version. No remote hook repositories,
managed hook environments, or additional Rust crate dependencies are used.

## Installation and migration

Install prerequisites with `scripts/setup-build-deps.sh`, then run
`just install-commit-hooks` (or `make install-commit-hooks`). The installer
validates `prek.toml` and installs only the `commit-msg` shim, explicitly
bound to that config. It replaces the recognized bootstrap hook rather
than leaving it in prek's legacy chaining mode, which would run the full
gate twice. Reinstalling refreshes the prek shim.

The installer refuses an existing `core.hooksPath`, an unrecognized,
symlinked, or non-regular `commit-msg`, and any `commit-msg.legacy` entry.
It does not delete or chain a user's unknown hook. Resolve such conflicts
explicitly before retrying. It leaves other hook types alone.

Git's default hooks directory is shared by linked worktrees. Installing
from any worktree therefore affects all worktrees; do this only after
they contain the new config. A checkout without `prek.toml` fails closed,
not silently without checks.

To evaluate a migration branch without changing the shared hook, install
a private shim in that linked worktree's Git directory and select it for
individual commits:

```sh
git_dir=$(git rev-parse --absolute-git-dir)
prek install --config prek.toml --hook-type commit-msg --git-dir "$git_dir"
git -c core.hooksPath="$git_dir/hooks" commit
```

Use this only in a **linked worktree**, where `git_dir` is its private
administrative directory, not the main checkout's `.git`. It does not
change persistent Git configuration.

## Execution contract

Both automatic checks stay at `commit-msg`, including for Git-created
merge commits. A `pre-commit`-only gate would need separate merge handling
and would run before the commit message can be rejected.

| Order | Check | Guarantee |
| --- | --- | --- |
| 1 | `commit-message` | Existing Conventional Commits, 72-character header, and forbidden assistant-metadata rules, including existing merge/revert/autosquash exemptions. |
| 2 | `staged-gates` | Every canonical gate against the entire staged tree, even for empty, deletion-only, and documentation-only commits. |

Distinct priorities and `fail_fast` reject an invalid message before
running expensive gates. Both hooks are `always_run`; there are no path
or file-type restrictions. The message checker receives Git's message
filename, while the staged gate receives no filenames and runs once.
Successful gate output remains visible; failures preserve their details.

`scripts/run-staged-gates.sh` exports the active Git index to a temporary
directory with `git checkout-index`. It runs **that index's**
`scripts/gates.sh`, not an unstaged replacement. Git's `GIT_INDEX_FILE`
remains effective, including the temporary index used by `git commit
--only`. Linked worktrees use their own index. Snapshots are removed on
exit and builds reuse the current worktree's `target/`.

Prek's normal staged-file handling can temporarily hide unstaged tracked
edits, but it is not a clean filesystem snapshot: untracked and ignored
files remain in the checkout. The explicit snapshot is retained so those
files cannot rescue a broken staged build or contaminate a passing one.
This is filesystem isolation, not a sandbox: checks still use installed
tools, environment variables, caches, and the network where required.

Unlike the bootstrap shim, prek also temporarily saves and restores
unstaged tracked edits while running hooks, and refuses an unstaged
config change. Stage `prek.toml` when changing hook definitions, and do
not edit the same checkout concurrently with a commit. The regression
suite checks restoration after both successful and failed gates.

The canonical gate list remains in `scripts/gates.sh`, with commands
listed in [Contributing](../CONTRIBUTING.md#2-the-gate). Prek does not
replace any of its checks with a cheaper file-filtered equivalent or
automatically format, fix, or stage source files.

## Running and verifying

`make gates` / `just gates` checks the current checkout. To check the
staged snapshot without committing:

```sh
prek run --config prek.toml --stage manual
```

This command intentionally checks the **index**, not all working-tree
files. The `manual` stage excludes the message checker; `git commit`
checks both. Set `FATHOMABLE_HOOK_VERBOSE=1` for full gate output.
`prek run --config prek.toml` also selects the staged gate at prek's
default `pre-commit` stage; this does not install a second Git shim.
Even `--all-files` does not change the gate's staged-snapshot semantics.

`make test-commit-hooks` / `just test-commit-hooks` exercises real Git
commits and prek shims in temporary repositories with cheap fixture gates.
It covers installation/migration safety, message rejection, failure
propagation, staged isolation, empty/deletion/merge commits, alternate
indexes, and linked worktrees. This suite runs in the canonical gate and
CI; the actual Rust and documentation checks still run separately.

Hooks are local guardrails, not an authorization boundary. Do not bypass
them with `--no-verify`, `SKIP`, `PREK_SKIP`, `PREK_ALLOW_NO_CONFIG`,
selector-based installations, or missing-config allowances. CI remains
independent of the locally installed shim.
