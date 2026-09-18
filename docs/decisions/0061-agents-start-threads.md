---
type: Decision
title: Agents start threads
description: A seventh tool, thread_start, lets an agent open a thread on a line range of a file, one or several in a call, signed as thread_reply signs and stamped with HEAD as the user's comments are; a thread records who wrote its comment, the user unless said otherwise, so an agent's thread is waiting on the user from birth and reaches no agent until the user speaks; the viewer names the author on the comment's rows and lets nobody edit an agent's comment.
resource: crates/fathomable/src/mcp/start.rs
related_resources:
  - crates/fathomable-core/src/annotations.rs
  - crates/fathomable-core/src/session.rs
  - crates/fathomable-core/src/vocabulary.rs
  - crates/fathomable/src/app/threads/mod.rs
  - crates/fathomable/src/app/draw/mod.rs
tags:
  - decision
  - sessions
  - annotations
---

# 0061 Agents start threads

Status: accepted (2026-09-05)

MCP contract amended 2026-09-16 by
[0084](0084-explicit-mcp-contracts.md): starts expose a precise input
schema, complete typed JSON results, and optional per-comment
`idempotency_key` retry protection. Without a key, two comments on the
same lines still create two discussions.

Tool shape amended 2026-09-15 by
[0082](0082-three-tool-review-core.md): `thread_start` requires one non-empty
`comments` array of `{path, line?, end_line?, body}`. The former top-level
single-comment arguments are removed. Batch prevalidation, file-wide
comments, automatic authorship, and thread behavior remain.

Identity amended 2026-09-15 by
[0080](0080-automatic-chat-identity.md): `thread_start` no longer accepts
a caller `id`. Every comment requires and records the automatic chat
identity, even without `follow`; a live subscription in the addressed
workspace supplies its optional display profile. No connection cache
or `hello` text remains. Batch validation and thread behavior below are
unchanged.

## Context

Every thread so far began at the keyboard: the user selects lines,
writes the comment ([0054](0054-the-draft-is-written-in-the-thread.md)),
and an agent answers it ([0055](0055-six-tools.md)). The store's
`Annotate` record has no author, and the last-act rule of
[0058](0058-the-user-has-the-last-word.md) reads the comment as the
user's without asking. On 2026-09-05 the user asked for the other
direction: an agent posts a thread to a line, or a range of lines, so
an agent can be told to tag the parts of a commit that deserve a
comment thread and the user reads them in the viewer as review.

The request also asked for "a range of characters on a line, like we
can". The viewer's mouse drag does select columns, but a submitted
comment keeps only the lines: a thread anchors to a line range and
nothing narrower ([0005](0005-annotations.md),
[0032](0032-placement-and-state.md)). Column anchoring would change the
anchor hashes, the placement, the gutter, and the stub rows, and is not
this record. This record is lines only.

A question round the same day settled the open points; each took the
recommended answer, and they are recorded below.

## Decision

### The tool

- **`thread_start`** joins the six of 0055 as the seventh tool. It
  takes `path`, `line`, `end_line`, and `body` for one comment, or a
  `comments` list of `{ path, line, end_line, body }` for several, the
  way `thread_reply` takes `thread` and `body` or `replies`. `end_line`
  defaults to `line`. It also takes `id` and `workspace`, as every
  writing tool does. It has no `resolve`: a comment cannot propose its
  own resolution.
- It is a seventh tool and not a mode of `thread_reply`, because
  starting a thread is not the undo of answering one, and a tool that
  does something else when `thread` is absent is the kind of thing a
  model gets wrong.
- The batch is **checked before anything is written**, as a reply
  batch is. A path that is not a file in the workspace, a range past
  the end of the file, a file that is not text, or an empty body fails
  the whole call, naming every offending item, and nothing is written.
  Nothing else is checked: two threads on the same lines are two
  threads.
- The result is `thread_reply`'s: the new threads in the agent shape of
  0055, and one line each, `started <id> at path:range (anchored)`,
  followed by the unsubscribed nudge when there is no subscription.
- It works without a viewer, through the store, as `thread_reply` does.

### Checkout-confined reads

Agent-start validation and origin capture read through one capability-relative
reader rooted at the bound checkout. The same reader supplies MCP placement,
reply validation, and headless or viewer-backed agent relocation. A
repository-relative spelling is not sufficient: symlink targets and parent
directories must also resolve within that checkout, even if they change
between validation and the actual read.

Relative symlinks that stay inside the checkout are supported. Absolute
symlink targets, escaping relative targets, and escaping directory symlinks
are refused with an explicit read error; no outside contents enter a new
snippet, content identity, or relocation. The reader rejects absolute input
paths and parent traversal independently of MCP validation, so direct viewer
socket requests have the same protection. Normal missing-file and binary
placement behavior is unchanged.

This boundary protects agent-driven reads; it does not sandbox the human
viewer, Git access, or the host agent. Existing stored excerpts are not
rewritten by the fix.

### Who wrote the comment

- A thread records the **author of its comment**. `Draft::new` takes
  the author first, as `Reply::new` does, and the viewer's own draft
  passes the user explicitly. The `Annotate` record and the thread on
  the socket carry `author` only when it is not the user, so every
  record written before this one loads as the user's and a user's
  comment is written as before.
- `thread_start` signs the comment as `thread_reply` signs a reply: the
  subscription's name and type when the connection subscribed, the
  session is known from the harness, or `id` is passed, and the
  harness's name otherwise, saying so.
- The comment is stamped with the workspace's `HEAD`, as the viewer
  stamps the user's ([0024](0024-workspace-sessions.md)), in the viewer
  and headless paths alike, so a thread on a commit's lines shows only
  where that commit is reachable.

### What the thread is from birth

- The last act of a fresh agent thread is the agent's comment, so it
  is **waiting** on the user under 0030 and 0058 as amended: the
  waiting colour, the `↩` tag on its file, a place in the review list,
  and `]r`. It is **pending** for no agent, the poster included, and
  no hook delivers it, until the user replies, edits, or reopens it;
  then it reaches every subscriber, as any thread does.
- The viewer toasts `comment on src/lib.rs:42 from Claude (coder)` and
  refreshes its marks. It does not move the cursor, open the file, or
  mark the lines seen ([0020](0020-reanchoring-across-restarts.md) marks what the user
  read, and they have not). An agent that wants the user looking calls
  `open`.
- The comment's rows in the text, the thread header, the review list,
  and the hook blob name the comment's author as they name a reply's,
  `name (type)` for an agent and the configured name for the user
  (0058). The user cannot edit an agent's comment, as they cannot edit
  an agent's reply; `e` on it does nothing, as it does there.

### Failures say what to do

| Failure | Says |
|---|---|
| neither `path` and `body` nor `comments` | `pass path, line, and body, or a non-empty comments list` |
| `path` with no `line` | `<path>: pass line` |
| a path that is a directory | `<path> is a directory; pass a file` |
| a path that is not in the workspace | names the same-named paths, as `threads` does |
| a range past the end | `lines 90-95 are past the end of src/lib.rs (40 lines)` |
| a file that is not text | `<path> is not a text file` |
| an empty body | `<path>:<line>: body is empty` |

## Consequences

- `mcp/start.rs`, which this record backs, holds the tool's parameters,
  the batch check, the headless path, and the result lines; `tools.rs`
  registers it and lends its `Shown`, `Tree`, and `Target`.
- `fathomable-core::annotations` gives `Draft` and `Thread` an author,
  `Thread::author()`, and `last_act` reads it; `Author` gains a
  `Default` of the user for the record's absent field.
- `session.rs` gains `Request::ThreadStart { path, range, author, body }`,
  answered with `Response::Threads` holding the one thread, and
  `PROTOCOL_VERSION` is bumped; [0051](0051-retire-one-release-compatibility.md)
  means no shim.
- `vocabulary::ALL` names seven tools and gains `COMMENTS`; the schema
  test of 0043 holds the table against the live schema and the guide's
  table.
- `app/threads/mod.rs` gains `agent_start` beside `agent_reply`;
  `message_for` reads the comment's author; the three drawing sites
  that assumed the user read `Thread::author()`; `agents.rs`'s blob
  names the comment's author through `who`.
- The hello text and the server instructions show the call shape; the
  guide's tool table gains the row, and its threads section says an
  agent may start one.
- 0055's table carries a dated note naming the seventh tool, and 0058's
  "the act's author is the user for the comment" reads "for a comment
  the user wrote".
