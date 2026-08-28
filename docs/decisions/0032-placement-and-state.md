---
type: Decision
title: Placement and state
description: A thread's placement (detached, edited) and its state (waiting, open, resolved) are shown as two words, an agent reply toasts in the viewer it came through, and the store appends each event in one write so concurrent writers cannot corrupt it.
resource: crates/fathomable/src/app/mark_words.rs
tags:
  - decision
  - annotations
  - rendering
---

# 0032 Placement and state

Status: accepted (2026-08-28)

## Context

A review on 2026-08-28 went wrong in a way the viewer could not
explain. The reader left ten comments; the agent rewrote the block
around each one, replied with `resolve: true`, and reported success.
The reader saw ten red *detached* marks and nothing else: no toast, no
sign of a reply, no sign of a resolution. The agent then diagnosed the
edits as the cause of the resolutions, which they were not. On the next
start the store failed to load: `threads.jsonl line 37: trailing
characters`.

Three separate faults, one per section below:

- [0019](0019-reanchoring-edited-lines.md) ranks `MarkKind` so that
  placement wins the gutter colour, and the thread pane and the
  file-threads pane took their only status word from the same rank. A
  detached thread therefore read `detached` whether it was open,
  waiting, or resolved; the glyph in the file-threads pane stayed `●`
  on a resolved detached thread.
- [0030](0030-waiting-threads.md) raises its `reply on …` toast from
  the store reload, which fires when *another* writer appends. The
  viewer that receives `thread_reply` over its own socket writes the
  reply itself, reloads nothing, and toasts nothing. A resolving reply
  never counts as waiting, so it toasts nowhere.
- `Store::commit` appended with `writeln!`, which issues one `write`
  for the line and another for the newline. `O_APPEND` makes each call
  atomic, not the pair: two writers (two viewers of one workspace, or a
  viewer and a headless `--mcp` reply) can interleave `{a}{b}\n\n`,
  and the strict loader of [0013](0013-annotation-storage-and-ux.md)
  then refuses the whole file.

Widening 0019's one-line window was considered and left alone: the
agent's edits were rewrites of the surroundings, which 0019 chose to
call removed rather than pin to an arbitrary line. Fanning
`thread_reply` out to every viewer, as `open` and `follow` do, was
rejected: each viewer holds its own `Store` and would append the reply
again.

## Decision

### Two words

- `app/mark_words.rs`, which this record backs, derives `Words` for a
  thread: an optional *placement* word (`detached`, `edited`) from the
  gutter kind, and a *state* word (`waiting`, `open`, `resolved`,
  `auto-resolved`) from the thread alone. The gutter colour is
  unchanged; it still ranks placement first.
- The thread pane's header shows both, placement first:
  `L12-14  detached · auto-resolved`, each in its own colour. A thread
  at its lines shows the state alone, as before.
- The file-threads pane draws `✓` from the state, so a resolved thread
  reads resolved wherever its lines went; its colour stays the gutter's.
- The thread list is unchanged: it never showed placement.

### The toast

- `agent_reply` toasts `reply on <path>:<line>` itself, and
  `reply on <path>:<line>, resolved` when the reply resolves, so the
  viewer the reply came through says so at once. The store-reload toast
  of 0030 stays for the other viewers.

### One write per event

- `Store::commit` appends the line and its newline in a single
  `write_all`, so an `O_APPEND` append lands whole and two writers
  interleave lines, never bytes. A test appends from two handles to one
  file and reads it back.
- The loader stays strict. A file already damaged is repaired by hand:
  the offending line holds two events run together, and splitting it
  before the second `{` restores it.

### The welcome pane

- `]g`/`[g` on the welcome pane said `no diff base: not in a git
  repository`; it now says `no file open`.

## Consequences

- `FileRow::words()` joins `kind()`; `ui.rs` maps kinds to words through
  `mark_words::label`.
- `docs/guide.md`'s annotation section names the two words and the
  resolving toast.
- A store written by two writers before this change may hold a joined
  line; the error names it.
