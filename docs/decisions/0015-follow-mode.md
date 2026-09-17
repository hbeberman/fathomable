---
type: Decision
title: Follow mode and the last-seen diff base
description: Change hints, sidebar badges, toasts, jump keys, debounced auto-jump, and the content-addressed snapshots that define "since I last looked".
resource: crates/fathomable-core/src/follow.rs
tags:
  - decision
  - input
  - git
  - sessions
---

# 0015 Follow mode and the last-seen diff base

Status: accepted (2026-08-26); amended 2026-09-06: `--doctor` reports
the watch budget; amended 2026-09-13: ignored trees spend no watches.

Last-seen storage and behavior superseded 2026-09-16 by
[0087](0087-global-comparisons-and-board-history.md). Live reload, transient
change hints, toasts, and manual change jumps remain; automatic reader
snapshots, seen-idle, mark-all-seen, and last-seen diff routes are removed.

Agent following and auto-jump superseded 2026-09-15 by
[0082](0082-three-tool-review-core.md). Live reload, last-seen snapshots,
change hints and toasts, the changed-file queue, and manual jump keys remain.
MCP `open`/`follow`, automatic movement, the `AUTO` badge, and the
auto/debounce config keys are removed; `jump.toast` remains.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

Milestone 6 of the [roadmap](../roadmap.md) is the charter's "follow the
agent" mode: keep the viewer near what the agent is touching without
constantly jumping. [0007](0007-key-grammar-and-mouse.md) promised a
bottom-line hint and a jump key, with auto-jump as a config option;
[0014](0014-mcp-server-and-socket-v1.md) stores the agent's `follow` list
and shows only a status marker. [0006](0006-git-access.md) named a second
diff base, **last seen**, and milestone 5 shipped HEAD only, parking the
recency heuristic and snapshot bounds. A jump is only useful if it lands on
what changed since the reader looked, so the two are one milestone.

Questions settled with the user on 2026-08-26: hints fire on any workspace
change by default, with the source configurable; hints appear as sidebar
badges, a status-line hint, and timed toasts; auto-jump is a runtime toggle
with a debounce and guardrails; jumps land on the first hunk against the
last-seen snapshot; snapshots are content-addressed in XDG state; the
change queue is newest-first; one milestone, one ADR.

## Decision

### Change sources

- The watcher covers the workspace with one **non-recursive** inotify
  watch per visible directory, filtered by the tree's ignore rules
  ([0012](0012-workspace-mode.md)) plus `follow.ignore` globs. A parent
  is watched before its children are listed; when a visible directory
  appears, it and its visible descendants are watched before their
  already-created files are folded into the pending batch. This keeps
  mkdir-then-populate races live without watching ignored build trees.
  Explicitly loaded files are an exception: their ancestor directories
  are watched even when ignored, so foreground and background files still
  reload and follow renames. Events under `.git/` are never change events (dedicated
  metadata watches drive the base refresh below).
- `follow.source` selects what counts as a change:
  - `workspace` (default): any non-ignored file under the root written or
    created by anyone.
  - `followed`: only paths in the session's agent `follow` list.
  - `open-only`: only agent `open` requests; disk changes never hint.
  In every mode an agent `open` is a change with an explicit target range.
- A burst of writes to one file collapses into one change once the file has
  been quiet for `follow.hint-debounce` (default 300 ms). A change to the
  visible file re-renders as today ([0004](0004-markdown-rendering.md));
  it still enters the queue so its hunks are reachable.

### Last-seen snapshots

- A **snapshot** is the text of a file as the reader last saw it. Snapshots
  live under `$XDG_STATE_HOME/fathomable/workspaces/<hash>/seen/`: a
  `blobs/<sha256>` file per distinct content and a `seen.jsonl` log of
  `{path, sha256, at}` records, last record per path wins, compacted on
  start. Files over 2 MiB and non-UTF-8 files are not snapshotted and have
  no last-seen base.
- A file is **seen** when it was visible and either the reader switches
  away from it, quits, or has not scrolled, searched, or selected in it for
  `follow.seen-idle` (default 5 s). The snapshot is the on-disk text at
  that moment, so what the reader watched arrive during the idle period
  counts as seen, but a change that lands after the idle mark does not.
  (Amended 2026-09-06 by [0069](0069-the-diffs-keys-on-the-bar.md):
  `Space d s` snapshots every non-ignored text file as seen at once.)
- Snapshots survive restarts; on start any blob unreferenced by `seen.jsonl`
  or older than 30 days is deleted, and the log is rewritten to its last
  records. Removing the `seen/` directory is always safe.
- The last-seen base for a file is its snapshot, or, when none exists,
  HEAD as in 0006; a file with neither has no base. `gd` cycles the diff
  view through last-seen and HEAD, the status pill reading `DIFF seen` or
  `DIFF head`; the gutter strip and `]c`/`[c` use whichever base the diff
  view last used, last-seen by default. The `+a -r` counts follow the same
  base.
- The HEAD base is refreshed when `.git/HEAD`, `.git/ORIG_HEAD`, or the
  index changes, closing the milestone-5 gap where a commit showed until
  the next edit.

### Edit deltas

The user wants, later, an animated view of edits landing: a hunk lit in
`diff.plus` or `diff.minus` that fades over a second or two (parked as
"diff-view animation"). The diff system is shaped for it now, without
shipping the animation:

- Every reload of a document keeps a **delta**: the `Diff` between the text
  that was on screen and the text that replaced it, stamped with the time
  it landed. This is distinct from the base diff (last-seen or HEAD), which
  answers "what changed since I looked"; the delta answers "what just
  moved". Deltas are kept per document for a bounded window (the newest
  few, dropped after `follow.toast` has elapsed) and are never persisted.
- Removed lines in a delta keep their text, so a fade-out can draw the
  vanished lines in place before collapsing them; the layout's `diff` face
  set already renders removed rows.
- A delta's hunks are expressed in the new text's line space so they map
  onto rendered rows through the existing source mapping
  ([0004](0004-markdown-rendering.md)); the queue's `Target` for a
  disk change is the first hunk of the newest delta when there is one.
- Rendering stays frame-free today: the delta is consumed once for the
  toast counts and target. An animation later adds a tick timer and a
  per-row intensity from hunk age; no change to `Diff` or the delta store.

(Amended 2026-09-03: the per-document delta window (`Doc.deltas`,
`follow::Delta`) was never read and is removed; a reload yields one
`Diff` that the change hint consumes. The window returns with the
animation, noted in [parked](../parked.md).)

### Hints, badges, and toasts

- The **change queue** holds one entry per changed file, newest first, with
  the target range: the agent's `open` range when given, otherwise the
  first hunk against the last-seen base, otherwise line 1. A later change
  to a queued file moves it to the front and recomputes the target.
- **Sidebar badge.** A queued file's tree row shows `●` in `diff.delta`
  after its name, and each ancestor directory shows the same mark while
  collapsed, so a change is visible however the tree is folded.
- **Status hint.** While the queue is non-empty the bottom line shows
  `→ src/foo.rs +12 -3 (3)`: the newest entry, its line counts against
  last-seen, and the queue length.
- **Toast.** Each change also raises a one-line toast, `src/foo.rs +12 -3`,
  stacked bottom-right above the status line, newest at the bottom, at most
  three visible, each lasting `follow.toast` (default 4 s; `0` disables
  toasts and leaves badges and the hint). Toasts never take focus and draw
  over the view, not the popups.
- An entry leaves the queue, and its badge clears, when the file is viewed
  with its target row on screen, or on `Space j c`.

### Keys

- `Space j` (jump) opens the follow submenu in the space popup, since
  `Space f` stays the file picker from 0012: `j` jump to the newest change,
  `a` toggle auto-jump, `s` cycle `follow.source`, `c` clear the queue.
- `]f` and `[f` step through the queue newest-first and oldest-first, from
  the current file's position when it is queued and from the ends when it
  is not, wrapping with a status message like `]c`. Both open the file at
  its target row and count it as viewed.
- `:follow` and `:follow source <name>` mirror `Space j a` and `Space j s`
  for scripting.

### Auto-jump

- `follow.auto` (default `false`) and `Space j a` enable auto-jump. When
  on, the newest queue entry is opened at its target after the queue has
  been quiet for `follow.jump-debounce` (default 1 s). The status pill
  shows `AUTO` while enabled.
- Auto-jump holds, keeping the entry queued and the hint showing, while any
  of these is true: a selection is active or the comment box is open
  ([0013](0013-annotation-storage-and-ux.md)); the reader scrolled,
  searched, or moved the cursor within the last 3 s; the diff view or the
  thread panel is open; any popup is open. It re-evaluates when the
  condition clears.
- An auto-jump pushes the previous position onto the `[o`/`]o` history so
  it can be undone in one key. (Amended 2026-09-04 by
  [0049](0049-inline-threads-and-the-rail.md): the history is the jumplist and the
  key is `Alt-Left`; the thread pane that held auto-jump back is gone,
  an expanded stub does not. The last-seen snapshot stays automatic;
  checkpoints are the reader's own marks beside it.)

### Configuration

```kdl
follow {
    source "workspace"       // workspace | followed | open-only
    auto false
    ignore "target/**" "*.lock"
    hint-debounce 300        // ms
    jump-debounce 1000       // ms
    seen-idle 5000           // ms
    toast 4000               // ms, 0 disables
}
```

Defaults apply per key; `ignore` adds to the tree's ignore rules rather
than replacing them. Unknown keys are errors as in
[0008](0008-configuration-format.md).

### Doctor and MCP

- `--doctor` reports the snapshot directory, its blob count and size, and
  the effective `follow.source`.
- No MCP tool changes. `session_info` gains `follow_source` and `auto_jump`
  so an agent can tell whether its `follow` list is being honoured; the
  TUI answers it, and the socket alone answers without those fields when
  the loop is gone.

## Consequences

- The workspace consumes one inotify watch per visible directory, plus
  the small explicit sets for the open file, workspace state, and Git
  metadata. Ignored trees consume no watches and emit no event-loop work.
  `--doctor` counts the visible directories against
  `fs.inotify.max_user_watches`; if only part of that set can be watched,
  the viewer says live updates have partial coverage while retaining the
  watches it could install and the open file's path.
- Snapshot state grows with distinct viewed contents, bounded by the 2 MiB
  cap, the 30-day prune, and content addressing across paths.
- `Config` gains a `follow` block; `Session` gains the follow source and
  auto-jump flag; the tree learns per-row badges.
- The milestone-5 follow-ups for last-seen, changed-file jumping, and base
  refresh on commit leave [parked ideas](../parked.md). Side-by-side diff
  stays parked.
