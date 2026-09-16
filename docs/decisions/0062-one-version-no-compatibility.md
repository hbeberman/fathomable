---
type: Decision
title: One version, no compatibility before the first tag
description: The socket keeps only the requests that have a client, refuses any protocol version but its own, and restarts at v1; the thread store and the agent register read the format version they have always written and refuse another with a message that says what to delete; the retired follow config block gets the ordinary unknown-setting error; and no on-disk or on-wire shape from before this record is owed a reader.
resource: crates/fathomable-core/src/session.rs
related_resources:
  - crates/fathomable-core/src/annotations.rs
  - crates/fathomable-core/src/config.rs
  - crates/fathomable/src/app/socket.rs
  - crates/fathomable/src/app/mod.rs
  - scripts/demo-repo.sh
tags:
  - decision
  - sessions
  - annotations
  - configuration
---

# 0062 One version, no compatibility before the first tag

Status: accepted (2026-09-05)

Current formats amended 2026-09-16 by
[0084](0084-explicit-mcp-contracts.md): durable keyed writes advance the
annotation event format to **3** and internal socket protocol to **6**.
Exact-only guards and the prohibition on automatic migrations or resets
remain. Older numbers below describe prior boundaries.

Current format boundary amended 2026-09-15 by
[0083](0083-single-user-alpha-clean-slate.md): the single-user alpha
advances the annotation store from format 1 to **2** and the internal socket
from protocol 4 to **5**, once each. The store and socket guards remain
exact-only: each build accepts only its current version. A store mismatch
reports the path and both versions with reset guidance; a socket mismatch
reports both versions and requires restarting the matching viewer and MCP
processes, not deleting annotation data. No older reader or migration is
added, and seen, checkpoint, marker, and runtime stores receive no format
machinery solely for this reset. The v1/v4 values below describe the
historical decision.

## Context

[0051](0051-retire-one-release-compatibility.md) removed the spellings
kept "for one release" by [0047](0047-one-vocabulary.md) and closed
with: no further compatibility window is owed by any earlier record. A
cruft audit on 2026-09-05 found three pieces of compatibility machinery
that predate that rule and that it did not name.

- **The socket carried its own history.** `session.rs` opened with a
  five-version changelog, kept `OLDEST_VERSION = 0`, and let a client
  speaking any version from 0 up send `ping` and `session_info`. No
  client in the tree sends either: the MCP server tests liveness by
  stat-ing `/proc/<pid>` through `Record::is_alive`, and the viewer and
  `--mcp` are one binary upgraded together (0051). `session_info`'s
  answer, the record plus the auto-jump flag, had one reader, the
  socket's own test.
- **The stores stamped a version nobody read.** Every thread-store
  event carries `"v":2` and every register event `"v":1`, and neither
  `Store::open` nor `Register::open` looked at the field: a file of any
  version loaded as the current one. A stamp with no reader implies a
  migration story that does not exist, and the comment on the
  register's constant promised that an older viewer would refuse a
  newer file.
- **The config still explained a block 0047 retired.** A `follow` node
  in `config.kdl` got a bespoke error naming where each of its six
  settings went, backed by a rename table. The block never shipped to
  anyone.

Fathomable is at 0.1.0 and has never been tagged. The user is its only
user, keeps no state worth migrating, and said so: nothing written
before this record, on disk or on the wire, is owed a reader.

## Decision

- **Socket.** `Request` has `open`, `threads_list`, `thread_reply`, and
  `thread_start`; `Response` has `Done`, `Threads`, and `Error`. `ping`,
  `session_info`, `Pong`, `Session`, and `FollowState` go, with the
  socket's short-circuit that answered them and the app's
  `follow_state`. Every request carries `"v"`, and a `v` other than
  `PROTOCOL_VERSION` is refused with the existing "unsupported protocol
  version" message; there is no floor and no per-op exemption.
  `PROTOCOL_VERSION` restarts at **1**: both ends are one binary, so the
  number bumps on any wire change and never needs to be read across a
  gap. Liveness is `Record::is_alive`, as it already was.
- **Stores read their stamp.** `Store::open` and `Register::open` read
  each line's `v` before its event and refuse a mismatch with a message
  that says what to do: `threads.jsonl line 3: format version 0, this
  build writes 1; delete <path> to start over`, and the same for
  `agents.jsonl`. The thread store's `FORMAT_VERSION` restarts at **1**:
  it is the first version anyone else will see. The register stays at
  1. Deleting `v` from the events was the alternative; a stamp that is
  read costs a few lines and is what a later format change keys on.
- **`follow` block.** A `follow` node in `config.kdl` is an unknown
  setting, the same error any misspelling gets. The rename table and
  its arm go.
- **Records describe the present shape.** Doc comments that explained a
  field as "absent on records written before ADR NNNN" say what the
  field means now: `author` is written only for an agent, the default
  being the user; `commit` is `None` for a workspace outside git;
  `context` is `None` when capture failed. `author` stays optional on
  the wire because the compact form is worth keeping and the default is
  one line.

## Consequences

- `Request::Ping`, `Request::SessionInfo`, `Response::Pong`,
  `Response::Session`, and `FollowState` leave `fathomable_core::session`;
  `OLDEST_VERSION` goes. `session.rs`'s module doc drops its changelog.
- A store or register from before this record fails to open with the
  line number, both versions, and the path to delete; the viewer and
  `--mcp` report it as they report any store error. There is no
  migration. (Noted 2026-09-06 by
  [0070](0070-one-workspace-many-worktrees.md): a state directory
  keyed by a root is renamed once to its common-dir key; a rename is
  not a format, and the rule stands.)
- `scripts/demo-repo.sh` writes `"v": 1` thread events.
- `config.rs` loses `FOLLOW_MOVED` and the `"follow"` arm; the
  `unknown setting` test already covers what a `follow` node now gets.
- [0014](0014-mcp-server-and-socket-v1.md) and
  [0024](0024-workspace-sessions.md) keep their history under the 0047
  rule; this record takes the backlink of `session.rs` from 0024, which
  keeps the file as a forward pointer.
- The rule of 0051 stands and now covers file formats as well as
  spellings: a shape change after the first tag needs its own record
  and its own reader.
