---
type: Decision
title: Inline threads, the rail, checkpoints, and the jumplist
description: Threads show under their lines as two-row stubs that c expands in place and the bottom thread pane goes; the left column is the rail, holding the tree pane and the threads pane as peers at a fixed split; Space A is the review list sorted by newest agent reply with resolved hidden; checkpoints of a file or the workspace sit on a per-file timeline beside git diff; Alt-Left and Alt-Right walk a jumplist of positions; and the leader gains c, v, and r submenus with a breadcrumb in the menu.
resource: crates/fathomable/src/app/threads/stubs.rs
related_resources:
  - crates/fathomable-core/src/layout/mod.rs
  - crates/fathomable/src/app/mod.rs
  - crates/fathomable/src/app/files_pane.rs
  - crates/fathomable/src/app/view.rs
  - crates/fathomable/src/app/threads/mod.rs
  - crates/fathomable/src/app/threads/cursor.rs
  - crates/fathomable/src/app/threads/pane.rs
  - crates/fathomable/src/app/threads/list.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/input/bindings.rs
  - crates/fathomable/src/app/jump.rs
  - crates/fathomable/src/app/jumplist.rs
  - crates/fathomable-core/src/config.rs
  - crates/fathomable-core/src/theme.rs
  - crates/fathomable-core/src/checkpoints.rs
  - crates/fathomable-core/src/workspace.rs
  - crates/fathomable/src/app/checkpoints.rs
tags:
  - decision
  - annotations
  - input
  - rendering
  - checkpoints
---

# 0049 Inline threads, the rail, checkpoints, and the jumplist

Status: accepted (2026-09-04); amended 2026-09-05 by
[0060](0060-one-diff-two-sides.md): the checkpoint diff is the one diff
view with a checkpoint base, and `Space v r` / `c` / `C` / `g` are
`Space d r` / `c` / `C` / `g`; amended 2026-09-05 by
[0066](0066-one-circle-language.md): the threads pane's rows, order,
header, and keys and the review list's order, folds, and header are as
that record says (`s` is gone from both); amended 2026-09-05 by
[0067](0067-the-texts-key-bar.md): the expanded thread's header carries
no keys and the stub no `(c expand)`; the text's keys are on a bar
along the column's bottom row, and the thread cursor's stub reads bold;
amended 2026-09-04 as the work landed:

- `dd` on an expanded thread's rows deletes the **thread**, not the
  message under the cursor: the store has no message-delete event
  ([0034](0034-deleting-threads.md) has only thread tombstones). `e` edits
  the message under the cursor when it is the user's, else the user's
  newest; `Space c e` edits the user's newest message from any pane.
- The checkpoint header and pickers name a checkpoint by its age (`5m
  ago`, `yesterday 14:02`, else the UTC date), not a local clock time:
  the app has no time zone. A checkpoint whose content equals the
  file's latest entry adds nothing under either key (toast `checkpoint:
  nothing changed`), so no pair on a timeline is ever an empty diff.
- `Space v r` toggles the view; `gs`, `gd`, and `gD` leave it too. `b`
  and `t` outside the view only say how to open it.

## Context

Reading a file with threads on it meant three surfaces and three keys:
`Space a` for the thread pane along the bottom, `Space t` for the
file-threads pane under the tree, `Space A` for the thread list in place
of the text. Each had its own open, close, and focus rules
([0010](0010-viewer-ux.md), [0013](0013-annotation-storage-and-ux.md),
[0025](0025-thread-list.md), [0027](0027-revisiting-threads.md)), the
file-threads pane existed only while the tree was shown and grew with
the file's thread count, and none of them put a thread where the reader
was looking: the gutter said *a thread is here* and the pane, elsewhere,
said what it was. The `]c`/`[c` family walks threads but the user does
not like relying on bracket chords and wanted stubs that are useful at a
glance when three or four stack in one region and still give context
while scrolling the whole file.

"Where was I" had two answers: the opened-file history on `[o`/`]o`
([0012](0012-workspace-mode.md)) remembered files but not positions,
and a search jump or `G` within a file left nothing to come back to.

"What changed since I looked" had one answer, the automatic last-seen
snapshot of [0015](0015-follow-mode.md), which moves on its own. There
was no way to mark *this is where my review stands* for one file or the
whole workspace and page through those marks, and no way to diff from a
chosen commit to now while an agent keeps committing.

The design was drawn as eleven mockups from the viewer's own chrome and
reviewed on 2026-09-04; four question rounds settled the shape and a
fifth the five points the mockups left open. This record holds the
result.

## Decision

### Vocabulary

- The **rail** is the left column. It holds the **tree pane** above the
  **threads pane** as peers. "Sidebar" and "dock" are not used.
  Amended 2026-09-05 by [0057](0057-the-sidebar.md): the column is the
  **sidebar** after all, the `rail` node and `ui.rail*` keys with it;
  the tree pane has been the **files pane** since
  [0056](0056-the-leader-trimmed.md).
- A **stub** is the condensed block a thread shows under its last
  anchored row. A stub is **collapsed** (two rows at most) or
  **expanded** (the whole thread).
- A **checkpoint** is a recorded content of one file at a moment, on
  that file's **checkpoint timeline**. A **workspace checkpoint** appends
  one to every file whose content moved.
- The **jumplist** is the ordered positions (path, source line) that far
  moves leave behind.
- The **thread pane** and the **file-threads pane** are gone; the thread
  list of [0025](0025-thread-list.md) is the **review list**.

### Inline stubs

- **Rendering.** Directly under the last rendered row of a thread's
  range (under its detached row when detached,
  [0039](0039-gutter-colour-and-detached-rows.md)), one row per message
  for the newest two messages, oldest first: the state glyph in the
  state colour (`●` open or waiting, `✓` resolved), the author (agents
  as `name (type)` in the agent colour), the age, then the first line of
  the body truncated with `…`. Rows never wrap and carry no line number;
  the thread's gutter bracket ends on its last text row. Stub rows sit
  on the `thread.inline` background.
- **Stubs are not lines.** `j`/`k`, `Ctrl-d`/`Ctrl-u`, `gg`/`G`, search,
  `:N`, `v`/`V`/`x` selection, `y`, `c` on a selection, and the gutter
  address source rows only; a collapsed stub is skipped as the detached
  row is. A click on a stub sets the thread cursor to it and expands
  it; the wheel scrolls through it.
- **Highlight.** When the text cursor is on a row the thread covers (or
  on its detached row), the stub's text takes `thread.focus`'s
  foreground and the thread's own rows take the focus tint as today.
  The thread cursor's stub ([0046](0046-one-thread-cursor.md): the
  thread starting on the cursor line, else the first on its row, else
  the nearest above) additionally shows `(c expand)` at the end of its
  last row in `ui.hint`; other covering threads' stubs are tinted but
  carry no hint.
- **Stacks.** Threads whose stubs land under the same row are laid out
  one thread after another in line order (start line, then end line,
  then id): every row of thread A, then every row of thread B. Never
  merged, never interleaved by time.
- **Expansion.** `c` in the text with no selection, on a row a thread
  covers, expands the thread cursor's stub in place: a header row
  (`● waiting  watched by demo (coder)` with `r reply  e edit  o resolve
  c fold` at the right edge), then every message as the pane drew them
  (author and age row, body rows as Markdown,
  [0037](0037-markdown-in-threads.md)). The view does not scroll and
  there is no END row. An expanded thread's rows **are** cursor rows:
  `j`/`k` walk its messages (the message under the cursor is the thread
  cursor's message), `e` edits and `dd` deletes that message, `r`
  replies (the comment box as today, focus returns to the message), `o`
  resolves or reopens; `c` on any of its rows folds it back to a stub.
  Only expanded rows are walkable; collapsed stubs stay skipped. `Esc`
  on an expanded row folds nothing; it clears as today.
- **`c` cycles from the thread cursor.** When several threads cover the
  cursor row, the first `c` expands the thread cursor's thread, the one
  the hint marks; each further `c` folds it and expands the next
  covering thread in line order, wrapping through the ones before it;
  after the last, `c` folds it and expands nothing. So the hint and the
  key always agree. `c` on a row with no thread comments on the line as
  today; `C` always starts a new thread.
- **Toggles**, all under `Space c`, from any pane:
  - `Space c c` toggles stub visibility for the session (toast `stubs
    hidden` / `stubs shown`). Hidden stubs leave the gutter marks, the
    focus tint, the rail, and every motion unchanged. Default shown,
    config `threads { stubs #true }`.
  - `Space c x` toggles resolved stubs. Default **hidden**, matching the
    review list; config `threads { stubs-resolved #false }`. The gutter
    still shows a resolved thread in grey, so the margin loses nothing.
  - `Space c z` expands every visible stub in the file, or folds every
    expanded one when any is expanded.
  - Amended 2026-09-04: **`Space c c` and `Space c z` are unbound.**
    Neither was reached for; `threads { stubs }` still sets whether
    stubs are drawn, and `c` and a click expand one at a time.
  - `Space c n` starts a new thread on the cursor line (as `C`);
    `Space c r` / `o` / `e` / `d` reply to, resolve or reopen, edit the
    newest own message of, and delete the thread cursor's thread, so a
    reply never needs an expanded stub.
- Expansion state is per thread for the session. A thread that gains a
  message while expanded stays expanded, and its newest message becomes
  the thread cursor's message when the reply is the user's, as 0046.
- **Removal.** The thread pane (`Focus::Thread`, `Space a`, its
  `h`/`l`/`H`/`L`, `Esc` back to the text) goes. `]c`/`[c`, `]C`/`[C`,
  `]r`/`[r` stay in the text; `]r` expands the thread it lands on, since
  reading the reply is its point; `]c`/`]C` do not expand. The one
  thread cursor of 0046 keeps its meaning: the text drives it, and an
  expanded thread's message rows are where its message index shows.

### The rail

- The left column is the rail, `rail { width 32 }` columns wide, with
  the tree pane above the threads pane. The split is fixed at `rail {
  split 8 }` rows for the threads pane, draggable with the mouse for the
  session; it no longer grows with the file's thread count. The threads
  pane exists whether or not the tree pane is shown; the rail is drawn
  when either pane is shown.
- `Space e` shows and focuses the tree pane, or returns focus when it
  is focused; `Space E` hides it and leaves the threads pane, and shows
  it again without taking the keys. `Space t` / `Space T` do the same
  for the threads pane. Amended 2026-09-04: **`Space t` is the review
  list**, the one place that shows every thread, so the lowercase key
  reaches it, and `Space T` focuses the threads pane or returns. The
  hides nest under a `Space p` **panes** submenu: `Space p e` hides or
  shows the tree pane, `Space p t` the threads pane. `Space E` and
  `Space A` are unbound. `Space r` opens the rail
  submenu (amended 2026-09-04: its breadcrumb word is **tree**, since
  every entry acts on the tree pane; the column itself is still the
  rail): `r` re-reads the directories, `i` toggles ignored entries,
  `.` reveals the current file in the tree, expanding to it and moving
  the tree highlight. `R` and `I` in the tree pane stay as aliases.
  Folding these under `Space e` was rejected: `Space e` stays the
  one-key focus toggle the 2026-08-26 round asked for. `h` at column 0
  in the text focuses the tree pane as today.
- **Threads pane.** Header `threads · file N` or `threads · workspace N`
  with `s x` at its right edge; `s` toggles the scope, `x` toggles
  resolved rows. One row per thread: glyph, place (`L9-11` in file
  scope, `lib.rs:9` in workspace scope), the first line of the newest
  message truncated, `↩n` when replied, age. Order follows the review
  list's sort; in file scope the default is line order. `j`/`k` move the
  thread cursor (the text follows; in workspace scope the file opens),
  `Enter` opens the file and expands the thread, `r`/`o`/`dd` act on the
  highlight, `Esc` returns to the text. The pill reads `THREADS`. The
  highlighted row is the thread cursor's, so reading the file walks the
  pane as today.

### The review list

- `Space A` (`Space t` since the 2026-09-04 amendment above) keeps its
  placement: the text column, the rail beside it,
  pill `REVIEW`. It opens sorted by **newest agent reply first**: threads
  whose newest message is not the user's, newest first, then the rest by
  their newest message. `s` toggles to file and line order, today's
  grouping. Resolved threads are **hidden** by default and `x` toggles
  them; `Z` goes, `z` still folds the entry. Every entry header carries
  `path  Lstart-end  state  age` in both orders. `f` narrows to the
  current file. `Enter` opens the file with the thread expanded. `r`,
  `e`, `o`, `dd` as today. Amended 2026-09-04: **`j`/`k` step between
  threads** and land on the newest message, as an inbox's rows do, and
  `l`/`h` step between the selected thread's messages; the two pairs
  swapped. `]r`/`[r` in the text gain `Tab`/`Shift-Tab` aliases, the
  next thing that needs the reader being the most-used motion.
- The header reads `review  4 open  resolved hidden  by newest agent
  reply` with `s sort  x resolved  f file  z fold` at the right.
  Amended 2026-09-05 by [0059](0059-headers-and-the-key-bar.md): the
  header reads `review  4 open · resolved hidden` with the sort word
  at the right edge, and the keys are on a bar along the list's bottom
  row. The sort, the resolved flag, and the file filter are one `ReviewState` on
  `App` that the threads pane reads: its `s` switches scope, its `x` is
  the same resolved flag.

### Checkpoints

- **Model.** Every file has a checkpoint timeline: entries `(created,
  origin, blob)` in time order, where `origin` is `file` or `workspace`
  and `blob` is the file's content, stored once per content under
  `$XDG_STATE_HOME/fathomable/workspaces/<hash>/checkpoints/<sha256>`,
  with an append-only `checkpoints/index.jsonl` of events
  `{ "event": "checkpoint", "id", "created", "origin", "files": [{
  "path", "blob" }] }`. Both keys append to the same timelines, which is
  what makes one file's view and the whole-repo sign-off one mechanism:
  - `Space v c` **checkpoints this file**: one event with one file.
  - `Space v C` **checkpoints the workspace**: one event listing every
    non-ignored file whose content differs from its latest entry, or
    has none. A file the agent has not touched since its last checkpoint
    gets no new entry, so its strip does not grow and paging never lands
    on an empty diff; a no-op tick per file was rejected. Files git
    already has at `HEAD` are still stored: the store does not depend on
    git state. A toast counts them: `checkpoint: 3 files`.
- The automatic last-seen snapshots of [0015](0015-follow-mode.md) keep
  running underneath for re-anchoring and `gD`; they are not on the
  timeline.
- **View.** `Space v r` opens the **checkpoint diff** (pill `CHECK`) for
  the current file: a unified diff between two sides, opened on the
  newest pair, latest checkpoint to the working file. `h`/`l`
  (`Left`/`Right`) page to the earlier or later pair along the timeline;
  the header reads `checkpoint 2/3  14:02 · now`. `b` and `t` open a
  picker to set the base or the target side to any of: the file's
  checkpoints (time, `◆` when workspace-wide), the commits that touched
  the file reachable from `HEAD` (short id, age, subject; at most 50),
  `HEAD`, and the working file; the header then names both sides
  (`a1b2c3 · now`). `Space v g` is the shortcut for "a commit to now":
  the same picker for the base with the target fixed at the working
  file. A strip at the bottom of the view lists the file's checkpoints
  with `◆` on workspace-wide ones. With no checkpoint the view says so
  and names `Space v c`.
- `Space v s` / `d` / `D` are the source view, the git `HEAD` diff, and
  the last-seen diff, beside `gs`/`gd`/`gD`, which stay. Checkpoints sit
  beside git diff and never replace it.
- Checkpoints are never expired automatically in this version;
  `--doctor` counts them. Pruning, and marking stub and rail rows "new
  since checkpoint", are [parked](../parked.md).

### The jumplist

- `App` keeps a jumplist of `(path, source line)` positions, at most
  100. A **far move** pushes the position it leaves: opening another
  file by any route (the tree, the pickers, `]f`/`[f`, `]G`/`[G`,
  `]C`/`[C`, `]r`/`[r`, the review list's or the threads pane's `Enter`,
  `Space j j`, auto-jump, an agent `open`); within a file, a search jump
  (`/`, `?`, `n`, `N`), `gg`/`ge`/`G`, `:N`, `]c`/`[c`, `]g`/`[g`.
  `j`/`k`, `Ctrl-d`/`Ctrl-u`, `h`/`l`, and the mouse do not.
- `Alt-Left` goes back, pushing the current position on first use so
  `Alt-Right` returns; `Alt-Right` goes forward; a new far move after
  going back truncates the forward part. Duplicate consecutive positions
  collapse. `Alt-Left`/`Alt-Right` were chosen over `[j`/`]j`: the user
  does not want new features to depend on the bracket chords.
- `[o`/`]o` and the opened-file history of
  [0012](0012-workspace-mode.md) are removed. The recent-files picker,
  `Space o`, stays.

### The leader and the menu

```
Space e           tree pane: focus or return
Space t           review list (s sort, x resolved, f file, z fold)
Space T           threads pane: focus or return
Space p e / t     panes: hide or show the tree pane / the threads pane
Space r r/i/.     tree: re-read, toggle ignored, reveal this file
Space f / F       file picker / with ignored
Space o           recent files
Space c x         toggle resolved stubs
Space c n/r/o/e/d new thread here, reply, resolve or reopen, edit, delete
Space v s/d/D     source, git diff, diff last seen (gs gd gD stay)
Space v r         checkpoint diff (h/l page, b base, t target)
Space v c / C     checkpoint this file / the workspace
Space v g         diff against a commit…
Space j j/a/c     newest change, auto-jump, clear
Space w           wake an agent
Space ?           all keys
Alt-Left / Alt-Right   jumplist back / forward
```

- Display toggles, tree actions, and thread actions on the cursor move
  under `Space`; the `:` commands do not. Every sequence is a row of the
  binding table ([0045](0045-bindings-are-data.md)) and of the guide's
  key tables in the same commit.
- The which-key box gains a first row naming the prefix typed so far and
  its group word (`Space c · threads`), in the pill colour, and
  re-renders at every level. The status badge still shows the raw
  prefix. Because the breadcrumb names the submenu, an entry inside one
  drops that word from its label: `Space c x` reads
  `toggle resolved stubs`, not `threads: toggle resolved stubs`.

### Theme and configuration

- New theme keys ([0011](0011-theme-schema.md) vocabulary):
  `thread.inline`, the stub and expanded-thread background, default a
  low-contrast tint beside the theme's own background (dark near
  `#171d27`, light near `#eef2f7`); `ui.hint`, the `(c expand)`
  affordance, dimmer than `ui.dim`. On a transparent terminal a
  background cell is always opaque, so the default is low-contrast
  against the theme's background rather than the terminal's; a theme may
  set `thread.inline` to `none`, in which case stub rows are marked by a
  `▎` in the state colour at their left edge instead. The `ui.sidebar.*`
  keys become `ui.rail.*`, with the old spelling accepted for one
  release as [0047](0047-one-vocabulary.md) did for `annotation.*`
  (retired 2026-09-04 by
  [0051](0051-retire-one-release-compatibility.md)).
- Config nodes `threads { stubs #true; stubs-resolved #false }` and
  `rail { width 32; split 8 }`; `checkpoints {}` is reserved.
  `--config-show` prints them. Note (2026-09-05): the reserved
  `checkpoints` config block was removed unused; a retention setting
  will add it back.

### What goes away

`Space a`, the thread pane and `Focus::Thread`, the file-threads pane's
dependence on the tree, `Space t` closing the thread pane, `[o`/`]o` and
the opened-file history, the review list's `Z`, the `LIST` pill (now
`REVIEW`), and the `TREE` pill's meaning "the sidebar" (now the tree
pane).

## Consequences

- A thread is read where its lines are; the gutter, the stub, and the
  expanded thread are one place. The bottom pane's scroll, its header,
  and its `Tab`-less scope are gone with it.
- The rail is one column with two panes that the reader shows and hides
  independently; the threads pane no longer disappears with the tree.
- The review list is an inbox: what an agent said last is at the top,
  and what is done is out of the way until asked for.
- A checkpoint is a deliberate mark the reader makes; last-seen remains
  the automatic one. The store is content-addressed and independent of
  git, so a checkpoint survives a commit, a rebase, and a rewrite.
- Far moves are undoable in one key on any surface, and the history is
  positions rather than files.
- The work lands in order, each step leaving the app usable: the leader
  map and the reactive menu; the jumplist (a new `app/jumplist.rs`
  beside the auto-jump module `app/jump.rs`); the rail; collapsed stubs
  and their toggles, then expansion and the pane's removal; the review
  list; the checkpoint store, then its view; then the vocabulary pass.
- Amended by this record where they describe the surfaces it removes:
  [0010](0010-viewer-ux.md) (the pills and Esc rule),
  [0012](0012-workspace-mode.md) (the sidebar and `[o`/`]o`),
  [0013](0013-annotation-storage-and-ux.md) (`Space a`),
  [0015](0015-follow-mode.md) (auto-jump and the history),
  [0025](0025-thread-list.md) (sort, `Z`, headers),
  [0027](0027-revisiting-threads.md) (the file-threads pane),
  [0045](0045-bindings-are-data.md) (the breadcrumb row),
  [0046](0046-one-thread-cursor.md) (the pane's keys),
  [0047](0047-one-vocabulary.md) (rail, stub, checkpoint, review list).
