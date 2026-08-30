---
type: Decision
title: Live workspace
description: The tree follows the agent creating, deleting, and renaming files without R; a deleted open file keeps its last content under a banner; a rename carries the file's threads to the new path and is recorded in the store.
resource: crates/fathomable/src/app/watch.rs
tags:
  - decision
  - architecture
  - annotations
  - rendering
---

# 0028 Live workspace

Status: accepted (2026-08-28)

## Context

An agent reshapes a workspace as it works: it writes new modules, moves
files, and deletes what it replaced. The tree of
[0012](0012-workspace-mode.md) re-reads directories only on `R` or on
expanding one, so a file the agent just created is not there until the
reader remembers to ask. A file deleted under the open view leaves the
view showing content with nowhere to go and a one-line notice. A rename
is a delete plus an unrelated creation: the threads of
[0013](0013-annotation-storage-and-ux.md) stay on the old path and read
as detached, though every annotated line still exists.

The parked note said a recursive workspace watch was skipped for inotify
budget reasons. That is stale: [0015](0015-follow-mode.md) already
watches the root recursively for follow hints, and its create and remove
events reach the app and are dropped. What is missing is consumers.
Settled in a question round on 2026-08-28; the choices are below.

## Decision

### The tree refreshes itself

- A create, remove, or rename event under the root re-reads the
  affected directory after the hint debounce (`follow.hint-debounce`,
  300 ms by default), so a burst of writes costs one rebuild. Only
  directories the tree has already read are re-read, expanded or
  collapsed; one it has never read is read when it is expanded. An
  event on a path the tree has no listing for — the agent made a
  directory and wrote into it in the same burst — re-reads the nearest
  listing above it, which is what brings the new directory into view
  (amended 2026-08-29; only expanded directories were re-read, so a
  collapsed listing kept its stale entries when it was re-expanded and
  a new directory never appeared at all). The cursor and expanded set
  survive as they do for `R`.
- Events on paths the tree would not show — the ignore rules of the
  workspace plus `follow.ignore` — never trigger a rebuild, so build
  output churning under `target/` is free. With `I` showing ignored
  entries the filter is `All` and such events count.
- `R` stays as the manual re-read (it also drops the picker indexes);
  the picker indexes are dropped by an automatic refresh too, so a new
  file is one `Space f` away.
- A file the agent `follow`s that is not in the tree is revealed
  (parents expanded) without moving the tree cursor unless the sidebar
  has focus, in which case the cursor moves to it as `Enter` would.
  This matches [0023](0023-sidebar-paging.md): the highlight is the
  shown file.

### A deleted open file keeps its content

- When the open document's file is removed, the view keeps rendering
  its last content and a banner row at the top of the text reads
  `deleted` in the warning face; the pill and `:status` say so too.
  Scrolling, search, and reading threads keep working on the last
  content; `c`/`C` and replies are refused with a notice, since a new
  thread would anchor to a snapshot no file matches.
- If the file reappears (an editor's write-then-rename, or the agent
  restoring it), the next change event reloads it and the banner goes;
  threads re-anchor through the reload diff as in
  [0019](0019-reanchoring-edited-lines.md). Switching to another file
  and back shows the file-info pane with "deleted" if it is still gone.
- The tree row disappears with the rebuild; the sidebar highlight
  stays on the neighbouring row and the view keeps the deleted
  document until the reader moves.

### A rename carries its threads

- A rename is detected from the watcher's rename pair (`notify` reports
  `Rename(From)`/`Rename(To)` or a paired `Both` on Linux) when both
  paths are under the root. When the platform delivers an unpaired
  remove-then-create within the debounce window, the pair is matched by
  file size and content hash of the created file against the last-seen
  snapshot of the removed one; no match means delete plus create.
- On a detected rename every thread on the old path moves to the new
  path with its range and anchor unchanged. The store records it as a
  new `move` event (`{"event":"move","v":1,"thread":ID,"path":NEW,
  "created":T}`) so a store read after restart agrees with what the
  viewer showed. Readers that predate the event ignore unknown events
  as they do today.
- The open document follows its rename: the view switches path with
  scroll, cursor, and threads intact, the pill shows the new name, and
  a notice reads `renamed to NEW`.
- A directory rename moves every thread under it by prefix.

## Consequences

- The watcher and its event classification move from `app/mod.rs`
  into `app/watch.rs`, which this record backs; it turns raw `notify`
  events into `Change`, `Created`, `Removed`, and `Renamed { from, to }`
  after the debounce, with rename pairing done there. `app/mod.rs`
  keeps its `@okf-doc` on 0012.
- `fathomable-core::tree::Tree` gains an incremental
  `refresh_dir(workspace, dir)` so one event re-reads one directory;
  `annotations::Store` gains `move_path(id, new)` and the `Event::Move`
  variant, and the JSONL format gains one event without a version
  bump.
- `docs/guide.md` gains the deleted banner, the rename notice, and the
  note that `R` is no longer needed for new files. The parked "automatic
  tree refresh" TODO is removed.
