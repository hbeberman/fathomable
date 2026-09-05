---
type: Decision
title: Resolution is the user's
description: An agent's `resolve` on a reply proposes closing the thread and nothing more; the thread stays open and waiting, its rows and headers say `proposed`, the status line and the review list count proposals, and `o` closes it. The agent force-resolve and the `auto-resolved` state go.
resource: crates/fathomable/src/app/threads/proposed.rs
related_resources:
  - crates/fathomable-core/src/annotations.rs
  - crates/fathomable-core/src/agents.rs
  - crates/fathomable/src/mcp/tools.rs
  - crates/fathomable/src/hooks.rs
  - crates/fathomable/src/app/threads/mod.rs
  - crates/fathomable/src/app/threads/words.rs
  - crates/fathomable/src/app/threads/list.rs
  - crates/fathomable/src/app/draw/mod.rs
tags:
  - decision
  - annotations
  - sessions
---

# 0053 Resolution is the user's

Status: accepted (2026-09-04)

## Context

[0005](0005-annotations.md) drew the line once: "an agent reply may
carry `proposed_resolved`; only the user resolves", with a force-resolve
kept as an escape hatch that marks the thread `auto_resolved` and shows
it "distinctly". What shipped merged the two. The one `resolve` flag on
`thread_reply` both badges the reply *proposes resolving* and resolves
the thread as the agent, so its status is `auto-resolved`: the same grey
as a user's resolve, hidden by default in the review list and the
threads pane, never waiting ([0030](0030-waiting-threads.md) rules a
resolved thread out), so `]r` never visits it and the tree never tags
its file. The only trace is the toast `reply on src/lib.rs:42,
resolved`.

The `hello` hook then shows every agent the reply call with `resolve:
true` in both example forms. Agents copy the example, so nearly every
reply closes its thread, and the user finds their review comments gone
from the inbox before they read the answers. The user raised this on
2026-09-04: agents "marking threads as resolve requested" made threads
"auto-resolve and show as closed". The parked item "Discouraging agent
force-resolve" had waited since 0005 for exactly this.

A question round the same day settled it: an agent's `resolve` is a
proposal and nothing more; a proposal is counted in the status line and
the review list's header and named on the thread's rows, with no colour
of its own; `o` accepts it, with no new key; and threads an agent
already force-resolved need no migration, since no release exists
([0051](0051-retire-one-release-compatibility.md)).

## Decision

- **A reply proposes; the user resolves.** `thread_reply { resolve:
  true }` marks the reply as proposing resolution and leaves the thread
  open. Nothing an agent sends closes a thread: `Status` is `open` or
  `resolved`, `auto-resolved` is gone, and `Store::resolve` takes no
  author. The MCP reply line reads `replied to <id>, proposing to
  resolve it`; the tool's description and the `hello` text say that
  `resolve` says the agent believes the thread is done and that the
  user closes it, and the `hello` example no longer carries `resolve:
  true` on the single-reply form.
- **A proposed thread is a waiting thread.** Its newest message is an
  agent's, so 0030 applies unchanged: the `thread.waiting` colour, the
  `↩` tag on its file, the toast, and `]r`/`[r`. The toast for a
  proposing reply reads `reply on src/lib.rs:42, proposes resolving`.
  The user's `o` resolves it, their reply keeps it open and ends the
  wait as before, and a later agent reply without `resolve` withdraws
  the proposal, since only the newest reply is read.
- **The word.** `Thread::proposes_resolution()` is true while the thread
  is open and its last reply proposes. The thread's words
  ([0032](0032-placement-and-state.md)) gain a third: after the
  placement and the state, `proposed`, so an expanded thread's header
  and the review list's entry header read `waiting · proposed` or
  `edited · waiting · proposed`. The message badge `[proposes
  resolving]` stays on the reply itself. `threads_list` and
  `threads_pending` describe such a thread as `open, proposed`.
- **The counts.** The status line's thread block reads `1 proposed  2
  waiting  3/5 threads` while the current document has proposals, the
  review list's header reads `review  4 open  1 proposed  resolved
  hidden`, and `:status` adds a `proposed` row with the workspace
  total. A proposal is also counted as waiting; the two numbers answer
  different questions (what needs an answer; what only needs a nod).

## Consequences

- `app/threads/proposed.rs`, which this record backs, holds the counts;
  `words.rs` the third word; `draw` the header and status text.
- `fathomable-core::annotations` loses `Status::AutoResolved`, and
  `Event::Resolve` its `by` field; a stored agent resolve replays as a
  user's, which the user accepted as the cost of no migration.
- `ThreadState` is three-way: `waiting`, `open`, `resolved`. 0047's
  table row is amended.
- The "Discouraging agent force-resolve" parked item closes: there is
  nothing left to discourage.
- `docs/guide.md` gains the word, the counts, the toast, and the new
  `thread_reply` wording in the same change.
