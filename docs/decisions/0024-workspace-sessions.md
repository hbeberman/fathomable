---
type: Decision
title: Workspace sessions, viewers, and git-scoped threads
description: A session is the annotation state of one workspace; running viewers are displays of it, agents bind lazily by directory, and a thread belongs to the commit it was written against.
related_resources:
  - crates/fathomable-core/src/session.rs
tags:
  - decision
  - sessions
  - annotations
  - git
  - architecture
---

# 0024 Workspace sessions, viewers, and git-scoped threads

Status: accepted (2026-08-27)

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.
`session.rs` is backed by [0062](0062-one-version-no-compatibility.md)
since 2026-09-05, which also retired `session_info` and the per-viewer
follow state it reported.

## Context

[0003](0003-sessions-and-mcp.md) made a session *one running TUI*: its id
is start time plus pid, its record and socket die with the process, and
`fathomable --mcp` binds to one id at startup and holds it for its whole
life ([0014](0014-mcp-server-and-socket-v1.md)). That broke in ordinary
use: the viewer was restarted, got a new id, and every agent tool answered
`session … is not running` until the agent went looking with `session_list`.

The identity was wrong, not the discovery. What an agent and a user share
is the annotation state of a workspace, which already lives on disk keyed
by workspace root ([0005](0005-annotations.md)) and already survives the
viewer being closed ([0020](0020-reanchoring-across-restarts.md)). A viewer
is one window onto that state; two viewers on the same repository should
show the same threads, and the state should still be readable when there
is no viewer at all.

A second gap surfaced in the same discussion. Threads are anchored by
content, so they follow lines across branches, but they do not know which
*work* they belong to. A user who reuses one checkout for several branches
sees review comments from one piece of work while looking at another.

Settled in a question round on 2026-08-27; the choices are recorded below.

## Decision

### A session is a workspace

- A **session** is the annotation state of one workspace root, keyed by the
  canonical path as the thread store already is. It exists whether or not a
  viewer is running. Keying by git common-dir (to survive renames and share
  across worktrees) was considered and parked. (Amended 2026-09-06 by
  [0070](0070-one-workspace-many-worktrees.md): the common-dir key is
  taken; a git workspace is the repository, and every worktree of it
  reads and writes one store.)
- A running TUI is a **viewer** of a session. Its runtime record and socket
  live under the workspace key:
  `$XDG_RUNTIME_DIR/fathomable/<workspace-key>/<pid>.sock`, one per viewer.
  Dead viewer records are swept as session records were.
- A viewer has an optional user-set **name** (`--name`, `:name`), shown in
  the status line and `:status`, so a user can tell an agent which window
  to drive. The pid stays as the fallback identity.
- `follow` state stays per viewer, in memory. A viewer started later sees
  nothing until the next `follow` call; keeping follow lists on disk was
  rejected because stale markers would need expiry.

### Agent binding

- `fathomable --mcp` resolves the session **on every call** by longest
  workspace-root match on its current directory. `session_switch` pins a
  workspace instead; a pin is dropped when the workspace's store is gone.
  There is no longer a viewer id to go stale.
- `session_list` lists workspaces, each with its live viewers (name, pid,
  started).
- `open` and `follow` **broadcast** to every live viewer of the session, or
  to one viewer when the call names a `viewer`. With no viewer they fail.
- `annotations_list` and `thread_reply` act on the **store**. With a live
  viewer they are forwarded to it as today; with none, `--mcp` reads and
  appends the JSONL itself, so an agent can read and answer comments after
  the user has closed the viewer. Snapshot re-anchoring (0020) therefore
  runs on a headless read as well as on viewer start, so the reported
  ranges are current either way.
- Viewers **watch `threads.jsonl`** and reload on change. That is how a
  reply written by another viewer, or by a headless `--mcp`, reaches every
  window; a viewer-to-viewer notification channel was rejected because a
  headless writer would need to know every socket and a viewer that
  appears later would still need the file.

### Threads belong to a commit

- A thread records the **HEAD commit** of the workspace at creation. Lines
  that are not yet committed carry the current HEAD too: the commit that
  eventually contains them is a descendant, so the thread stays with the
  work; discarding the changes detaches it as any lost lines do.
- The viewer and `annotations_list` show a thread only when its commit is
  HEAD or an **ancestor of HEAD**. Switching to a branch that does not
  contain the work hides its threads from both; merging or rebasing the
  work onto a branch brings them along. The agent sees exactly what the
  user sees. Listing off-branch threads with a marker, or dimming them
  in the viewer, was rejected as noise.
- Keying the store by branch was rejected because threads would not
  survive merge or rebase; keying by exact commit because every commit
  would orphan them.
- A workspace that is not a git repository, and a thread recorded before
  this change (no commit field), scope to the whole workspace as today.

## Consequences

- The socket protocol and the session record are public surfaces and both
  change shape; the protocol version is bumped and the `--sessions`
  listing groups by workspace.
- `--mcp` becomes a store writer when no viewer is running. Both writers
  append to one JSONL file, which the format tolerates; a viewer picks up
  the headless write through the watcher like any other.
- The thread record gains a `commit` field; the record version is bumped
  and old records read as unscoped.
- Reachability needs a git ancestry query per thread on HEAD change. It
  uses the existing `gix` access ([0006](0006-git-access.md)) and is
  cached per HEAD.
- The walk is bounded (2026-09-05). A wanted commit the object store no
  longer holds is unreachable without a walk, and the walk descends no
  further than a week below the committer time of the oldest wanted
  commit. Until then one thread on a commit a rebase or squash dropped,
  and a resolved thread is never rescoped
  ([0035](0035-threads-follow-head.md)), made every hook, `threads_list`,
  and reach refresh walk the whole history.
- `docs/guide.md` gains `--name`, `:name`, the `viewer` argument, and the
  new socket layout when the milestone ships.
