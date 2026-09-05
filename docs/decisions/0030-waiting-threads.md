---
type: Decision
title: Threads waiting on the user
description: An open thread whose newest message is an agent's is waiting on the user; it gets its own gutter colour, a count in the status line and a toast when it arrives, and ]r and [r jump through them across files.
resource: crates/fathomable/src/app/threads/waiting.rs
tags:
  - decision
  - annotations
  - input
---

# 0030 Threads waiting on the user

Status: accepted (2026-08-28)

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

The charter's loop is: the user annotates, the agent reads and replies,
the user reads the reply and acts. Everything up to the reply is built
([0013](0013-annotation-storage-and-ux.md),
[0014](0014-mcp-server-and-socket-v1.md),
[0024](0024-workspace-sessions.md)), and file changes have hints,
badges, toasts, and jump keys ([0015](0015-follow-mode.md)). A reply
has none of that: the store reloads silently, the marks redraw, and
nothing says an agent answered. The user finds replies by opening the
thread list and scanning for changed counts.

Settled 2026-08-28 in a short question round. The user's framing was
the deciding one: the key should step through *threads I am not the
most recent reply on*, so they can clear what needs them quickly.

## Decision

- **Waiting.** A thread is *waiting* when it is open and its newest
  message — the last reply, or the comment itself when there are none —
  was not written by the user. Replying, resolving, or reopening it
  therefore ends the wait; nothing is tracked per viewer, nothing
  persists, and two viewers on one store agree. A resolved or
  auto-resolved thread is never waiting, so an agent's force-resolve
  with a final reply is not visited; the resolved section of the thread
  list still shows it. (Amended 2026-09-04 by
  [0053](0053-resolution-is-the-users.md): an agent can no longer
  resolve, so a reply that proposes resolving waits like any other.
  Amended 2026-09-05 by [0058](0058-the-user-has-the-last-word.md):
  the test is the thread's *last act*, so the user's edit of any
  message or reopen also ends the wait, being the agent's turn.)
- **Colour.** A waiting thread has its own mark state, drawn in the
  `annotation.waiting` face wherever a thread's state is coloured: the
  gutter bracket, the file-threads pane's dot, and the thread list's
  header row (which reads `waiting`). Waiting ranks above `open` and
  below `edited` and `detached` when one row carries several threads.
- **Count.** The status line's thread block reads `2 waiting  3/5
  threads` while the current document has waiting threads;
  `:status` adds a `waiting` row with the workspace total. The
  sidebar tags a file that has a waiting thread with `↩` after its
  name, in the same face.
- **Toast.** A store reload that turns a thread waiting — an agent's
  reply arriving — raises a toast in the follow-mode style
  ([0015](0015-follow-mode.md)): `reply on src/lib.rs:42`, or `3
  replies` when several land in one batch. The user's own reply, and
  a reload that changes nothing, raise none.
- **Keys.** `]r` jumps to the next waiting thread and `[r` to the
  previous: first the current document's waiting marks below (above)
  the cursor, then the other files' waiting threads in path order,
  wrapping, with the `wrapped` notice of `]g`. The jump opens the
  file, lands the cursor on the thread's first line, and opens its
  pane so the reply is read without a second key. When nothing is
  waiting the notice says so.

## Consequences

- `MarkKind` gains `Waiting`; `MarkKind::of` reads it from a new
  `Thread::awaits_user()` in `fathomable-core::annotations`, which
  this record's predicate defines and which the MCP list can reuse.
- `app/threads/waiting.rs`, which this record backs, holds the jump, the
  reload diff that raises the toast, and the counts; `ui` draws the
  new face, the sidebar tag, and the status block.
- `Theme` gains `annotation.waiting`; both built-in themes set it, and
  0011's key table gains the row.
- `docs/guide.md` gains the keys, the face, the tag, and the toast in
  the same change. The "Discouraging agent force-resolve" parked item
  is unchanged; this record only makes a force-resolve visible in the
  resolved section.
