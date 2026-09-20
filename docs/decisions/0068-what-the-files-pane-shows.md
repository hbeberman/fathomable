---
type: Decision
title: What the files pane shows
description: The File list filters paths and can keep directories recursively unfolded through keys under `Space F` and checked settings under its clickable title.
resource: crates/fathomable/src/app/files_shown.rs
related_resources:
  - crates/fathomable-core/src/tree.rs
  - crates/fathomable/src/app/input/bindings.rs
  - crates/fathomable/src/app/input/menu.rs
  - crates/fathomable/src/app/draw/header.rs
  - crates/fathomable/src/app/draw/mod.rs
tags:
  - decision
  - input
  - git
  - rendering
---

# 0068 What the files pane shows

Status: accepted (2026-09-06)

Amended 2026-09-17: the left header label is now `Files`; it takes the shared
hover background and a left-click opens the three pane settings. The passive
filter words move before the diff totals without a dot. Repository and
worktree identity move to the global menu bar. Right-click on the header does
nothing, and row context menus contain only actions on the pointed item.

Amended later 2026-09-17: the title menu uses stable filter labels with
checkmarks for active settings instead of changing its action wording. Files
and directories also offer relative and full-path copy actions; the file
action is **File comment**.

Amended later 2026-09-17: the title menu unfolds from the pane chrome rather
than from the pointer. Its left edge aligns with the sidebar and its top
border occupies the row immediately below the Files header.

Amended later 2026-09-17: current-file identity moves from the centered
global menu bar into a separate File surface header. The Files pane continues
to own repository filters and change totals; the File header owns the current
path, current-file lifecycle counts, and document display settings.

Amended 2026-09-18: **Only reviews** is a fourth session filter at
`Space F o`. It admits paths with active or resolution-proposed, non-archived
threads visible in the current workspace, intersects the other filters and
selected-comparison scope, and refreshes as thread lifecycle, reach, or local
rename projection changes.

Amended later 2026-09-18: the header represents active filters only as the
dim comma-list `c`, `r`, `u`, and `i`, where `u` means untracked files are
hidden. The clickable **Files** title and the key chords carry the full labels.

Amended later 2026-09-18: direct comparison and open-thread navigation may ask
Files to reveal and center a destination, including while the pane is hidden.
The four filters remain authoritative: an excluded destination creates no row
and leaves the highlight unchanged until a later listing admits it.

Amended 2026-09-19: `Space F` contains File-list controls. Its `c/o/u/i`
suffixes mean only changed, only reviews, hide untracked, and show ignored;
`Z` folds or unfolds all directories without moving focus. The same keys work
bare while Files has focus. File-opening workflows live under lowercase
`Space f`: `f` opens the ordinary picker, `i` includes ignored paths, and
`r` opens recent files.

Amended 2026-09-20: `Z` is a persistent auto-unfold toggle, also available as
a checked **auto-unfold** row in the File-list title menu. While active it
keeps newly added or newly admitted directories recursively unfolded and shows
exactly `Z` as the list's status marker. A second `Z` folds everything and
exits the mode. A manual keyboard or mouse fold/unfold exits the mode; ordinary
navigation and selection do not.

## Context

The files pane lists every non-ignored file under the workspace, with a
letter and `+n -m` counts on the ones that differ from `HEAD`
([0017](0017-git-status-navigation.md)). In a repository of any size the
changed files are a handful of rows among hundreds, and the user asked on
2026-09-06 for the pane to filter: only the files with outstanding
changes, untracked files or not, ignored files or not.

[0056](0056-the-leader-trimmed.md) removed the pane's `I` key and its
"toggle ignored" menu entry because the picker at `Space F i` finds
ignored files. Filtering what the pane lists is a different need from
finding one file, so the ignored toggle returns here with the two
others, as a rule about what the pane shows.

The question round settled where the keys live. Two paradigms were
weighed: a strip of key hints in the status line, so every pane's keys
have one home of a fixed width, or leader chords. The status line is
already full at the user's half-width terminal (path, badge, counts,
waiting count, change hint), and [0067](0067-the-texts-key-bar.md) had
just settled that keys live on a bar along the bottom of the pane they
act in. These toggles change what the pane shows, not what its cursor
does, which is the shape of `Space p f`, `Space v t`, and `Space d w`:
a global toggle under the leader, documented by the which-key popup,
working from any pane. So: leader chords under the existing `Space F`
files submenu, no bar on the files pane, and the pane's header names the
state.

The user also asked for the popup's entries to say what a press does
now, `show ignored` or `hide ignored`, not `toggle ignored`, and for
the files pane's header row to take the `ui.header` surface the threads
pane's has.

## Decision

### Filters and auto-unfold

```
Space F c    only changed      /  all files
Space F o    only reviews      /  all files
Space F u    hide untracked    /  show untracked
Space F i    show ignored      /  hide ignored
Space F Z    auto-unfold       /  fold all

Space f f    open file
Space f i    open incl. ignored
Space f r    recent files
```

- **Only changed** lists the files that differ from `HEAD` as the
  status walk of 0017 reports them, `M`, `A`, `D`, and `U`, so untracked
  files count as changed. Directories with nothing to show are not
  drawn, expanded or not. A file that becomes clean leaves the pane on
  the next status; one that changes appears.
- **Only reviews** lists files with at least one active or
  resolution-proposed thread in the current workspace. Resolved and archived
  threads do not qualify. The app projects viewer-local working-tree renames
  before supplying paths to the annotation-agnostic tree.
- **Hide untracked** drops the `U` files. Under *only changed* it leaves
  the tracked changes; on its own it leaves everything git knows.
- Deleted files and their missing parent directories remain listed under
  these rules, as in [0017](0017-git-status-navigation.md) (amended
  2026-09-14); every new status updates them even with no filter enabled.
- **Show ignored** lists what `.gitignore` hides, as the picker at
  `Space f i` finds it; `.git` itself stays hidden. It changes what the
  tree reads, so the pane re-reads its listings when it flips. An
  ignored file is never a changed one, so *only changed* wins when both
  are on.
- All four filters are **session toggles** starting off, with no config block.
  `Space F c/o/u/i` works from any pane, and bare `c/o/u/i` mirrors it while
  Files has focus. Toggling while Files is hidden changes the pane all the
  same and a status-line notice names the new state.
- `Space F Z` and direct `Z` in File list toggle the same session mode without
  transferring focus. Activation recursively unfolds every admitted directory
  and keeps directories that appear or become admitted later unfolded. A
  second press folds every directory and exits the mode.
- A manual fold or unfold by `z`, `h`, `l`, Enter, a directory click, or its
  row menu exits auto-unfold. Navigation and file selection leave it active.
- Rules compose by intersection. A review-bearing file must also satisfy
  *only changed*, untracked, ignored, and selected-comparison snapshot rules
  that are active. Directories appear only when they lead to an admitted file.
  Store reloads, thread creation, lifecycle and archive changes, deletion,
  local rename projection, and comparison or reach changes update the listing
  immediately.
- The **open file may drop out** of the pane. Nothing pins it; the pane
  highlights it again when it qualifies.
- `]f`, `]g`, `]G`, the picker, `follow`, and every other walk of the
  workspace are unchanged: the filter is the pane's view, not the
  workspace's.

### Entries say what a press does

- A which-key entry may carry a **live label**: the binding table's
  label is the static form (`only changed`, `only reviews`, `hide untracked`,
  `show ignored`, what `Space ?` lists), and while a restrictive toggle is on
  the popup reads `all files`; the others read `show untracked` or
  `hide ignored`. The label states
  the outcome of pressing the key now, in the fewest words.
- A left-click on the header's **`Files` title** opens a pane settings menu
  with stable `only changed`, `only reviews`, `hide untracked`, and
  `show ignored` labels, plus a checked `auto-unfold` mode.
  Each active setting carries a checkmark; inactive settings reserve the
  same space without one. The menu aligns to the sidebar's left edge and
  begins on the row below the header instead of at the pointer. The title is
  the only clickable part of the Files header and takes `ui.list.hover` under
  the pointer; right-click on the header does nothing.
- A file row's **right-click menu** is item-local: `open`, `file comment`,
  `copy path` (`y`), and `copy full path` (`Y`). A directory row offers
  `expand` or `collapse`, then both path-copy actions. Save review point
  remains in the global **Diff** menu, and review navigation and pane filters
  do not appear on row menus.

### The header names the state

- The files pane's header row is a `Header` on **`ui.header`**, as the
  threads pane's is (0066): `Files` is bold in the directory colour at the
  left. Against the right edge, active filters appear in the same dim colour
  as the former words, compacted into the comma-list `c,r,u,i`: changed-only,
  reviews-only, untracked hidden, and ignored shown. The comparison's `+n -m`
  totals (0017) follow the marker, separated by a space.
- While auto-unfold is active, the status marker is exactly `Z`, replacing the
  filter comma-list until the mode ends.
- `Files                                  c,u +12 -3`. The compact marker is
  retained while it fits; as before, it drops before either diff total.
  Repository and active-worktree identity live in the global menu bar under
  [0081](0081-the-menu-bar.md). Current-file identity lives in the File
  surface header.

### Amendments

- 0056's *Fewer entries*: the ignored toggle is `Space F i`, and all four
  settings also have direct pane keys. Pickers move to lowercase `Space f`.
- 0056's map: `Space F` is `c` / `o` / `u` / `i` / `Z`; `Space f` is
  `f` / `i` / `r`.
- 0050's Files title opens the four pane filters and auto-unfold. Row context
  menus stay item-local, and right-click on the header remains inert.
- 0017's tree bullet: the pane may list a subset; the letters, the
  counts, and the root totals are unchanged.
- 0087's comparison model (amended 2026-09-17): with a non-working target,
  "all files" means the target snapshot plus source-only comparison deletions,
  not the current checkout. The visible file picker follows the same boundary.
  The four rules remain filters over that source.

## Consequences

- `app/files_shown.rs`, which this record backs, holds the filters and
  auto-unfold state labels, the live labels, the notice while the pane is
  hidden, and the header marker.
- `fathomable_core::tree` gains `Shown`, what the tree lists: four
  rules with `Shown::all()` as the start, toggled by `Rule`. `Tree`
  keeps a `Shown`, caller-supplied review paths, and the paths it admits under
  the current `Status`;
  `Tree::set_shown` re-reads the listings when the ignored rule flips
  and `Tree::sift` re-applies the rules to a new status. The rows are
  filtered as they are built, so the cursor, clicks, and `reveal` see
  only listed rows.
- `App::take_status` sifts the tree after a status lands.
- `bindings::menu_entries` and `bindings::menu` take a relabel
  function; `App::which_key` supplies the live labels, and the drawing
  and the mouse go through it. The binding table gains `FilesChanged`,
  `FilesReviews`, `FilesUntracked`, `FilesIgnored`, and `FilesAutoUnfold`
  under `Space F`.
- `draw::tree_lines` builds the header through `Header`; `Tone` gains
  the diff colours for the counts. Header layout retains the totals while
  dropping the compact filter marker first.
- The guide's key table, its files pane passage, and its mouse passage name
  the four filters and auto-unfold.
