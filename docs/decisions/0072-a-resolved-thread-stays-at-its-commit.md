---
type: Decision
title: A resolved thread stays at its commit
description: Resolving a thread fixes it to the commit that is HEAD at that moment, and a resolved thread shows in the file only while that commit is HEAD; the next commit or checkout takes it out of the text, the gutter, the tools, and the hooks, and the review list and the threads pane list it under x with its commit named, where o brings it back.
resource: crates/fathomable-core/src/reach.rs
tags:
  - decision
  - annotations
  - git
---

# 0072 A resolved thread stays at its commit

Status: accepted (2026-09-09)

Amended 2026-09-16 by
[0085](0085-thread-lifecycle-and-auto-resolve.md): direct user resolution
and an authorized one-shot agent reply both atomically pin the resolved
thread to the acting checkout's `HEAD`. An unauthorized completion reply
leaves it open as `resolution_proposed`. The reach, past-history, and
reopen rules below are unchanged; references to user-only resolution are
historical.

## Context

A thread belongs to the commit it was written against and shows while
that commit is `HEAD` or one of its ancestors
([0024](0024-workspace-sessions.md)). Resolving changed the thread's
state and nothing else: the stub went, and the entry left the review
list until `x`, but the grey circle stayed in the gutter, the lines
kept their tint, a detached one kept its `?` row, and the thread
cursor still stopped on it ([0039](0039-gutter-colour-and-detached-rows.md),
[0049](0049-inline-threads-and-the-rail.md)). Since an ancestor of
`HEAD` is an ancestor of every later `HEAD` on the branch, a resolved
thread never left. The user reported on 2026-09-09 that opening a file
on another machine showed resolved threads from commits long past, and
asked that a thread resolved against a `HEAD` attach to that commit and
show only when that commit is what the viewer is looking at, so that
iterating does not pile up stale threads.

[0035](0035-threads-follow-head.md) already fixed half of this: an open
thread follows `HEAD` across a rewrite, and "resolved threads keep
their commit, so a finished discussion does not travel". What was
missing was a reach rule that treats a finished discussion as
finished. The viewer reads the working tree, so "viewing that commit"
means the commit is `HEAD`.

A question round on 2026-09-09 settled the rest: the review list under
`x` still lists resolved threads of earlier commits, naming the commit;
a thread resolved with its fix uncommitted hides on the commit that
lands the fix; the `threads` tool's `resolved` and `all` shrink to the
current `HEAD`; showing a commit's resolved threads in the diff view
when that commit is one of its sides is left for a later milestone.

## Decision

### Resolving fixes the thread to `HEAD`

- `Store::resolve` takes the workspace's `HEAD` commit. When the thread
  has a commit and it is not `HEAD`, or when it has none and `HEAD`
  exists, a `Rescope` event to `HEAD` ([0035](0035-threads-follow-head.md))
  is appended before the `Resolve`. The format version stays the same:
  both events exist. Outside git nothing is rescoped.
- Reopening changes nothing about the commit: an open thread is back
  under the ancestor rule and shows again wherever `HEAD` reaches it.

### The reach knows `HEAD`

- `Reach` moves from `annotations` to `fathomable_core::reach`, which
  this record backs, and carries the checkout's `HEAD` beside the
  reachable set, and each other worktree's `HEAD` beside its set
  ([0070](0070-one-workspace-many-worktrees.md)).
- An open thread is reached when its commit is `HEAD` or an ancestor,
  as before. A resolved thread is reached only when its commit **is**
  `HEAD`. A thread without a commit, or a workspace without git, is
  reached as before.
- `Reach::past` names the third case: a resolved thread whose commit
  the checkout in hand reaches but which is not `HEAD`. It is not shown
  in the file; it is what the review lists under `x`.

### What the file shows

- The marks of a document are the threads the reach shows here, so a
  resolved thread of an earlier commit has no gutter circle, no line
  tint, no stub, no detached row, and the thread cursor does not stop
  on it. `Space v x` and `threads { stubs-resolved }` give a stub to
  the resolved threads at `HEAD` alone.
- The `threads` tool's `resolved` and `all`, the hooks, and a live
  viewer's answer to them list the same threads the file shows.

### The review list and the threads pane

- Under `x`, the review list and the threads pane list the resolved
  threads at `HEAD` and the past ones, files in the files pane's order
  and threads by line, each past entry dimmed as resolved entries are
  and its header naming its commit, `abc1234`, after the state words
  where a worktree's branch goes. The resolved count in both headers
  counts them, so the count says what `x` reveals.
- A past entry is placed at the lines its record holds; no placement
  word is shown, since nothing in the file is located. `Enter` opens
  the file and puts the cursor on those lines; there is no block to
  expand. `o` reopens it and it returns to the file; `dd` deletes it.
  `c` and `e` are refused with a notice naming the commit and `o`, so
  the thread is not written in from the past.
- Off-branch resolved threads, whose commit `HEAD` does not reach, stay
  hidden everywhere as they were.

## Consequences

- Iterating leaves nothing behind: every resolved thread drops out of
  the file on the next commit, and comes back when that commit is
  checked out again. A thread resolved while its fix is uncommitted
  hides on the commit that lands the fix, which is the intent.
- Reopening a thread after `HEAD` has moved is done from the list
  under `x`; the file offers no way to it.
- The history walk still asks about resolved threads' commits, since
  the list needs to know which past threads are on this branch. The
  bound of [0024](0024-workspace-sessions.md) holds.
- Agents are unaffected by default: `threads` lists open threads. A
  call with `resolved` or `all` sees this `HEAD`'s resolved threads
  and no earlier ones.
- The diff view can pick a commit as a side; showing that commit's
  resolved threads there is the true "viewing that commit" and is
  parked for a later milestone.
- The reach's `HEAD` makes every commit a reach change even when the
  reachable set is unchanged, so the marks refresh on each commit; the
  walk was already made on each `HEAD` change.
- `docs/guide.md` gains the rule, the past entries, and the tool
  wording.
