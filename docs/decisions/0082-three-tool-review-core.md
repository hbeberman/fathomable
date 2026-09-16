---
type: Decision
title: Three-tool review core
description: Fathomable exposes only repository-bound thread reading, starting, and replying; discussion state stays human-governed and viewer navigation stays manual.
resource: crates/fathomable/src/mcp/tools.rs
related_resources:
  - crates/fathomable/src/app/agents.rs
tags:
  - annotations
  - architecture
  - decision
  - sessions
---

# 0082 Three-tool review core

Status: accepted (2026-09-15)

Supersedes the agent subscription, delivery, watch, viewer-routing, and
auto-jump contracts of [0014](0014-mcp-server-and-socket-v1.md),
[0015](0015-follow-mode.md), [0031](0031-lazy-follow.md),
[0040](0040-agent-subscriptions-and-hooks.md),
[0042](0042-turn-start-delivery.md), and
[0055](0055-six-tools.md). It amends, rather than replaces, the
human-authority and automatic-identity contracts of
[0058](0058-the-user-has-the-last-word.md) and
[0080](0080-automatic-chat-identity.md).

## Context

Fathomable accumulated two responsibilities around its thread store: review
discussions, and automatic delivery and viewer control that subscribed chats,
tracked delivery, installed harness hooks, routed individual calls among
workspaces and viewers, and moved the viewer on an agent's behalf. The
second responsibility introduced state that did not express the user's actual
decision. Reading a thread looked like consumption, an agent reply could
make work disappear from another agent's queue, and hooks still could not
reliably wake an idle chat.

The useful core is smaller. A reviewer and the user discuss code in threads;
the user may then hand that discussion to a coder to defend or implement the
agreement. The user's words and task assignment are the authority. No
delivery ledger, acknowledgement bit, or structured approval state improves
that handoff.

## Decision

### One repository-bound server, three tools

`fathomable --mcp [DIR]` binds to the repository or checkout containing
`DIR`, or the server's startup directory when `DIR` is omitted. The binding
does not change. It discovers the checkout directly without workspace-marker
setup. Tools have no per-call workspace or viewer selector.

The complete MCP surface is:

- `threads`: read discussions.
- `thread_start`: start one or more discussions.
- `thread_reply`: continue one or more discussions.

`workspaces`, `open`, `follow`, and `thread_watch` are removed without
compatibility aliases. MCP never opens a file, moves a viewer, subscribes a
chat, or promises future delivery.

One git repository still has one shared thread store across its worktrees
([0070](0070-one-workspace-many-worktrees.md)). A viewer can still page
manually among those worktrees. The MCP binding fixes the repository and
the checkout for new comments, not a snapshot of its worktree list. Reads
include discussions reached by its current worktrees, with the reaching
worktree named when it differs from the bound checkout. Replies use that
discussion's worktree for placement, without moving any viewer.
Cross-workspace routing is a possible future facility, not retained dead code.

### Reading is observation

With no arguments, `threads` returns the first page of all eligible open
discussions in deterministic `(updated, id)` order, regardless of who wrote
last. `status` is `open`, `resolved`, or `all`; `path` narrows to a
repository-relative file exactly or a directory subtree; inclusive `since`
is only an update-time lower-bound filter; `limit` defaults to 50 and zero
is coerced to 1. `after: {updated, id}` is the exclusive continuation pair
and may be combined with the same filters. The response's `more` counts
omitted results and `next_after` is the last returned pair when more remain,
otherwise null. Equal timestamps therefore progress without repeats or
skips. `ids` alone retrieves exact discussions in the requested order,
cannot combine with any selector including `after`, rejects duplicates or
missing ids, and returns `more: 0, next_after: null`.
Unlike the ordinary repository-reach filters, this is a direct store lookup:
it reliably retrieves named historical or resolved discussions even when
ordinary reach or status visibility would omit them. Exact lookup retains
the same current worktree and placement metadata as listing. These selectors
change no state.
A read does not acknowledge, assign, consume, deliver, or change a thread.
Every returned discussion carries its id, file, optional range, placement,
status, created and updated times, original author and comment, every reply
with its serialized author/body/proposal/edit data, and optional snippet,
commit, edited, and `worktree` metadata; `worktree` identifies another
worktree when it is the checkout reaching or placing the discussion.
Resolved history is not collapsed. Results carry no `pending`, `answered`,
`messages`, delivery, assignment, acknowledgement, or compact-resolved
shape.

There is no `pending` view of agent obligations. A reply by one agent does
not make the discussion irrelevant to another. Callers decide what matters
from the user's prompt, the thread history, and their assigned task.

Required annotation housekeeping runs once when the MCP server starts:
legacy root-keyed state adoption, offline snapshot/context re-anchoring, and
stranded-open follow-HEAD rescoping may append or move their existing state
before calls are accepted. A `threads` call itself reads that state and
computes placement without persisting re-anchor or rescope changes. All MCP
thread reads project directly from the shared store, including placement
after line edits and visibility across an amend or rebase while the server
stays running. Exact `ids` additionally bypass ordinary reach and status
visibility. Viewer startup performs its housekeeping independently.

### Writes are batch-shaped and prevalidated

`thread_start` requires a non-empty `comments` array. Each item is
`{path, line?, end_line?, body}`; omitting `line` starts a file-wide
discussion.

`thread_reply` requires a non-empty `replies` array. Each item is
`{thread, body, resolve?, line?, end_line?}`. `resolve` is a proposal:
only the human closes a thread
([0053](0053-resolution-is-the-users.md)). A range can re-anchor a
line-anchored discussion after a rewrite; file-wide discussions reject range
overrides. Errors retain useful re-anchor hints.

Both tools validate the entire batch before writing anything. The former
top-level single-item arguments are removed rather than accepted as aliases.
Unknown fields are rejected. Starting validates repository-relative existing
text files, non-empty bodies, and 1-based in-bounds ranges (`end_line`
requires `line`). Replying rejects duplicate or missing ids, resolved
threads, empty bodies, invalid ranges, and detached threads without a new
line. Prevalidation is not an I/O transaction: if a later write fails after
earlier items succeeded, the error names the completed items so a caller can
retry only what remains.

### Identity records authorship, not routing or registration

The harness-qualified automatic identity of
[0080](0080-automatic-chat-identity.md) remains. Unknown or unidentified
callers may read; writes require the supported harness channel. Tools accept
no manual caller id, role, type, persona, or registration.

Existing authorship and annotation data remain readable, including historical
display profiles. The harness lifecycle limitations recorded in 0080 remain.
Removing agent runtime features does not authorize deleting thread stores or
rewriting their authors.

### Discussion is user-mediated

Multi-agent review discussion is a core use case, mediated by the user:

1. A reviewer and the user discuss findings in threads.
2. The user states the decision in their own words or hands the discussion to
   another agent with a task.
3. The assigned agent reads the relevant threads and history, then explains,
   defends, or implements the stated decision and replies where useful.

Fathomable stores no structured approval, agent-task ownership, assignment,
or agent-task completion state. Thread resolution remains human-controlled.
Reading does not authorize edits. An agent proposal remains a proposal, and
only the user closes a thread.

The server prompt is deliberately small:

> Fathomable holds review discussions attached to files in this repository.
> When asked, read the relevant threads and their history. Use thread_start
> for new findings or questions and thread_reply to continue existing
> discussions. Treat proposals as proposals; follow the user's stated
> decisions and your assigned task. Reading a thread does not authorize
> changes. Only the user closes threads.

Tool descriptions carry schema, filtering, paging, validation, and
re-anchoring details. The prompt imposes no automatic polling, all-thread
batch, or response duty.

### The viewer follows files, not agents

Live reload still preserves reading position. Changed-file hints, manual
changed-file jumps, the jumplist, and manual worktree navigation remain.
MCP `open`, agent-followed paths, auto-jump, its `AUTO` badge, `:auto`, and
`Space j a` are removed. `--name` still names a viewer window for the human;
it is not an agent routing target.

`Space a w` remains a visible human action but reports
`Wake agent is not yet implemented`. It has no picker, delivery check, spawn,
or success path.

### Migration removes configuration, not user state

Subscriptions, delivery ledgers, watches, reminders, nagging, and hook
integration are removed. `fathomable pending`, the old Fathomable hook
entries, the `agents` config node, and `jump.auto` / `jump.debounce` are no
longer supported. `jump.toast` remains the duration of change toasts.

The upgrade does not edit installed hook files or configuration and does not
wipe legacy state. Users remove old `fathomable pending` and `fathomable
hello` hook entries themselves, remove the obsolete config keys, and may
leave legacy `agents.jsonl` files in place unused. Thread data must not be
deleted.

## Consequences

- The MCP surface expresses durable review operations only; it no longer
  pretends to schedule or wake agents.
- Socket protocol v4 retains only `ThreadStart` and `ThreadReply` requests,
  with `Threads` and `Error` responses. `ThreadsList`, `Open`, and `Follow`
  are removed; MCP reads the store directly.
- A read is safe to repeat and safe for several agents because it has no
  delivery side effect.
- User prompts and thread prose remain the coordination mechanism. This is
  less structured and more truthful than inferred obligation state.
- A future wake integration may let the user send **all open threads** plus
  an optional instruction to a chosen chat. Its transport and chat-selection
  mechanism are undecided; hooks alone are not assumed sufficient.
- Historical decisions retain their narratives with explicit superseding
  notes so the removed design remains understandable.
