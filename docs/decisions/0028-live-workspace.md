---
type: Decision
title: Live workspace
description: The tree follows workspace changes without R; deleted tracked files retain a searchable Git snapshot under a banner; renames carry threads to the new path.
resource: crates/fathomable/src/app/watch.rs
tags:
  - decision
  - architecture
  - annotations
  - rendering
---

# 0028 Live workspace

Status: accepted (2026-08-28); amended 2026-09-13 (resource-bounded
watches); amended 2026-09-14 (searchable Git tombstones)

Thread observation amended 2026-09-18 by
[0089](0089-store-only-mcp.md): thread-store notifications are coalesced
independently of the workspace/Git quiet period and may trigger immediate
reconciliation and redraw. Known state-watch failures reattach and retry
while degraded; failed refreshes preserve the last-good board. Socket
requests no longer exist. Ordinary workspace/Git event batching remains.

Thread rename placement amended 2026-09-16 by
[0087](0087-global-comparisons-and-board-history.md): an exact live rename
still carries the active viewer and its thread marks, but the path projection
is checkout-local and no longer overwrites one global board path shared by
other worktrees.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

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
- Paths the tree would not normally show — the ignore rules of the
  workspace plus `watch.ignore` — are not watched, so build output
  churning under `target/` is free at the kernel, event-loop, and render
  layers. `Space F g` can browse ignored entries from a directory
  snapshot but does not recursively live-monitor ignored trees. An
  ignored file that has been opened is watched narrowly and still
  reloads, including while another file is in front.
- `R` stays as the manual re-read (it also drops the picker indexes);
  a new file is one `Space f` away because an automatic refresh patches
  the picker indexes (2026-09-06, `app/file_index.rs`): a path that
  appears joins them where the walk would have put it, a directory that
  arrives whole is walked, a path that goes leaves them, and only a
  rules change or a lost-events rescan walks the tree again.
- A file the agent `follow`s that is not in the tree is revealed
  (parents expanded) without moving the tree cursor unless the sidebar
  has focus, in which case the cursor moves to it as `Enter` would.
  This matches [0023](0023-sidebar-paging.md): the highlight is the
  shown file.

### Deleted files retain explorable source

- When the open document's file is removed, the view keeps rendering
  its last content and a banner row identifies it as
  `deleted from worktree · showing last loaded`; the pill and `:status`
  say it is deleted too.
  Scrolling, search, and reading threads keep working on the last
  content; `c`/`C` and replies are refused with a notice, since a new
  thread would anchor to a snapshot no file matches.
- If the file reappears (an editor's write-then-rename, or the agent
  restoring it), the next change event reloads it and the banner goes;
  threads re-anchor through the reload diff as in
  [0019](0019-reanchoring-edited-lines.md). Switching away and back
  keeps the retained source visible.
- Opening a Git-reported deletion not previously loaded creates a
  read-only, searchable tombstone without creating a worktree file.
  For an `index -> missing worktree` deletion (` D`, `MD`, or `AD`), it
  shows the index blob under
  `deleted from worktree · showing INDEX`. For a staged deletion (`D `),
  it shows the `HEAD` blob under `staged deletion · showing HEAD`.
  Binary and over-limit snapshots use the ordinary file-info
  presentation beneath the same banner. A recreated worktree file is
  loaded normally even when its staged deletion remains.
- Retained source is presentation state, not the worktree side of a
  comparison. Diffs and aggregate counts continue to treat a missing
  endpoint as empty, and deleted tombstones are never written to the
  last-seen or checkpoint stores.
- A tracked deletion keeps its tree row, red `D`, removed-line count,
  and highlight until git no longer reports the deletion
  ([0017](0017-git-status-navigation.md), amended 2026-09-14).
  Other removed files disappear with the rebuild; the highlight stays
  on the neighbouring row and the view keeps the deleted document until
  the reader moves.

### A rename carries its threads

- A rename is detected from the watcher's rename pair (`notify` reports
  `Rename(From)`/`Rename(To)` or a paired `Both` on Linux) when both
  paths are under the root. When the platform delivers an unpaired
  remove-then-create within the debounce window, the pair is matched by
  file size and content hash of the created file against the last-seen
  snapshot of the removed one; no match means delete plus create.
- A rename pair whose old path is created again in the same debounce
  window is not a rename but an editor's backup swap (Helix and Vim
  move the file aside, write a new file at its path, and usually delete
  the backup): it lands as a change to the old path, and the backup
  counts only if it stays. Amended 2026-09-10; before this the open
  document followed the swap to the backup name and stopped reloading.
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

### Rendering follows observable state

- A raw filesystem notification only joins the debounce batch. It does
  not redraw the terminal. A settled relevant batch, input, a timer whose
  visible deadline arrived, a status-walk result, or a viewer socket
  request may draw one frame.
- Subscriber and watch labels are cached in the viewer. The agent
  register is reloaded when `agents.jsonl` changes or the next subscriber
  expires, never as part of drawing `:status` or recording crash state.
- A blocked auto-jump has no polling timer while a popup, selection,
  review list, or diff prevents it from moving. The user event that clears
  the guard schedules the next evaluation.

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
