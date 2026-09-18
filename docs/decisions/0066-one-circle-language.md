---
type: Decision
title: One circle language and the grouped threads pane
description: Every surface that names a thread draws one lifecycle circle; the Threads pane groups two-row summaries by file, presents passive responsive scope and counts beside a title settings menu, and keeps row menus item-local; the review list is the same view full screen, and every act on these surfaces takes the mouse.
resource: crates/fathomable/src/app/draw/threads_pane.rs
related_resources:
  - crates/fathomable/src/app/threads/pane.rs
  - crates/fathomable/src/app/threads/list.rs
  - crates/fathomable/src/app/threads/words.rs
  - crates/fathomable/src/app/threads/mod.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/draw/header.rs
  - crates/fathomable/src/app/draw/gutter.rs
  - crates/fathomable/src/app/threads/detached.rs
  - crates/fathomable/src/app/input/mouse.rs
  - crates/fathomable/src/app/input/menu.rs
  - crates/fathomable/src/app/input/bindings.rs
tags:
  - decision
  - annotations
  - input
  - rendering
---

# 0066 One circle language and the grouped threads pane

Status: accepted (2026-09-05). Amended 2026-09-05 by
[0067](0067-the-texts-key-bar.md): the review list's cursor entry header
carries no keys; the list's bar has them. Amended 2026-09-11 by
[0075](0075-the-header-names-its-counts.md): the headers' counts carry
words, a `◐` thread counts on its own, and the list is `review threads`.
Selection superseded 2026-09-14 by [0079](0079-list-focus-language.md):
thread and file-group rows distinguish active from remembered selection
with shared list roles, overriding current-file `thread.focus` context;
overlays and prefixes deactivate them. Review messages keep author
stripes, with a bright bar only while review owns the keys.

Lifecycle and summary presentation superseded 2026-09-16 by
[0085](0085-thread-lifecycle-and-auto-resolve.md) and
[0086](0086-one-thread-summary-and-its-actions.md). Every surface still uses
one glyph, but it now means lifecycle only: `●` active, `◐` resolution
proposed, `○` resolved. Detached placement is the `?` suffix on `Lx-y?`,
not a glyph; last-author colours, waiting, and overlapping counts retire.
The sidebar's grouped two-row cards and file folding remain, backed by the
shared summary facts.

Amended 2026-09-17: the pane header is a settings-button model. Only
`Threads` takes title hover and opens the checked **Only current file** and
**Show resolved** settings below the header. Its subdued scope and lifecycle
counts are passive and right-aligned; `file`/`workspace` shorten to `f`/`w`
before scope is omitted. A pane file-group menu no longer carries the
pane-wide resolved setting.

Amended later 2026-09-17: the normal full-column review view adopts the same
model. `Reviews` is the clickable left title; passive `workspace`/`file`
scope and lifecycle counts sit at the right, with `w`/`f` as the compact
scope. Its title menu begins with **Open File**, then carries **Only current
file** and **Show resolved** below a separator.

## Context

A thread's state was said four ways. The gutter drew `╭ │ ╰` and a
small `•` in the state colour ([0027](0027-revisiting-threads.md),
[0039](0039-gutter-colour-and-detached-rows.md)); a stub drew `●` in
the same colour whatever the state; the threads pane drew `●` or `✓`
([0032](0032-placement-and-state.md)); the files pane tagged a file
with a waiting thread `↩` ([0030](0030-waiting-threads.md)); and a
proposal ([0053](0053-resolution-is-the-users.md)) was a word alone,
with no cue on any glyph. The pane's one-row entries (`● L3 three ↩2
5m`) cut the newest message with no ellipsis and named no author, so
the reader could not tell an agent's answer from their own comment
without opening it. In workspace scope the pane listed threads in the
review's recency order with the file name folded into each place
(`lib.rs:9`), so two threads of one file sat apart and the file was
repeated on every row. The review list had its own order (`s`), its
own fold (`z` on an entry), and its own header words, so the two
surfaces disagreed about the same threads.

Nine mockups drawn from the viewer's own chrome were reviewed on
2026-09-05 (`.tmp/mockups-threads-pane/`); the choices are below. A
question round the same day settled four points the mockups left open:
the files pane keeps its change badge beside the thread circle; the
review list's cursor thread selects both its header and its message;
the pane and the list each keep their own fold set; and `j`/`k` stop
once on a folded file.

## Decision

### One circle language

Every surface that names a thread draws the same glyph in the same
colour. The colour is whose turn it is; the fill is the lifecycle.
Since [0071](0071-author-stripes.md) the turn colours are the author
colours: `thread.open` is the user's blue, `thread.waiting` the
agents' green (amber and teal until 2026-09-09).

| glyph | colour | meaning |
| --- | --- | --- |
| `●` | `thread.open`, the user's hue | open, waiting on an agent or anyone |
| `●` | `thread.waiting`, the agents' hue, bold | waiting on the user: an agent's reply is newest |
| `◐` | the state colour | an agent's newest reply proposes resolving (0053) |
| `○` | grey `thread.resolved` | resolved |
| `?` | the state colour | the thread's lines are gone (`Placement::Detached`) |

- `Words` ([0032](0032-placement-and-state.md)) gains `glyph()`, the
  one place the circle is chosen; a `Mark` carries its circle too, so
  the gutter and the stubs need no second lookup.
- **The gutter** keeps `╭ │ ╰` in the state colour. A thread that fits
  one rendered row draws its circle, one dot size everywhere; the small
  `•` retires. A detached thread's row draws `?`. Where several one-row
  threads share a row the most urgent circle wins, as the colour did.
- **The stubs and the expanded header** draw the circle in place of
  their `●`. A stub draws it once, on its newest message's row; the
  older row keeps the cell blank so the names align (until 2026-09-09
  every stub row drew it, and a stub of three or more messages showed
  its oldest two; later the same day a stub became one row, the
  newest message, so the circle is on its only row).
- **The files pane** draws one circle after a file that has listed
  threads, the most urgent among them, no count; a folded directory
  rolls up its children's most urgent circle (`▸ docs/ ●`). The `↩` tag
  retires. The change badge of [0015](0015-follow-mode.md) stays before
  it: two marks, two meanings. Urgency is the state first (waiting,
  open, resolved), then `?`, `●`, `◐`, `○`, so a file with a thread to
  answer reads in the waiting colour whatever else it holds.
- **The status line** draws a waiting-coloured `●` before `2 waiting`; `proposed`
  and `threads` stay words.
- `✓` retires from every surface.

### The pane's rows

- Two rows per thread, the stubs' form. Row 1: the circle, the place
  in the dim colour, the author of the newest message, then `↩n age`
  flush right in the info colour. Row 2: the newest message's first
  line, indented under the place and truncated with `…`. The author
  reads `name (role)` when the row has room and `name` when it does
  not; there is one form and no compact toggle. (Amended 2026-09-06 by
  [0070](0070-one-workspace-many-worktrees.md): an entry the active
  worktree does not reach carries its worktree's branch, dim, after
  the author. Amended 2026-09-11 by
  [0077](0077-threads-nest-under-their-file.md): both rows sit two
  cells in under the file row's path, in file scope too.)
- A thread's two rows are one unit: `j`/`k`, the wheel, and a click
  treat them as one entry, and they are never separated.
- `fit` gains a sibling, `fit_ellipsis`, that marks a cut with `…`; the
  pane, the stubs, and the list's rows share it.

### Grouping

- In workspace scope the pane groups by file: a file row in
  `ui.sidebar.dir` with the thread count at the right edge, then that
  file's threads by line. Files follow the files pane's order
  (directories before files at each level, names case-insensitively),
  whether or not the files pane is shown. A file appears only when it
  has a listed thread, so with resolved hidden a file with only
  resolved threads is absent. The current file's rows draw on
  `thread.focus`.
- The place in a grouped row is `L14-16` or `file`; the file name is on
  the group row. File scope lists the current file's threads with no
  file row, as before.
- `z` on a file row folds it to `▸ path  n`; `z` on a thread row folds
  its file, there being nothing else for it to do; `Z` folds every file
  or unfolds them all when any is folded (the case rule of
  [0065](0065-z-folds-and-unfolds.md)). Folded state survives scope and
  file switches. A folded file is one stop for `j`/`k` and the wheel:
  they land on its first thread, the file row highlighted, so `z` can
  unfold it, and the next step leaves the file. A thread cursor inside
  a folded file, however it got there, highlights the file row.
- The pane and the review list each keep their own fold set: what the
  reader closes in the sidebar stays open on the full screen.

### Header and key bar

- The header reads `Threads` at the left. Flush right, subdued
  `workspace` or `file` names the pane's scope before lifecycle counts:
  `● 2 active`, `◐ 1 resolution proposed`, and `○ 1 resolved`, each in its
  lifecycle colour. Zero counts are omitted; with resolved hidden, the
  resolved count still says what the setting would reveal and is dimmed.
  The count words drop together on narrower rows, then scope shortens to `w`
  or `f`, then scope is omitted before the lifecycle counts.
- Only the `Threads` title has the shared title-hover background. A
  left-click opens a menu directly below the pane header with checked
  **Only current file** and **Show resolved** settings. Scope and counts are
  display state, not click targets.
- The keys leave the header. While the pane has the keys its bottom
  row is a key bar on `ui.header`: cursor-thread actions begin with
  `reply c · auto-resolve R · resolve r` (or `reopen r`), followed in
  workspace scope by `fold z · fold all Z`, then `scope s · resolved x`.
  Hints drop from the end when narrow; `z` and `Z` show only where they
  work ([0064](0064-hints-you-can-press.md)).
  The bar replaces the bottom row; the rows above it do not move, and
  when the pane loses the keys that row is an entry row again.
- The cursor's entry draws both rows on `ui.picker.selected`, bold,
  the circle in its state colour, whether or not the pane has the keys.
- The pane scrolls to keep the cursor's entry on screen; the default
  split stays at 8 rows and the drag stays. The header's counts say
  what is above and below.

### The review list

- The review list is the threads view full screen: the same file rows
  in the same order, always by file then line, every thread open with
  its messages under a header that matches the expanded thread in the
  text: `●  L14-16  waiting  10m ago`, on `ui.header`. The path is on
  the file row, not repeated on every header. `f` narrows to the
  current file, with no file row, as the pane's file scope does.
  (Amended 2026-09-11 by
  [0077](0077-threads-nest-under-their-file.md): a thread's rows sit
  two cells in under the file row, in file scope too, and the file row
  draws the cursor bar while the cursor is on one of its threads.)
- The cursor's thread draws its header bold on `ui.picker.selected`
  with the editing hints (`c reply · e edit · o resolve · z fold`) at
  its right edge while the list has the keys, and its message keeps the
  selected band, so `l`/`h` and `e` stay visible.
- `z` folds or unfolds the cursor's file; `Z` folds or unfolds every
  file. A folded file is one stop for `j`/`k`, as in the pane. `s`
  retires: one order everywhere. `x` stays. (Superseded 2026-09-11 by
  [0076](0076-threads-fold-in-the-list.md): every thread in the list
  folds as in the text, `z` acts on the row the cursor is on, file
  rows are stops, `Z` folds or expands every thread and the list has
  no fold-all for files, and a file row always draws `▾` or `▸`; the
  pane keeps its file folds and gains the arrows.)
- The normal header reads `Reviews` at the left, then passive
  `workspace`/`file` scope and lifecycle counts at the right. Count words
  drop together, scope shortens to `w`/`f`, and scope is omitted before the
  counts. The `Reviews` title opens checked **Only current file** and **Show
  resolved** settings below the header. Amended by
  [0086](0086-one-thread-summary-and-its-actions.md), the key bar reads
  action then hotkey, beginning `reply c · auto-resolve R · resolve r ·
  fold z · fold all Z`; scope, traversal, open, and escape hints follow
  where they fit.

### Mouse

Everything the keys do here the mouse does too, extending
[0050](0050-mouse-menus-and-gestures.md) and
[0059](0059-headers-and-the-key-bar.md).

- A thread's two rows are one hit target; the wheel steps by thread.
- A file row (in the list since 2026-09-11 the click also rests the
  cursor on the row, and the list's file menu has no fold-all,
  [0076](0076-threads-fold-in-the-list.md)): a left-click folds or unfolds it, as a click on a
  directory in the files pane; a right-click puts the cursor on the
  file's first thread and opens the file's menu. In the pane it offers
  fold/unfold, fold/unfold all, and open file; in the review list it offers
  fold/unfold, open file, and show/hide resolved.
- The pane's header: a click on `Threads` opens the checked scope and
  resolved settings below the header. A click on its passive scope or counts
  only focuses the pane.
- The normal review header works the same way: `Reviews` opens the checked
  scope and resolved settings below the text-column header; its passive state
  only focuses the view.
- The key bar: a click on a hint runs it; a pane without the keys
  focuses on the first click and shows the bar, as the list does.
- The files pane: a right-click on a file with listed threads gains
  `threads` and `review`. `threads` shows the threads pane in file
  scope on that file and gives it the keys; it is bound to `t` in the
  files pane so the entry has a key, as every entry must. `review`
  opens the review list on the file's first thread, as `Space r` does
  once the file is shown. A click on a folded directory's circle is a
  click on the directory.
- The status line takes two clicks, which
  [0050](0050-mouse-menus-and-gestures.md) said it did not: the
  waiting count opens the review list and the `n threads` count focuses
  the threads pane.
- In the review list a click on a thread's header selects the thread,
  a click on a message row selects the message, and a right-click opens
  the thread menu: go to, reply, resolve or reopen, edit, delete, and
  `fold file`.

### Amendments

- 0025's list section, 0027's file-threads pane, 0030's `↩` tag,
  0032's `✓`, 0039's `•`, 0049's threads pane and review list
  sections, 0050's status line rule and threads pane header click,
  0053's header counts, 0059's review header words and sort click, and
  0065's review list `z` are superseded as described above; each
  carries a dated note.

## Consequences

- `app/draw/threads_pane.rs`, which this record backs, draws the pane:
  the header with its counts, the file and thread rows, and the key
  bar. `app/threads/pane.rs` computes the rows (`PaneRow::File` and
  `PaneRow::Thread`), the stops, the fold set, and the scroll.
- `ReviewSort` and `Action::ReviewSort` go; `ReviewList` keeps a fold
  set of paths; `review_entries` sorts by the files pane's order, and
  `Row::File` joins the list's rows.
- `Action::Fold` and `Action::FoldAll` are bound on the threads pane
  and `FoldAll` on the review list; `Action::ThreadsOnFile` is new on
  the files pane.
- The status line's right block is segments with their click actions.
- `docs/guide.md` gains the circle table, the pane's rows and fold
  keys, the review list's keys, the files pane's `t`, and the mouse
  additions in the same change.
- Not done: a `copy link` entry on a thread's menu was named in the
  review and has no action to run; it waits for a decision on what a
  thread's link is.

The selected surface under the cursor's entry and message was replaced
by a cursor bar and bold text in [0071](0071-author-stripes.md), which
also stripes every message by its author.
