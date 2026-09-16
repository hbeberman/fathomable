---
type: Decision
title: The user has the last word
description: A thread is pending for an agent only while the user's act — a comment, a reply, an edit, or a reopen — is the newest thing on it, whichever session answered before; an agent's answer leaves it waiting on the user. The threads tool says who answered, filters on pending, and marks edited messages. Fathomable names an agent once, at follow, from the harness it speaks through, and names the user from config, User by default.
resource: crates/fathomable-core/src/identity.rs
related_resources:
  - crates/fathomable-core/src/annotations.rs
  - crates/fathomable-core/src/config.rs
  - crates/fathomable-core/src/vocabulary.rs
  - crates/fathomable/src/app/draw/mod.rs
tags:
  - decision
  - annotations
  - sessions
  - configuration
---

# 0058 The user has the last word

Status: accepted (2026-09-05)

Agent obligation and delivery state superseded 2026-09-15 by
[0082](0082-three-tool-review-core.md). `threads` defaults to every open
discussion regardless of last author and reading changes nothing; there is
no pending filter. Last-act reasoning remains useful for the viewer's
human-facing waiting state and resolution proposals. Multi-agent discussion
is now explicitly user-mediated: the user's words and assigned task, not an
inferred queue, determine what another agent should do.

Identity amended 2026-09-15 by
[0080](0080-automatic-chat-identity.md): every write carries an automatic
harness-qualified caller id, before `follow` as well as after it.
The optional type/persona profile is fixed by `follow` and read from
the addressed workspace's live subscription, not connection memory.
The `hello` text is removed; names still appear in tools and delivery
blobs. The last-act and pending rules below are unchanged.

## Context

[0040](0040-agent-subscriptions-and-hooks.md) made a thread *pending*
for a subscriber when its newest message was not that subscriber's,
keyed on the harness session id. The rule was written for several
agents conversing through one thread. On 2026-09-05 the user watched a
Copilot session answer "is this really necessary?" on a demo file for
the third time in four hours: three sessions, three replies, three
proposals to resolve, and the user had not touched the thread once. Each
session was new, so each saw an answer it had not written and took it as
a question. The agent said so itself in its feedback: it could not tell
"needs a reply" from "awaiting the user".

The user called the multi-agent idea underbaked and asked that only a
human's word wake an agent, and for a way to read every open thread
even when an agent spoke last. Two more things came up in the same
review:

- The name an agent signs with was whatever it chose. `persona` was
  accepted on every `thread_reply`, so one session could sign each
  message differently, and without it the name was the MCP client
  string the harness reports, which changes between releases. The
  stores on the user's machine held `claude-code`, `Claude`, `Copilot`,
  `Copilot CLI`, and `codex-mcp-client` for three harnesses.
- The user had no name at all: every surface said `user`.

The user also asked that an edit of any message, even one an agent has
answered, count as the user speaking again, with a marker so the agent
knows why it is seeing the thread. And that a reopen count the same
way: the user reopening an answered thread is the user saying "not
done".

## Decision

### The last act

- A thread's **last act** is the newest of: the comment, written or
  edited; each reply, written or edited; and the most recent reopen.
  The act's author is the user for a comment the user wrote (an agent
  may start a thread since [0061](0061-agents-start-threads.md),
  2026-09-05), an edit, and a reopen,
  and the reply's author for a reply. A later act in the thread wins a
  tie. `Thread::last_act()` returns the author and the time.
- A thread is **pending** when it is open and its last act is the
  user's. A thread is **waiting** ([0030](0030-waiting-threads.md))
  when it is open and its last act is an agent's. Over open threads
  the two are complements, and neither depends on who is asking. The
  session id leaves the predicate: `Thread::awaits(Party)` and the
  `Party` enum go, replaced by `Thread::awaits_user()` and
  `Thread::awaits_agent()`.
- A **delivery** is keyed on the last act's time, not the newest
  message's. An edit or a reopen therefore re-delivers the thread to
  every subscribed session, once each, as a new reply would.
- To carry the acts, a reply records when it was last `edited`, and a
  thread records when its comment was last edited and when it was last
  `reopened`. Old records load with none of the three; the fields are
  written only when set.
- Watches are unchanged: `when: "message"` still fires on a message
  from anyone but the watcher. A watch is an explicit one-shot request
  on one thread, and it is where one agent may still hand a thread to
  another.

### What the agent reads

- The hook blob marks a message edited since it was written with
  `[edited]` after the author, and when the last act is an edit of a
  message older than the two it shows, it shows that message too.
- The `threads` tool's summary line carries one of three words after
  the status: `pending` when the user has the last word; `answered by
  Claude (coder)` when an agent has it; `proposed by Claude (coder)`
  when that agent's reply proposes resolution. The JSON keeps `pending:
  true` and gains `answered: {name, client, type, id, proposed}` on a
  thread an agent answered last; each reply carries `edited` when set.
- `status` accepts `pending`: the open threads where the user has the
  last word. `open` (the default) still lists every open thread, so an
  agent that wants the board rather than the queue asks for nothing
  new. `threads_pending`'s successor is one word on one tool, as
  [0055](0055-six-tools.md) wanted.
- `pending` is marked for every caller, subscribed or not, since it no
  longer depends on the caller; only a subscriber's read records the
  delivery.

### Names

- Fathomable names an agent once, at `follow`. The name is `persona`
  when given, else the harness's name from a fixed table keyed on the
  MCP client string — `claude-code` is Claude, `copilot-cli` and
  `github-copilot-developer` are Copilot, `codex-mcp-client` is Codex —
  else the client string itself, else `agent`. The table lives in
  `fathomable-core::identity`, which this record backs.
- `thread_reply` loses `persona`. A reply signs with its subscription's
  name and type; a reply with no subscription signs with the harness
  name from the same table. A session cannot change its name between
  messages.
- The label is `name (type)` everywhere: the viewer's message rows and
  stubs, the thread pane header, `:status`, the hook blob, and the
  `threads` lines, including the `answered by` word.
- The user is named by `user { name "User" }` in `config.kdl`, `User`
  by default. The wire keeps `user` as the author, so old stores and
  the socket are untouched; every rendering reads the configured name.
  The hello text tells the model the name as well, so an agent can
  address the person it is answering.

### Amendments

- 0040's *Pending* section is superseded by *The last act* above; its
  scope rules were already retired by 0055.
- 0030's *Waiting* rule reads "last act" for "newest message": an edit
  or a reopen by the user now ends the wait, since it is the agent's
  turn.
- 0053's "a proposed thread is a waiting thread" holds, and a proposal
  is no longer pending for the next session that comes along.
- 0055's tool table loses `persona` from `thread_reply` and gains
  `pending` as a `status` value.
- The charter's vocabulary entry for *waiting* and *pending* reads
  "whose last act is the other side's", and its deferred list gains
  "agents conversing with each other through threads".

## Consequences

- `fathomable-core::identity` holds the harness table, the agent-name
  rule, and the default user name. `annotations.rs` holds `last_act`
  and the edit and reopen times; `agents.rs` keys deliveries on the act
  and marks edits in the blob; `config.rs` parses `user { name }`.
- `mcp/tools.rs` drops `persona` from `ReplyParams`, adds the `pending`
  status, the three words, and the `answered` object; the vocabulary
  gains `STATUS_PENDING` and `ANSWERED`.
- `hooks.rs` passes the user's name into the blob and the hello text.
- The viewer's four `"user"` literals read the configured name, and
  `--config-show` prints the `user` block.
- The guide's §7 gains the `user` node; §8's tool table, hook table,
  and prose say "the user's last act". [0040](0040-agent-subscriptions-and-hooks.md),
  [0030](0030-waiting-threads.md), [0053](0053-resolution-is-the-users.md),
  and [0055](0055-six-tools.md) carry dated notes.
- A thread an earlier session answered no longer reaches a new session
  until the user speaks again. A user who wants an agent to look at an
  answered thread replies, edits, or reopens it, or wakes the agent
  with `Space a w` and lets it call `threads`.
