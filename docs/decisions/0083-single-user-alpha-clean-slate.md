---
type: Decision
title: Single-user alpha clean slate
description: The single-user alpha uses exact current annotation and socket formats, removes historical compatibility exceptions, and makes its one-time state reset an operator action.
tags:
  - annotations
  - configuration
  - decision
  - sessions
---

# 0083 Single-user alpha clean slate

Status: accepted (2026-09-15)

## Context

Fathomable is a single-user alpha that has not reached a release boundary.
The current-only policy of [0062](0062-one-version-no-compatibility.md)
already rejects an unexpected store or socket version, but later decisions
left a few deliberate exceptions: adopting a root-keyed state directory,
backfilling context onto older threads, and preserving subscription-era
author representations. Those exceptions now cost more than the state they
protect. The user explicitly authorizes one clean-slate reset of
Fathomable-owned state so the product can keep one current shape instead of
shipping readers and migrations for abandoned shapes.

The reset is not a fix for a live process mismatch. A viewer and its MCP
child must still be restarted from matching builds when the internal socket
rejects the other version.

## Decision

### One current boundary

The annotation event format advances once from **1 to 2** and the internal
socket protocol advances once from **4 to 5**. Store and socket guards remain
exact-only: a reader accepts only its current version and refuses another
before interpreting it. A store mismatch reports the path and both versions
with reset guidance; a socket mismatch reports both versions with restart
guidance. There is no older reader, version range, translation layer, or
automatic retry of a failed socket write.

An internal socket mismatch is resolved by stopping and restarting the
matching viewer and MCP processes, then reconnecting the host as needed. It
is not resolved by deleting annotation data.

No new format machinery is added to seen snapshots, checkpoints, workspace
markers, runtime records, or any other store merely to make every file share
the annotation or socket counters. The inert `agents.jsonl` register has no
reader and is not migrated.

### Retire the compatibility exceptions

- Git repositories continue to use their common-directory workspace key and
  share state across worktrees. Startup no longer probes, adopts, or renames
  a prior root-keyed directory. A plain directory later initialized as Git
  does not transfer its old state automatically.
- Snapshot-first and context-window re-anchoring remain current behavior for
  newly written line discussions. New line annotations and relocations carry
  their context; file-wide discussions remain context-free. There is no
  historical context backfill event or startup scan.
- The current author wire shape has one human value, `"user"`, and one
  attributed-agent object shape:
  `{"name": ..., "client": ..., "id": ...}`, with `client` and `id`
  optional. Subscription type/persona profiles, `kind`, and bare-agent
  string values are not current forms. Human labels use the configured user
  name; agent labels use the stored name, with an observed client only where
  the current display distinguishes it.

### Operator-only reset

The authorized clean-slate reset is a separate operator action after the
implementation is installed. It may discard only the app-owned Fathomable
state and runtime subtree described by the deployment procedure: annotation
stores, markers, snapshots, checkpoints, viewer records, logs, crash
reports, sockets, and inert agent registers. Configuration is preserved by
default, with obsolete settings removed manually. Repositories, Git
worktrees, harness chat history, credentials, unrelated configuration, and
other processes are outside this authorization.

Ordinary viewer or MCP startup never deletes, imports, moves, or rewrites
state to make the current build work. There is no reset command hidden in
startup and no automatic cleanup of installed hooks or configuration. The
operator performs the reset only after affected processes have stopped and
reviews the actual XDG paths before starting a matching build.

## Amendments

- [0038](0038-reanchoring-without-a-snapshot.md) keeps its mapping algorithm
  but no longer promises context backfill for pre-existing threads.
- [0058](0058-the-user-has-the-last-word.md) keeps configured human identity
  and human-mediated last-act authority, while its subscription profile and
  type-label promises are historical.
- [0062](0062-one-version-no-compatibility.md) remains the exact-only
  compatibility rule, with the current alpha counters recorded here.
- [0063](0063-a-comment-on-the-file.md) keeps file-wide comments context-free
  while its reference to the retired context-backfill API becomes historical.
- [0070](0070-one-workspace-many-worktrees.md) keeps common-dir identity and
  retires its one-time root-keyed adoption exception.
- [0082](0082-three-tool-review-core.md) keeps the three repository-bound
  tools and human-mediated discussion, while retiring startup adoption and
  historical author-profile preservation.

## Consequences

- State written by an older annotation or socket shape is refused rather
  than interpreted. The authorized operator reset is the path to a fresh
  alpha state; ordinary startup remains non-destructive.
- The three-tool review core, automatic supported-harness identity, current
  snapshot/context recovery, Git/worktree reach, and human-only resolution
  remain product behavior.
- Old `hello` and `pending` hooks and obsolete configuration entries require
  manual cleanup. The product neither edits harness configuration nor reads
  `agents.jsonl`.
- The guide documents the fresh-only paths, current author labels, and
  matching-build restart procedure without turning the deployment reset into
  application behavior.
