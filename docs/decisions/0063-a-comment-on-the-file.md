---
type: Decision
title: A comment on the file
description: A thread may be on a file as a whole rather than on lines of it; `Space c f` starts one, an agent's `thread_start` starts one by omitting `line`, the record carries no range, anchor, or snippet, the viewer shows it as a stub above the first line, and every list names it by its path alone.
resource: crates/fathomable/src/app/threads/file.rs
related_resources:
  - crates/fathomable-core/src/annotations.rs
  - crates/fathomable-core/src/layout/mod.rs
  - crates/fathomable/src/app/threads/stubs.rs
  - crates/fathomable/src/app/threads/words.rs
  - crates/fathomable/src/app/input/bindings.rs
  - crates/fathomable/src/mcp/start.rs
tags:
  - decision
  - annotations
  - input
  - sessions
---

# 0063 A comment on the file

Status: accepted (2026-09-05)

Context backfill amended 2026-09-15 by
[0083](0083-single-user-alpha-clean-slate.md): file-wide comments remain
context-free and `Store::relocate` still refuses them. The
`Store::record_context` reference below describes the retired historical
backfill API.

## Context

Every thread has been on lines: [0013](0013-annotation-storage-and-ux.md)
gave a thread a `LineRange`, a snippet of those lines, and an anchor
that follows them ([0019](0019-reanchoring-edited-lines.md),
[0038](0038-reanchoring-without-a-snapshot.md)), and
[0049](0049-inline-threads-and-the-rail.md) hung its stub under the
last of them. A remark about a file as a whole, that it should be
split, renamed, or is in the wrong crate, has had to borrow a line it
is not about, and then to follow that line when it moves and to detach
when it goes.

The user asked for a way to leave a comment on the file. `C`, which
[0049](0049-inline-threads-and-the-rail.md) made "always a new thread",
was the first candidate; he chose to keep the top-level keys as they
are and bury the new act under the `Space c` threads submenu instead.
He accepted that an agent may start one too, with the worry that an
agent might do so by accident where it meant a line; the tool's words
carry that warning, and the bridge is crossed when it is reached.

## Decision

- **A thread's lines are optional.** `Thread` and the `annotate` event
  carry `range` and `anchor` as `Option`, omitted from the record when
  absent, and a thread without them has an empty snippet and no
  context. `Thread::range()` returns `Option<LineRange>`, so every
  reader says what it does with a thread that has no lines. A file
  thread is started with `Draft::on_file(author, path, comment)`;
  `Draft::new` keeps its range. The store's `FORMAT_VERSION` stays 1:
  a record with lines reads exactly as before, and an optional field
  with a default is not a new shape.
- **It has one placement.** `Placement::File` joins `Anchored`,
  `Edited`, and `Detached`; `Thread::locate` returns it whatever the
  text says, and `Placement::range()` is `Option<LineRange>`. A file
  thread is never edited, never detached, and never re-anchored:
  `Store::relocate` and `record_context` refuse it with an error that
  says the thread is on the file, and the viewer's re-anchoring passes
  over it.
- **`Space c f` starts one.** The draft opens in a block of its own
  above the first line, headed `comment on <path>`, and submits as a
  file thread on the open file; from any pane it means the file the
  text shows. The `Space c` menu reads `c r o e d f`. `C` keeps its
  meaning.
- **The viewer shows it above line 1.** A `RowAnchor::Top` places its
  stub before the first row, and it folds, expands, replies, edits,
  resolves, and deletes like any thread. Its expanded header reads
  `● file · waiting` and so on: `file` is the placement word, in the
  place `detached` and `edited` take. The gutter carries no bracket
  for it, since it covers no line. The review list, the threads pane,
  and the review inbox name it by its path alone; it sorts before the
  file's line threads, so `]c` / `[c`, `l` / `h`, and the pane's
  `j` / `k` reach it first.
- **Agents may start and see one.** `thread_start` with `path` and
  `body` but no `line` starts a file thread, single or in `comments`;
  the tool's description says to omit `line` only for a comment on the
  file as a whole. `threads_list` and `thread_reply` show it with
  `placement: "file"` and no `range`, `thread_watch` and the hooks
  name it as `<path>`, and the reminder and the turn-start delivery
  print no snippet lines for it. `open` with no line shows it, as it
  shows any file.
- **Toasts and notices name the path.** Where a line thread's notice
  reads `comment on p:l from name (type)` or `reply on p:l`, a file
  thread's reads `comment on p from name (type)` and `reply on p`.

## Consequences

- `annotations.rs` gains `Placement::File`, `Draft::on_file`, and an
  `ErrorKind::OnFile`; `LineRange` is untouched. `Thread::range` and
  `Placement::range` change type, and every caller is revisited: a
  filter over lines skips a file thread, a sort key puts it first, a
  format prints the path alone.
- `fathomable_core::layout::RowAnchor` gains `Top`, and
  `with_rows_after` inserts a block on it before every other row.
- A new `app/threads/file.rs` holds `Space c f` and what the viewer
  needs to know about a file thread; `stubs.rs`, `words.rs`, and
  `draft.rs` use it.
- `mcp/start.rs` checks a file item as it checks a path: a path that
  is not a text file refuses the batch; no range check applies.
- `docs/guide.md` gains the chord, the placement word, and the tool's
  rule for `line`.
- A thread from before this record reads unchanged. A file thread
  written by this build is refused by no reader in the tree; an older
  build would fail on the missing field, and
  [0062](0062-one-version-no-compatibility.md) owes it nothing.
