---
type: Decision
title: Six tools
description: The agent-facing surface shrinks from ten tools to six, each pair with a natural undo becoming one tool with a flag; a subscription always covers the whole workspace and follow takes no paths; one threads tool lists open threads by default, flags and delivers the ones waiting on the caller, and widens to resolved threads on request; every thread an agent sees carries its placement in the working tree and no anchor hashes; thread_reply answers with the updated thread and refuses to reply into a detached thread without a line; and every failure names the call that fixes it.
resource: crates/fathomable/src/mcp/tools.rs
related_resources:
  - crates/fathomable/src/mcp/mod.rs
  - crates/fathomable-core/src/vocabulary.rs
  - crates/fathomable-core/src/agents.rs
  - crates/fathomable/src/hooks.rs
  - crates/fathomable/src/app/jump.rs
tags:
  - decision
  - sessions
  - annotations
---

# 0055 Six tools

Status: accepted (2026-09-04)

## Context

[0014](0014-mcp-server-and-socket-v1.md) gave the server four tools;
[0040](0040-agent-subscriptions-and-hooks.md) and
[0043](0043-agent-vocabulary.md) grew them to ten, each one job. On
2026-09-04 the user had a capable model try the server on a seeded
workspace and asked it for feedback. It subscribed, then called
`threads_list` to find the one open thread and got every resolved demo
thread with it, each with its full history and its anchor hashes; it
ran a filesystem glob the server had made unnecessary; and it asked for
a status filter, an idempotency key, the updated thread back from a
reply, and an anchor-health field. The user's reading: `threads_list`
against `threads_pending` is confusing even to a strong model, the
per-file subscription of 0040 is over-engineered, and the surface
should be fewer tools with more flags, error texts that say what to
call instead, and fewer tokens per answer.

A question round the same day settled the open points; each took the
recommended answer, and they are recorded below.

## Decision

### The six

| Tool | Parameters | Does |
|---|---|---|
| `workspaces` | `switch` | lists the known workspaces and their viewers, the default marked; with `switch` (a root, or a viewer name or id) pins that one first |
| `follow` | `id`, `type`, `persona`, `end` | subscribes the session to the workspace; `end: true` ends the subscription, its deliveries, and its watches |
| `threads` | `status`, `path`, `since`, `limit`, `id` | lists threads; see below |
| `thread_reply` | `thread`, `body`, `resolve`, `line`, `end_line`, `replies`, `persona`, `id` | answers one thread or several and returns each as it now stands |
| `thread_watch` | `on`, `when`, `remind`, `cancel`, `id` | asks to be woken when `on` moves; `cancel: true` removes the watch |
| `open` | `path`, `line`, `end_line`, `viewer` | shows a file in the viewer |

Every tool but `workspaces` still takes an optional `workspace`. The
listing tool is `workspaces`, plural, and not `workspace`, so that the
parameter of that name stays the only thing called `workspace`.
`workspace_list`, `workspace_switch`, `unfollow`, `threads_list`,
`threads_pending`, and `thread_unwatch` go; `unfollow` and
`thread_unwatch` were each an undo of one call, and the two listings
were one question asked two ways.

### A subscription covers the workspace

- `follow` takes no `paths`. A subscriber's reach is every thread in
  the workspace's git reach ([0024](0024-workspace-sessions.md),
  [0035](0035-threads-follow-head.md)); the follow list, the
  component-wise directory match, and the "threads it posted in" clause
  of 0040 go, since the whole workspace already holds them.
  `Subscriber` loses `paths`; a register line written before this
  record still loads, the field ignored.
- The viewer is no longer told what an agent follows: `Request::Follow`
  leaves the socket protocol, and with it the viewer's followed list,
  the `N followed` count in the status line, the `followed` row of
  `:status`, and auto-jump's preference for the agent's files. 0031's
  "Bursts" is amended: after a quiet period auto-jump opens the newest
  entry. `:status` still lists the subscribers from the register.
- `follow` therefore works the same with and without a viewer, and its
  reply says only whom it subscribed and as what. A `follow` with
  neither `id` nor `type` on a connection that is already subscribed
  is a no-op that says so; on one that is not, it says what to pass.

### One `threads` tool

- `threads` returns the threads in reach, oldest change first, at most
  `limit` (50) of them, with the `since` paging of 0040. `status` is
  `open` (the default), `resolved`, or `all`. `path` narrows to a file
  or a directory, as `threads_list` did, and fails, naming same-named
  paths elsewhere, when it is neither.
- When the caller is a subscriber (its `follow` on this connection, a
  session bond, or `id`), each open thread whose newest message is not
  the caller's carries `pending: true` in the JSON and `pending` in its
  summary line, and returning it **records it as delivered**: the
  agent has now seen it, so the hooks do not hand it over again. A
  caller with no subscription reads without side effect. A watch that
  has fired is reported first, as the hook's blob reports it, and the
  threads it reminds of come in full whatever the filters say.
- The hook's overflow line reads `N more; call threads` and the
  overflow stays undelivered, as 0040's correction requires; that call
  returns it. `threads_pending`'s `limit` of 20 goes with it.

### What a thread looks like to an agent

- The JSON a tool hands back is not the `Thread` of the socket. An
  **open** thread is `id`, `path`, `range`, `placement`, `status`,
  `created`, `updated`, `comment`, `snippet`, `replies`, and, when
  set, `commit`, `edited`, and `pending`. A **resolved** thread is
  `id`, `path`, `range`, `placement`, `status`, `updated`, `messages`
  (a count), and `comment` cut to its first line. Neither carries the
  anchor's hashes. The socket between viewer and server keeps the full
  thread.
- `placement` is `anchored`, `edited`, or `detached`, the
  [`Placement`](0032-placement-and-state.md) the viewer draws from,
  and `range` is the range that placement names: where the lines are
  now, or the last known range of a detached thread. The server
  computes it itself against the working tree when it answers, through
  `Thread::locate`, whether the threads came from a viewer or from the
  store, so the socket does not change and the two paths cannot
  disagree.
- The summary line stays one per thread: `id  path:range  status`,
  then `pending`, `edited`, or `detached` when they apply, then the
  first line of the comment.

### `thread_reply` answers with the thread

- The result carries the updated threads in the JSON, in the shape
  above, and one summary line each: `replied to <id> at path:range
  (anchored|edited|detached)`, with `, proposing to resolve it` when
  `resolve` was set.
- `Request::ThreadReply` is answered with `Response::Threads` holding
  the one thread as it stands after the reply, in place of
  `Response::Done`; the headless path reads the thread back from the
  store the same way.
- A batch is **checked before anything is written**: an unknown id, a
  resolved thread, or a detached thread given no `line` fails the whole
  call, naming every offending item, so a retry with the fixed list is
  a whole retry. A reply to a detached thread with `line` and
  `end_line` places it there first, as 0033 does for any rewrite. The
  proposed-resolution rule of [0053](0053-resolution-is-the-users.md)
  is unchanged.

### Failures say what to do

Every failure names the call that fixes it, in the vocabulary:

| Failure | Says |
|---|---|
| no workspace contains the cwd | `call workspaces, then workspaces with switch` |
| unknown thread id | `no thread <id>; call threads to see the ids` |
| reply to a resolved thread | `<id> is resolved; the user reopens it; call threads with status: all to read it` |
| reply to a detached thread without a line | `<id> is detached: its lines are gone from <path>, last seen at <range>; pass line and end_line to place it` |
| `id` without `type`, or an unknown type | names the configured types, as today |
| `type` with no way to learn the session | asks for the `id` from the hello hook, as today |
| `follow` with `end` and no session | `nothing to end: pass id, or follow with type first` |
| `thread_watch` with no subscription | `call follow with type first` |
| `path` that is neither file nor directory | names the same-named paths, as today |

And one nudge in a success: a `thread_reply` signed by no subscription
appends `signed as <name> with no subscription; call follow with type
to be told about answers`.

### Tokens

- Descriptions are two or three sentences: what the tool does, what it
  returns, and the one thing a model gets wrong. The server
  instructions keep their first 512 characters self-contained (0040).
- The `hello` body shows `follow { type }` as the call to make, and
  `id` only when the harness cannot be told.

## Consequences

- `mcp.rs` splits: `mcp/mod.rs` keeps the transport of 0014 (the
  server, the bind, the socket exchange, `list_tools`), and
  `mcp/tools.rs`, which this record backs, holds the six tools, their
  parameters, the shape a thread takes for an agent, and the failure
  texts. The tests follow the code they test.
- `vocabulary::ALL` names six tools; the `SWITCH`, `STATUS`, `END`,
  `CANCEL`, and `PENDING` parameters join it and `PATHS` leaves. The
  schema test of 0043 holds the new table against the live schema and
  the guide's table.
- `Subscriber::paths` and `Subscriber::covers` go; `Register::subscribe`
  loses its `paths` argument; `Thread::pending_for` needs no scope.
- `Request::Follow` leaves the wire and `PROTOCOL_VERSION` is bumped;
  [0051](0051-retire-one-release-compatibility.md) means no shim.
  `App::followed`, `reveal_followed`, the status pill's count, and the
  `:status` row go; `app/jump.rs`'s target choice is the newest entry.
- `hooks.rs`'s blob names `threads` in its overflow line and `hello`
  drops `paths` from the call shape; `scripts/demo-repo.sh` seeds a
  subscriber without paths.
- Amended where they describe the ten tools or the follow list: 0014
  (the tool table), 0031 ("Bursts"), 0040 (subscriptions, scope, the
  tool table), 0042 (`threads_pending` in the blob), 0043 (ten tools).
- `docs/guide.md`'s tool table, its coverage paragraph, and its hooks
  section are rewritten in the same change.
