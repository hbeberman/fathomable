---
type: Decision
title: Re-anchoring without a snapshot
description: Each thread carries a window of the text it was last placed in, so an offline edit is followed even when the file has no last-seen snapshot.
resource: crates/fathomable-core/src/context.rs
tags:
  - decision
  - annotations
---

# 0038 Re-anchoring without a snapshot

Status: accepted (2026-08-28)

Amended 2026-09-20: truncated windows remain display evidence but cannot
authorize context-based placement. Exact full-anchor matching remains
available independently.

Amended 2026-09-16 by
[0087](0087-global-comparisons-and-board-history.md): the bounded context
window and mapping algorithm remain, but there is no snapshot-first path,
startup persistence, or historical backfill. Viewer and MCP project at read
time; live working-tree reload may persist a trustworthy local relocation.

Context backfill amended 2026-09-15 by
[0083](0083-single-user-alpha-clean-slate.md): snapshot-first and
context-window mapping remains current for records written by the new build,
but the startup scan that recorded windows on older threads is retired.
Line annotations and relocations carry context when written; file-wide
comments remain context-free. The `context` event and
`Store::record_context` described below are historical implementation
details, not a migration promise.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

[0020](0020-reanchoring-across-restarts.md) follows a thread edited
while Fathomable was closed through the file's last-seen snapshot
([0015](0015-follow-mode.md)). It said what it left out: the snapshot
store refuses a file over `seen::MAX_BYTES` (2 MiB), and a blob can be
deleted at any time (the guide says so), so a thread on such a file
still turns *detached* on the next start. The parked investigation
"re-anchoring without a snapshot" named the fix: a snapshot of the
annotated lines in the thread store. Picked as milestone 29 in a
question round on 2026-08-28, with these choices:

- *Where does the old text come from?* The thread store: at annotate
  time the annotated lines and a few lines either side are recorded
  with the thread. Lifting the size cap for pinned files was offered
  and not taken on its own, because it leaves the missing-blob case;
  a fuzzy search for the snippet was rejected because it has no
  locality rule. A window per thread covers both cases and each
  thread carries its own base.
- *How does a relocation found this way read?* Exactly as one found
  through the snapshot: persisted with `Store::relocate`, so the
  thread reads as *edited* and `updated` moves past an agent's
  `since`. A lower-confidence placement word was offered and not
  taken.
- *Threads from before this record?* Backfilled on start: a thread
  that still locates in the current text has its window recorded
  then, so an old thread is covered after one successful start.
- *Window size.* Not asked. Three lines either side: the context a
  unified diff shows, and wide enough that the one line of slack
  [0019](0019-reanchoring-edited-lines.md) allows an edit to reach
  is inside it with two lines to spare.

## Decision

### The context window

- `fathomable_core::context::Context`, which this record backs, holds
  the text of a thread's lines and up to `CONTEXT_LINES` (3) lines
  before and after them, as `before`, `lines`, and `after`.
  `Context::capture(text, range)` takes it from `text`;
  `Context::text()` is the window joined back into one text and
  `Context::range()` where the annotated lines sit inside it.
- `Store::annotate` and `Store::relocate` capture a window from the
  text they are given and store it on the `annotate` and `relocate`
  events as `context`. `Thread::context` returns it; the field is
  absent on records written before this change and is not sent over
  the session socket, where the snippet already is.
- A new `context` event carries a window for an existing thread
  without moving `updated` or marking it edited. `Store::record_context`
  appends one; it is the backfill.

### Mapping through the window

- `context::map_context(context, current, hint)` follows the window's
  annotated lines into `current`. The outer lines of each side of the
  window — those beyond the one line of slack 0019 lets an edit reach
  — are located by line hash in `current`, each as one block, the
  match nearest `hint` winning; a side that is empty because the
  window met the start or end of the file is located at that edge.
  The two blocks must both be found, in order. The text of `current`
  between them, blocks included, is the *region*; the window is
  diffed against the region with `reanchor::map_range`, and the
  result is shifted by the region's start. A side that cannot be
  found means the surroundings were rewritten, and the result is
  `Mapping::Removed`, as 0019's locality rule would give.
- Any truncated window returns `Mapping::Removed` before matching. The stored
  truncation flag does not distinguish dropped surroundings from shortened
  selected-line contents, so even a window with no omitted whole lines cannot
  establish complete evidence. This conservative rule covers existing records
  without a format change. Viewer and MCP still try exact full anchors first;
  when those fail, a truncated window leaves the thread detached rather than
  matching a retained prefix elsewhere. The stored excerpt remains available
  for display.

### On start

- `reanchor::follow_snapshots` runs the 0020 mapping first. A thread
  still detached after it — because the file has no snapshot, the
  snapshot cannot be read, or the thread does not locate in it — is
  followed through its context window when it has one. `Moved` and
  `Edited` are persisted with `Store::relocate` as before; `Removed`
  and a thread without a window stay detached.
- After the mapping, every thread that locates in its file and has no
  window has one recorded with `Store::record_context`. The viewer's
  start and the headless `--mcp` read ([0024](0024-workspace-sessions.md))
  both do this, so an agent's `annotations_list` sees the same ranges
  either way.

## Consequences

- A thread on a file over 2 MiB, or whose snapshot blob is gone,
  follows a local offline edit the way any other thread does. The
  snapshot path stays first because its diff sees the whole file.
- `threads.jsonl` grows by the window on every annotate and relocate
  event, and by one `context` event per pre-existing thread on the
  first start after upgrading. A binary that predates this record
  cannot read a store with a `context` event in it; `FORMAT_VERSION`
  is unchanged because the events it can read are unchanged.
- The window sees only what it holds: an edit whose hunk reaches past
  the window's outer lines detaches the thread, and a rewrite that
  moves the annotated lines far away with their context intact is
  still followed, as the nearest matching blocks are taken.
- The parked "re-anchoring without a snapshot" investigation is
  resolved.
