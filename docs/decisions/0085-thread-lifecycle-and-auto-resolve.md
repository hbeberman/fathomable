---
type: Decision
title: Thread lifecycle and one-shot auto-resolve
description: Threads have active, resolution-proposed, and resolved lifecycle states; user-controlled one-shot auto-resolve authorizes one agent reply, whose reply, placement, resolution, and replay outcome persist atomically.
tags:
  - annotations
  - decision
  - sessions
---

# 0085 Thread lifecycle and one-shot auto-resolve

Status: accepted (2026-09-16)

Transport amended 2026-09-18 by [0089](0089-store-only-mcp.md):
all MCP writes use the store directly and viewers reconcile observed
activity without a socket. Atomic reply authority, original retry outcomes,
and silent startup/replay behavior remain; observed bursts may aggregate.
The socket-format rules below are historical.

Origin and resolution context amended 2026-09-16 by
[0087](0087-global-comparisons-and-board-history.md): lifecycle and one-shot
permission remain, while every resolution records its own actor, time,
checkout, and version and never rewrites immutable comment origin.

Supersedes the waiting and last-act model of
[0030](0030-waiting-threads.md) and [0058](0058-the-user-has-the-last-word.md),
and replaces the human-only proposal contract of
[0053](0053-resolution-is-the-users.md). It preserves
[0072](0072-a-resolved-thread-stays-at-its-commit.md): resolution still fixes
a thread to the resolving checkout's `HEAD`.

## Context

Fathomable's stored thread status is open or resolved, but the viewer grew a
second state system around whose act was newest. That *waiting* state chose
colours, counts, traversal, notifications, and header words. It no longer
matched the simpler conversation model: the latest visible message and the
last act can differ after an edit or reopen, and proposal counts overlap
waiting counts.

The user wants agents to remain visible as they work without making
attention a thread state. The user also wants to authorize an agent to
finish one turn without granting a lasting ability to resolve discussions.

## Decision

### Three lifecycle presentations

The visible lifecycle is one of:

- **active** (`●`): unresolved, with no current resolution proposal;
- **resolution proposed** (`◐`): unresolved after an agent said its reply
  completes the work but lacked auto-resolve permission;
- **resolved** (`○`).

The current proposal is durable lifecycle state. A message records whether
it carried a resolution proposal as history, but reopening or another
ordinary message clears the current proposal without rewriting that
history. The current lifecycle is therefore not recomputed from the latest
historical flag.

An ordinary appended message supersedes a proposal. Editing an older
message does not reorder the conversation or clear it. Resolution and
reopening clear it, and reopening returns active.

Placement is independent. Anchored, reanchored, detached, and file-wide say
where a discussion is; a detached range is shown as `L42-46?` and never
replaces the lifecycle glyph. A reanchor timestamp is a lasting fact rather
than a marker cleared by a later answer.

The waiting state, last-act authority, waiting counts and traversal, and the
`thread.waiting` theme role retire. `Tab`, `Shift-Tab`, `]r`, and `[r` are
unbound. There is no replacement attention set.

### One message projection

Opening comments and replies project through one message model:

```text
Message {
    author
    body
    created_at
    modified_at
    resolution_proposed
}
```

Storage may retain distinct opening and reply events, but rendering, MCP
reads, latest-message selection, and modification calculations use this
projection. Latest means append order, not edit time. A message's
`modified_at` is its creation or later edit time. A thread's `modified_at`
is its latest persisted change, including a message, edit, lifecycle,
auto-resolve, relocation, move, or rescope event. Equal Unix-second values
are allowed.

### One-shot auto-resolve

`auto_resolve` is unresolved thread state controlled only by the user.
`R` enables or disables it. It is consumed by the next agent reply whether
or not that reply asks to resolve. Direct user resolution remains
unrestricted. Resolution and reopening leave auto-resolve disabled.

`Enter` submits or saves a user draft normally. `Ctrl-Enter` submits or
saves and enables auto-resolve in the same durable operation. `Shift-Enter`
inserts a newline; `Alt-Enter` remains an accepted fallback.

If another writer resolves the thread while a draft is open, the first
submit preserves the draft and shows a confirmation that submission will
reopen the thread. `Enter` then atomically reopens and submits, carrying the
original Ctrl-Enter auto-resolve intent; `Esc` returns to the intact draft.

### One atomic agent-reply command

The MCP `thread_reply` item has one `resolve` boolean. It means "this reply
completes the work":

- `resolve: false` records an ordinary reply. Any auto-resolve permission is
  consumed and the unresolved lifecycle is active.
- `resolve: true` with auto-resolve permission records the reply, consumes
  permission, resolves, and pins the resolved thread to the acting
  checkout's `HEAD`.
- `resolve: true` without permission records a reply carrying
  `resolution_proposed`, leaves the thread unresolved, and returns the
  successful outcome `resolution_proposed` with reason
  `pending_fathomable_user_review`.

The thread ID, not current source placement, selects the discussion. A
coordinate-free reply follows this same lifecycle when placement is detached;
an authorized resolution pins the acting bound checkout's `HEAD` and does not
claim that checkout contains or validates a fix.

The last result tells an agent not to ask for permission in chat or retry:
the Fathomable user reviews the proposal in Fathomable.

Each item is one locked append-log transaction containing the reply,
optional relocation, permission consumption, proposal transition, optional
resolution and commit pin, idempotency receipt, and original resolution
outcome. Replay recognition happens before mutable-state validation.
Receipts fingerprint the requested `resolve` intent, not the generated
message flag, and preserve the original `not_requested`,
`resolution_proposed`, or `resolved` outcome even when the returned thread
has since changed.

The annotation and internal socket formats advance exactly, with no alias,
migration, or compatibility reader.

### Complete MCP messages and batch outcomes

MCP reads return one `messages` array; every item has the same author, body,
created, modified, and `resolution_proposed` fields. Thread results expose
the lifecycle, modification time, and auto-resolve state. The `open` filter
continues to include active and resolution-proposed threads.

`thread_reply` returns request-order item results. Each completed item has
the complete current thread and its original structured resolution
outcome. Execution failures distinguish completed, failed, and unattempted
items; batch prevalidation still prevents any write, while execution
atomicity remains per item rather than for the whole batch.

### Activity, not attention

Every newly observed agent-authored opening comment or reply raises an
activity toast, including consecutive replies to one thread. User activity
does not. Proposal and resolution wording comes from the recorded
operation.

Observation uses an append-log cursor reconciled after every store refresh
and write path. Startup seeds it without replaying history; idempotent
replays do not notify again. This finds an agent event imported during a
user write before the file watcher observes it, which a before/after thread
state comparison cannot.

## Consequences

- Core state needs explicit lifecycle, auto-resolve, compound reply, user
  submit/edit, direct resolution, and observation operations rather than
  loosely coupled events.
- Resolution's commit pin and relocation become part of the same durable
  operation that reports success.
- `propose_resolve` and `proposed_resolved` retire in favour of the
  `resolve` intent and `resolution_proposed` fact.
- Historical message proposal flags remain inspectable without reviving a
  cleared current proposal.
- [0084](0084-explicit-mcp-contracts.md) remains the general schema and
  idempotency contract; this record replaces its reply-specific shape and
  result.
