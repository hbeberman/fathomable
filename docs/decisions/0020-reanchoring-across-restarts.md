---
type: Decision
title: Re-anchoring across restarts
description: How a thread edited while Fathomable was closed finds its lines again, using the last-seen snapshot as the text it was last placed in.
resource: crates/fathomable/src/app/reanchor.rs
tags:
  - decision
  - annotations
---

# 0020 Re-anchoring across restarts

Status: accepted (2026-08-27)

## Context

[0019](0019-reanchoring-edited-lines.md) follows a thread onto rewritten
lines through the diff between the text the running viewer held and the
reload. It left the offline case open: an agent that edits annotated lines
while Fathomable is not running still turns the thread *detached* on the
next start, because the store keeps hashes and the viewer has no old text
to diff from. Picked as milestone 11 in a question round on 2026-08-27,
with these choices:

- *Where does the old text come from?* The last-seen snapshot of
  [0015](0015-follow-mode.md). It already holds each file as the reader
  last saw it, in the same workspace state directory, written on
  switch-away, quit, and idle. A second snapshot mechanism inside the
  thread store was rejected as duplication; a hybrid was rejected as too
  large for one session.
- *When?* On start, for every file that has threads, so
  `annotations_list` ([0014](0014-mcp-server-and-socket-v1.md)) reports the
  new ranges before any file is opened. Deferring to first open was
  rejected because the MCP view would lie until then.
- *Pruning?* Snapshots of files with open threads are exempt from the
  30-day expiry of 0015, so a thread on a file untouched for a month keeps
  its base.
- *Snapshot on annotate?* Yes. Commenting on lines means the reader saw
  them, and it guarantees a base at least as new as the anchor even if the
  process dies before the idle mark. A relocation on reload does *not*
  snapshot: doing so would erase the last-seen diff of the very edit the
  reader most wants to review; the idle mark covers it seconds later.

## Decision

### Startup mapping

- `App::reanchor_from_snapshots` runs once after the snapshot store is
  installed and before the first file opens. For each path with threads
  it reads the current file from the workspace and the path's snapshot;
  a thread that no longer locates in the current text but does locate in
  the snapshot is followed with `reanchor::map_range(snapshot, current)`.
  `Moved` and `Edited` results are persisted with `Store::relocate`
  exactly as 0019 does on reload, so the thread reads as *edited* and
  `updated` moves past any agent's `since`. `Removed`, a missing file, a
  missing snapshot, or a thread that does not locate in the snapshot
  either leave the thread detached as before.
- The mapping is the 0019 one, so the same locality rule applies: a
  rewrite reaching more than a line beyond the range detaches.

### Snapshot store

- `seen::Store::open_pinned(dir, paths)` opens the store keeping the
  records of `paths` past `MAX_AGE`; `open` is `open_pinned` with no
  paths. The viewer and `--doctor` pin the paths of threads whose status
  is open, so resolved threads let their snapshots age out.
- The thread store is therefore opened before the snapshot store.

### Snapshot on annotate

- A successful `Store::annotate` marks the document seen, which records
  the snapshot and resets the last-seen diff base for that file.

## Consequences

- The 0019 limitation and the parked "re-anchoring across restarts"
  investigation are resolved for files under 2 MiB that the reader has
  looked at; a file annotated and never idled, switched from, or quit
  from cannot happen any more because annotating snapshots it.
- Startup reads each annotated file and its snapshot once; workspaces
  without threads pay nothing.
- Pinned snapshots make the `seen/` directory grow with the set of open
  threads rather than only with recent reading; resolving threads
  releases them on the next start.
