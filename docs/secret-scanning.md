---
type: Software
title: Offline secret scanning
description: Pinned Betterleaks checks for staged source, introduced commits, fetched history, and exact release artifacts.
resource: scripts/betterleaks.py
related_resources:
  - scripts/betterleaks.toml
  - .github/workflows/secrets.yml
tags:
  - git
  - security
---

# Offline secret scanning

Betterleaks **1.8.1** is approved contributor tooling, not a product dependency
or a shipped component. The installer pins Linux x86_64 and aarch64 release
archives and their SHA-256 digests, reviewed against upstream's
[v1.8.1 checksums](https://github.com/betterleaks/betterleaks/releases/download/v1.8.1/checksums.txt).
It extracts only the executable and its MIT license into the current
checkout's ignored `.tmp/tools/betterleaks-1.8.1/`. No global executable or
Git hook is installed. Each worktree needs its own tool installation:

```sh
python3 scripts/betterleaks.py install
# Also included in scripts/setup-build-deps.sh.
```

Installation alone downloads public tool bytes. Scanning is offline: every
invocation explicitly sets `--validation=false` and `--redact=100`. It never
checks a credential against a provider, uploads source or findings, or reads
GitHub comments, Actions logs, or other remote private data. Missing tools,
wrong versions, invalid boundaries, shallow history, scanner failures, and
archive warnings fail closed. Tool setup does not enable GitHub settings.

## Scopes

| Trigger / command | Exact scope |
| --- | --- |
| Normal commit; `prek run --config scripts/configs/prek.toml secrets` | All tracked files as visible after prek hides unstaged tracked edits; the staged state, including unchanged tracked files |
| `just secrets` | All tracked files at their current working-tree contents, including unstaged edits; not an empty staged diff |
| `python3 scripts/betterleaks.py staged` | Only the staged Git diff, using the effective index; useful for explicit local triage |
| PR | Every commit in `base.sha..head.sha`, not merely the final tree or net diff |
| Push | Every commit in `before..after`; new refs or unavailable old force-push boundaries conservatively scan all fetched history |
| Weekly Monday 09:43 UTC; manual workflow dispatch | All fetched refs and their reachable history |
| Scanner/helper, pinned version/checksum, rule, or scanner-workflow changes | Full fetched history when those paths appear anywhere in the PR/push commit range, even if later reverted |
| `just secrets-history` | All locally available refs and their reachable history (`--all`); no network fetch |
| `just package` | The exact newly generated publishable workspace `.crate` archives selected from Cargo metadata, after package verification |
| `just release` | The exact target-directory `x86_64-unknown-linux-gnu/release/fathomable` executable, after the build |
| `just secrets-artifacts PATH...` | Only the explicitly named, nonempty regular distribution files; ignored/untracked artifacts are included |

The fast fourteenth [commit gate](commit-hooks.md) does not scan history.
Native prek saves/restores unstaged tracked edits; the helper copies only
`git ls-files` entries to an owner-private directory under `.tmp/`.
Untracked and ignored files are excluded, even though other gates can see them.
Tracked symlinks are scanned as link text, never followed; escaped parents,
unmerged entries and submodules fail rather than expanding the scope.
Alternate indexes and `git commit --only` retain native prek semantics.
Do not edit the same worktree concurrently with staged checks.

CI uses full-history checkout, read-only repository permissions, no persisted
checkout credentials, SHA-pinned Actions and a 15-minute job timeout. Scanner
execution has a separate nine-minute limit. Merge diffs include each parent,
so merge-resolution additions are not omitted. Ref deletion introduces no
commits. An invalid PR boundary fails; an unavailable old push boundary
expands coverage rather than silently skipping it.

Before publicity, explicitly fetch the intended remote branches and tags,
then run `just secrets-history` locally. Verify the fetch actually covered
the intended refs. `--all` covers available reachable history, not unreachable
objects, reflogs, other repositories, or every ref stored by GitHub. A previous
clean result does not replace this pre-publicity check.

## Trusted policy and confidential output

The independent `Secret scanning` workflow checks out the PR's immutable
base commit for the scanner helper, installer and configuration. The PR head
is input data only; its scripts and configs are not executed by that job.
Pushes and scheduled runs use the accepted repository revision. No
`pull_request_target` workflow or privileged PR execution is used. A proposed
rule upgrade is scanned using the old trusted policy before merge and the
new policy on the subsequent push.

The helper supplies an explicit trusted config, removes Betterleaks/Gitleaks
config environment overrides, disables inline allow comments, supplies an
empty ignore file, and avoids automatic source-root ignore discovery. This
last step matters in 1.8.1: `--gitleaks-ignore-path` alone does **not** prevent
loading `.betterleaksignore` from the source. No baseline or repository-wide
exception list is used. The config retains upstream detection rules and
secret-value filters, but overrides broad upstream path/extension exclusions.

Any exception must be narrowly scoped, justified using synthetic evidence,
and reviewed by a maintainer. Do not add blanket path, rule, or history
allowlists to make a failure pass. Branch protection and review of changes to
the workflows, gate, helper and policy remain essential: a PR can propose
changes to workflow YAML itself. Repository files alone cannot enforce those
external controls. The ordinary Cargo CI job executes contributor code and
is not the trusted-policy security check.

Only counts and validated rule IDs reach the console. Scanner reports stay
in memory and are never uploaded or persisted as CI artifacts. Raw stderr,
matched lines, paths, commit metadata and context are suppressed because
redacting the recognized token alone does not sanitize arbitrary diagnostics.
Warnings about unreadable files, corrupt archives or archive depth limits
reject the scan, even when upstream returns success. A maintainer needing
locations can rerun the pinned CLI privately with the same offline/redaction
flags; do not paste raw diagnostics or reports into public CI, issues or PRs.
For a real credential, stop publication, arrange revocation/rotation through
the owner, and follow the [private reporting policy](../.github/SECURITY.md).

## Distribution preflight and limitations

Artifact paths are resolved using Cargo metadata, including a custom
`CARGO_TARGET_DIR`; stale archives from other versions are not selected.
Betterleaks 1.8.1 identifies archives by extension and does not recognize
`.crate`. The helper scans byte-identical private copies named `.tar.gz`,
without extracting archive-controlled paths onto disk. Supported nested
archives are scanned up to depth eight, and encoded content to depth five.
GNU `strings --all --bytes=4` additionally feeds the exact artifact bytes'
printable strings into the scanner: ordinary directory mode skips ELF and
other application binaries. `strings` is supplied by Linux binutils.

This is pattern-based coverage, not proof that a release contains no secrets.
Unsupported/encrypted archives, unrecognized nested archive extensions
(including nested `.crate` files), non-printable binary credentials, unknown
credential formats and deeper encodings require separate review. Synthetic
tests cover added-then-deleted secrets, policy/inline/ignore bypasses, native
prek staged restoration, ignored artifacts, `.crate` contents, supported nested
archives, ELF strings and corrupt archives. They never contact providers:

```sh
python3 scripts/test-betterleaks.py
```

Scan every final distribution file again after any packaging, signing or
other transformation. Successful local package/build preflight does not
publish crates, create tags, create a GitHub release, or authorize publicity.
Those steps remain manual.

## Maintainer operations

- Verify and enable GitHub native secret scanning and push protection where
  available for the repository's visibility and plan; enable supported
  non-provider patterns and validity checks only after reviewing their data
  handling. Do not assume the local scanner config enables any GitHub feature.
- Require the independent secret-scanning check before merge and protect
  scanner/workflow policy edits with maintainer review. Verify fork-PR
  behavior and notifications on a synthetic canary without a real credential.
- Keep Actions schedules and failure notifications enabled. GitHub can disable
  public-repository schedules after 60 days without activity.
- Review Betterleaks releases periodically. Dependabot does not update this
  embedded binary pin. Update the version and both checksum constants from
  reviewed upstream release data, rerun synthetic tests, and perform a full
  history scan with the new rules before accepting the update.

These external settings and the final full-history/publicity check are
maintainer sign-off items in [dependency monitoring](dependency-monitoring.md);
checked-in automation is not evidence that they are enabled.
