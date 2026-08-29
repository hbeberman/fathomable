---
type: Decision
title: Threads follow HEAD across a rewrite
description: An open thread whose commit a history rewrite dropped, but whose lines are still in the working tree, is rescoped to the new HEAD and stays visible; recorded as a rescope event.
resource: crates/fathomable/src/app/rescope.rs
tags:
  - decision
  - annotations
  - git
  - sessions
---

# 0035 Threads follow HEAD across a rewrite

Status: accepted (2026-08-28)

## Context

[0024](0024-workspace-sessions.md) ties a thread to the `HEAD` it was
written on and shows it while that commit is `HEAD` or an ancestor. A
plain `git commit` keeps the thread: the old `HEAD` becomes the parent.
What loses it is any rewrite that takes the recorded commit out of the
ancestry: `commit --amend`, a squash or fixup, `rebase -i`, a reset and
recommit. The reader asked (2026-08-28) to commit as they go and keep
the discussions going; committing as you go, with amends and squashes,
is exactly that rewrite. The threads are not gone from the store, they
are only unreachable, and the viewer and `annotations_list` both drop
them.

Settled in a question round on 2026-08-28. The choices:

- *Which threads follow?* Open threads whose anchor still locates in
  the working tree. The lines being present is the same evidence 0024
  accepts for uncommitted lines: the work is here. Resolved threads
  keep their commit, so a finished discussion does not travel; a
  detached thread stays hidden, which keeps the branch-switch case of
  0024 intact. Following every open thread regardless of its lines was
  rejected because a branch switch would drag foreign threads along.
- *When?* On every `HEAD` change, in the viewer and on a headless
  `--mcp` read, so `annotations_list` is right with no viewer running,
  as the 0020 snapshot re-anchoring already is.
- *Persist and show?* Persist as a `rescope` event, so it survives a
  restart and bumps `updated` for `since` polling. No new state or
  colour: nothing on screen changed, the thread only moved commits.

## Decision

- `Event::Rescope { thread, commit, created }` (`"v": 2`) sets the
  thread's commit and bumps `updated`; `Store::rescope` appends it.
  The format version stays 2: an older reader rejects it as it does
  any unknown event.
- `app/rescope.rs`, which this record backs, holds `follow_head`: with
  the reachable set of 0024 in hand, every open thread whose commit is
  set and not reachable, and whose anchor locates in the file as it is
  on disk, is rescoped to the current `HEAD`. Outside git, or before
  the first commit, nothing happens.
- `App::refresh_scope` runs it before computing the scope, so the
  `.git` events that already refresh the scope ([0017](0017-git-status-navigation.md))
  carry the threads along; the headless store of `--mcp` runs it after
  0020's snapshot follow, for the same reason the viewer re-anchors
  from snapshots before scoping: a thread edited offline must locate
  before it can be kept.

## Consequences

- Amending, squashing, and rebasing a branch with review in flight
  keeps the review; the thread's `commit` is the rewritten `HEAD`, so
  the next rewrite does the same again.
- The evidence is content, so a thread on lines two branches share (a
  comment on a heading both have) follows `HEAD` when the checkout
  switches between them. That is the price of the option chosen;
  threads on the lines the work actually changed are not affected.
- A thread whose lines were dropped in the rewrite is hidden as 0024
  says; restoring the lines on a later commit does not bring it back,
  since the check runs only when `HEAD` changes and only for reachable
  history. That is the branch-switch guarantee, kept on purpose.
- One more event kind; `annotations_list` sees a bumped `updated` on
  a rescope, so an agent polling `since` re-reads the thread once.
