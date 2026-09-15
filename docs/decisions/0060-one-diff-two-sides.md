---
type: Decision
title: One diff, two sides
description: All comparisons share one two-sided diff view; Git exposes HEAD, index, and worktree endpoints with staged, unstaged, and net badges alongside snapshots, checkpoints, and commit sides.
resource: crates/fathomable/src/app/diff.rs
related_resources:
  - crates/fathomable/src/app/checkpoints.rs
  - crates/fathomable/src/app/view.rs
  - crates/fathomable/src/app/input/bindings.rs
  - crates/fathomable/src/app/draw/header.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable-core/src/diff.rs
  - crates/fathomable-core/src/config.rs
tags:
  - decision
  - git
  - checkpoints
  - input
  - configuration
---

# 0060 One diff, two sides

Status: accepted (2026-09-05); amended 2026-09-14 (index side and
layer-labelled diffs)

## Context

[0049](0049-inline-threads-and-the-rail.md) put seven entries under
`Space v`, five of them about comparing: the diff against `HEAD`, the
diff against last seen, the checkpoint diff, the diff against a commit,
and the two checkpoint marks. The user asked on 2026-09-05 for the
comparisons to have a menu of their own, and to settle how the app says
which diff is on screen and what a view menu is for once they leave.

Reading the code back, the three diffs were three displays. `gd` and
`gD` set `Display::Diff` and `Display::DiffSeen`, laid out against the
`head` and `seen` texts the view already holds for the gutter; the
checkpoint diff of 0049 set `Display::Checkpoint` and laid out a pair
of `Side`s that could already be any of a checkpoint, a commit, `HEAD`,
or the working file. Only the checkpoint diff had a header naming its
two sides and the `b` / `t` pickers; the other two had a badge, `DIFF`
or `DIFF seen`, and nothing else. `b` outside the checkpoint diff
answered with a notice. The checkpoint diff generalised the other two
and the code did not say so.

The question round settled: one diff view with two sides (over a key
move alone); the same letters under `Space d` that `Space v` had, so
`gd` and `gD` keep their mirror; a badge naming the base with the
header naming the pair; and, for `Space v`, the stub toggles. A wrap
toggle was raised and dropped: [0044](0044-wrap-all-lines.md) removed
sideways scrolling so that no row is ever cut, and a wrap-off mode
would have to bring that back or cut rows, either its own decision.

## Decision

### One diff view

- The view has one diff display, `Display::Diff`, backed by a **pair**:
  a base `Side` and a target `Side`. `Side` is what 0049's checkpoint
  diff had, plus `Seen`, the last-seen snapshot of
  [0015](0015-follow-mode.md), and `Index`, the staged snapshot.
  `Head`, `Index`, and `Seen` read the texts the view already holds.
  A missing index or worktree remains an empty diff endpoint even when
  the source display retains a tombstone from another endpoint.
- `gd` / `:diff` / `Space d d` shows the aggregate `HEAD · now` pair
  with badge `DIFF net`; `gD` / `:diff seen`
  / `Space d D` shows `last seen · now`; `Space d r` shows the newest
  checkpoint pair, `checkpoint 3/3  5m ago · now`, or the notice that
  there is no checkpoint. Each key **closes** the diff when the pair on
  screen is its own (`d` on `HEAD · now`, `D` on `last seen · now`, `r`
  on any pair whose base is a checkpoint) and opens its pair otherwise,
  so `gd` from the last-seen diff switches, as it did.
- Closing a diff returns to the file's **home** display: rendered for
  Markdown, source for anything else. Leaving the checkpoint diff used
  to land a source file on the rendered layout; it no longer does.
- `Esc` leaves the diff once it has nothing else to clear: the cascade
  of [0007](0007-key-grammar-and-mouse.md) is the input, then the
  selection, then the search highlight, then the diff. `gs` leaves it
  for the source view, as before.
- Every diff has the **header** 0049 gave the checkpoint diff: the
  pair's names (`HEAD · now`, `last seen · now`, `checkpoint 2/3  5m ago
  · now`, `a1b2c3d · HEAD`), then ` · whitespace ignored` while it is,
  then the hints `h/l page · b base · t target · w whitespace · Esc
  close`. (Amended 2026-09-06 by
  [0069](0069-the-diffs-keys-on-the-bar.md): the hints are on the
  text's key bar, `h/l page` on a checkpoint base only, and the header
  is the pair's names alone.) `b` and `t` open the side pickers from any diff, and their
  lists gain `last seen` when the file has a snapshot and always include
  `HEAD` and `INDEX` inside Git. `h` and `l` page
  the timeline when the base is a checkpoint and say so when it is not,
  as before. The **strip** of the file's checkpoints draws under the
  diff only while the file has one; a diff against `HEAD` of a file
  never checkpointed has a header and no strip.
- The gutter bar, `]g`, and the counts stay against `HEAD`, as 0049
  promised: checkpoints and snapshots sit beside git.

### The badge

- The status line's badge is one family: `DIFF net`, `DIFF staged`,
  `DIFF unstaged`, `DIFF seen`, `DIFF cp 2/3`, `DIFF a1b2c3d`.
  `HEAD -> INDEX` is staged, `INDEX -> WORKTREE` is unstaged, and
  `HEAD -> WORKTREE` is net. The header names
  the target, which is the working file nearly always. `CHECK` goes;
  `SRC` and `AUTO` stay. `:status` says `diff HEAD · now`.

### The diff menu

```
Space d d         diff vs HEAD             gd
Space d D         diff vs last seen        gD
Space d r         checkpoint diff
Space d g         diff vs commit…
Space d b / t     pick base… / pick target…
Space d c / C     checkpoint file / checkpoint workspace
Space d w         ignore whitespace
Space d s         mark all files seen      (0069)
Space v s         source view              gs
Space v t         toggle thread stubs
Space v x         toggle resolved stubs    (was Space c x)
```

- `Space d` is the **diff** submenu: what is compared, and the
  checkpoints the comparisons are made of. The letters are the ones
  `Space v` had, so `d` and `D` mirror `gd` and `gD` and nothing is
  relearned; `b` and `t` mirror the bare keys inside a diff and, from
  outside one, open it on the chosen side against the working file.
- `Space d w` **ignores whitespace**: lines that differ only in
  whitespace are equal, as `git diff -w`. It is a session toggle that
  starts from the config and relays out the open diff; the header
  says ` · whitespace ignored` while it is on. The gutter never
  ignores whitespace.
- `Space v` is the **view** submenu: how the text is drawn in every
  display. `s` is the source view; `t` toggles thread stubs, the
  runtime switch for `threads { stubs }` that 0049 left config-only;
  `x` toggles resolved stubs, which was `Space c x` and is not a thread
  action. `Space c` keeps the five actions on the thread at the cursor.

### The `diff` config block

```kdl
diff {
    context 3                // unchanged lines around each hunk
    ignore-whitespace #false // start with whitespace ignored
}
```

- `context` is the unchanged lines each diff shows around a hunk, three
  by default as `git diff`; it was a constant. `ignore-whitespace` is
  the starting state of `Space d w`. The block follows the pattern of
  `jump { auto }` and `Space j a`: the config gives the starting value,
  a key toggles it, the chrome shows the live state. There is no
  setting for the default base: `gd` meaning `HEAD` is a fact of the
  app, not a preference. The `checkpoints {}` block stays reserved
  (removed unused 2026-09-05; see
  [0049](0049-inline-threads-and-the-rail.md)).

### Amendments

- 0049's checkpoint section: `Space v r` / `c` / `C` / `g` are `Space
  d r` / `c` / `C` / `g`, and the checkpoint diff is the diff view with
  a checkpoint base; its leader map is superseded by 0056's, which this
  record amends.
- 0056's map: `Space v` is `s` / `t` / `x`, `Space d` is new, `Space c`
  loses `x`; `SUBMENUS` gains `d` diff.
- 0017's `gd` bullet: the badge reads `DIFF net`, and `DIFF seen` is
  the same family; 0010's badge list reads `SRC`, `DIFF <base>`, `AUTO`.
- 0050's checkpoint header click is the diff header click: a hint runs
  its key, the base name opens the base picker, the target name the
  target picker, in any diff.

## Consequences

- `app/diff.rs`, which this record backs, holds the diff view: `Side`,
  the pair, `show_diff`, the toggles for each pair, the side pickers,
  the timeline paging, the whitespace toggle, and the header text.
  `app/checkpoints.rs` keeps the two marks and the strip.
- `View` loses `Display::DiffSeen` and `Display::Checkpoint`;
  `Display::Diff` carries the pair's texts and labels. `diff_view()`
  answers for every diff; `diff_seen()` and `checkpoint_view()` go,
  replaced by the base the view names.
- `fathomable_core::diff::Diff::compare` takes a `Whitespace`, `Exact`
  or `Ignore`; `Diff::new` is `compare` with `Exact`. `Layout::diff`
  takes the context and the whitespace rule.
- `Config` gains `DiffConfig { context, ignore_whitespace }`, parsed
  from `diff { }` and printed by `--config-show`.
- The binding table renames `CheckpointDiff` to `DiffCheckpoint`,
  `CheckpointCommit` to `DiffCommit`, `CheckpointBase` and
  `CheckpointTarget` to `DiffBase` and `DiffTarget`; it gains
  `DiffWhitespace` and `StubsToggle`; `Space c x` moves to `Space v x`.
- The guide's key tables, its §5, and its config block are rewritten in
  the same change; the binding test that checks the guide's keys holds.
