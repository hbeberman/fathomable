---
type: Decision
title: The diff's keys on the bar
description: Comparison actions live on the text key bar while the local pair-name header has retired in favor of the File surface header and global endpoint controls.
resource: crates/fathomable/src/app/diff_keys.rs
related_resources:
  - crates/fathomable/src/app/diff.rs
  - crates/fathomable/src/app/draw/bar.rs
  - crates/fathomable/src/app/draw/header.rs
  - crates/fathomable/src/app/input/bindings.rs
  - crates/fathomable/src/app/input/mouse.rs
  - crates/fathomable/src/app/mod.rs
tags:
  - decision
  - input
  - rendering
  - git
---

# 0069 The diff's keys on the bar

Status: accepted (2026-09-06); amended 2026-09-14 (layer-aware cycle)

Superseded 2026-09-16 by
[0087](0087-global-comparisons-and-board-history.md). The diff key bar remains,
but `D`, last-seen, checkpoint paging, and mark-all-seen retire. `Space d`
now controls the single global pair, review points, and whitespace.

Presentation amended 2026-09-17: the pair-name header also retires. A diff
uses the File surface header, while the global menu-bar endpoint buttons
open the base and target pickers. The diff's applicable actions remain on
the text key bar.

## Context

[0067](0067-the-texts-key-bar.md) set one rule for keys: a header is
its words, and keys live on a bar along the bottom row of the pane they
act in. It named one exception. The diff header of
[0060](0060-one-diff-two-sides.md) kept `h/l page · b base · t target ·
w whitespace · Esc close` at its right edge, because the text's bar was
new and the thread cursor's keys would have pushed the diff's off the
user's 68-column text column.

Two things have changed since. The bar has been in use for a day and
holds its shape: it replaces the bottom text row only while it has
something to say, and hints drop from the end when the column is
narrow, so a long list costs nothing but its tail. And
[0064](0064-hints-you-can-press.md)'s rule, that a hint is drawn only
where pressing it now does what it says, was never applied to the diff
header: `h/l page` shows on every diff, and on a `HEAD` or last-seen
base the press is a notice.

The user asked on 2026-09-06 to finish the rule, and for two small
things beside it. A one-key walk through the diffs a file has, since
`gd`, `gD`, and `Space d r` each toggle one pair and switching between
them is a chord each. And a way to mark every file seen at once: the
last-seen snapshot of [0015](0015-follow-mode.md) is taken per file as
the reader looks at it, so after an agent's burst across a workspace
the only way to say "I have seen all of this" was to open each file.

The question round settled the order on the bar, the narrowed `h/l`,
the key and the shape of the cycle, and no menu entry for it.

## Decision

- **The diff header is its words.** ` HEAD · now`, ` last seen · now`,
  ` checkpoint 2/3  5m ago · now`, ` a1b2c3d · HEAD`, then
  ` · whitespace ignored` while it is. No hints. A click on the base
  name still opens the base picker and one on the target name the
  target picker ([0050](0050-mouse-menus-and-gestures.md)). The
  exception 0067 named is closed: a header is its words everywhere.
- **The diff's keys are on the text's bar.** A diff is something the
  bar has to say, so the bar replaces the bottom text row for every
  diff. While the text has the keys it reads, first, the diff's keys:
  `h/l page` on a checkpoint base only, then `b base`, `t target`,
  `D next diff`, `w whitespace`, `Esc close`. After them the thread
  cursor's keys and `Z` for the file as 0067 has them. The view's keys
  keep their place at the left edge while the cursor's come and go,
  and paging is the most pressed key in a checkpoint diff. On a
  68-column text column a checkpoint diff's bar drops `Esc close` and
  a `HEAD` or last-seen diff's fits whole; hints drop from the end as
  every bar's do. While another pane has the keys the bar reads
  `click or Space w l to focus`, over the diff as over the file.
- **`h/l page` only where it pages.** The hint is drawn while the base
  is a checkpoint; on any other base the press keeps its notice and the
  hint is not there, under 0064's rule.
- **`D` steps the diffs.** In the text, `D` shows the next pair along
  the file's row of diffs: the file itself, the unstaged
  `INDEX -> WORKTREE` layer, the staged `HEAD -> INDEX` layer, last
  seen against the worktree, the newest checkpoint against the
  worktree, then the file again. Each unavailable pair is skipped. The
  aggregate `HEAD -> WORKTREE` comparison is deliberately outside this
  cycle and remains explicit on `Space d d`, `gd`, and `:diff`. From a
  pair that is not on the row, a commit base, an explicit net diff, or
  a picked target, `D` returns to the file. With no pair to show, `D`
  says so. It is forward only. There is no `Space d` entry for it, by
  [0056](0056-the-leader-trimmed.md)'s rule that the menu carries no
  entry that duplicates a bare key; the bar and `Space ?` name it.
- **`Space d s` marks every file seen.** From any pane, it snapshots
  every non-ignored text file under the workspace as last seen, the way
  `Space d C` checkpoints them: a file whose content is already its
  snapshot is skipped, as are binary files and files over the viewer's
  limit or the store's. A toast counts them, `seen: 12 files`, or says
  `seen: nothing new`. Every open document's last-seen base is re-read,
  so an open `last seen · now` diff relays out to empty and `gD` from
  then on shows only what came after. Without a snapshot store the key
  says so. The idle, switch-away, and quit snapshots of 0015 are
  unchanged; this is the workspace-wide form of the same mark.
- **The whitespace hint stays `w whitespace`.** The header's words say
  `· whitespace ignored` beside it while it is on; a live label would
  say the same thing twice.

### Amendments

- 0067's *The diff header keeps its keys* bullet carries a dated note:
  the exception is closed here.
- 0060's header bullet: the hints are on the text's bar, not the
  header; its diff menu gains `Space d s`.
- 0064's diff header sentence: the diff's keys are on the bar, `h/l
  page` on a checkpoint base only.
- 0015's last-seen section: `Space d s` marks every file seen.
- 0050's checkpoint header bullet: the header takes name clicks only;
  its hints are on the bar, which takes clicks as 0067 has it.

## Consequences

- `app/diff_keys.rs`, which this record backs, holds the diff's hints
  for the bar, `diff_hints`, and `App::diff_next`, the `D` cycle.
- `app/last_seen.rs` holds the last-seen marks in the app: `mark_seen` and
  `on_quit` move there from `app/mod.rs`, and `mark_all_seen` joins
  them.
- `app/draw/bar.rs`: `text_bar` puts the diff's hints before the thread
  cursor's; `App::text_bar_shown` counts a diff as something to say.
- `app/draw/header.rs`: `diff_header` takes the text alone and builds
  no hints; the focus test moves to the bar's tests.
- `app/input/mouse.rs`: `diff_header_click` runs no hint; the name
  clicks stay.
- The binding table gains `DiffNext` on `D` in the text and `SeenAll`
  on `Space d s` everywhere; the submenu test and the guide's key
  tables follow.
- The guide's key tables, its mouse passage, and its §5 and §6 say
  where the diff's keys are, what `D` does, and what `Space d s` does.
