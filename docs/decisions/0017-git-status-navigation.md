---
type: Decision
title: Git status as the primary change layer
description: The gutter, hunk keys, and sidebar marks mean uncommitted git state, staged and unstaged told apart; last-seen recency moves to its own signals.
resource: crates/fathomable-core/src/status.rs
tags:
  - decision
  - git
  - input
---

# 0017 Git status as the primary change layer

Status: accepted (2026-08-26)

## Context

[0006](0006-git-access.md) gave the gutter and `]c`/`[c` a diff base, and
[0015](0015-follow-mode.md) made the **last-seen** snapshot the default
base, with `HEAD` second. In use that inverted the tool's purpose: the
reader wants to move through everything uncommitted in the working tree,
in the file and from file to file, and last-seen answered a different
question ("what landed while I was away") that the follow queue, sidebar
badge, and toasts already answer. The two bases also disagreed silently: a
file dirty against `HEAD` could show no hunks because the reader had
already watched the change arrive.

Nothing in the app knew the set of dirty files. `Workspace::head_text` was
a per-file lookup; the follow queue held only files that changed while
Fathomable was running. Settled with the user on 2026-08-26: the gutter
always means git state; staged and unstaged are marked distinctly;
untracked, non-ignored files are in the set; hunk keys cross files; key
letters follow Helix; the sidebar shows a git letter and line counts.

## Decision

### Dirty set

- `Workspace::status()` walks the repository with `gix`'s status support
  (worktree against index, index against `HEAD`) and returns one
  `GitEntry { path, state, staged }` per dirty path, sorted by path.
  `state` is `Modified`, `Added`, `Deleted`, or `Untracked`; `staged` is
  whether the index differs from `HEAD` for that path. A path with both
  staged and unstaged changes is one entry with `staged: true` and the
  worktree's state. Ignored paths are never in the set; untracked files
  are, so a new file appears the moment it is written.
- The set is refreshed on the same `.git` events that refresh `HEAD`
  bases in 0015 (`HEAD`, `ORIG_HEAD`, the index) and on every workspace
  file event after the hint debounce, so a save, a `git add`, and a commit
  each update the sidebar within a beat. Outside a repository the set is
  empty and every git feature below is inert.
- The walk is in-house over `gix`'s `index` feature rather than its
  `status` feature, which would pull `blob-diff` and its rename machinery
  that [0006](0006-git-access.md) declined: the index against the `HEAD`
  tree for staged changes, a stat-then-hash pass over index entries for
  unstaged ones (a size and mtime match is clean, as in git), and the
  tree's ignore-aware file walk for untracked paths. It runs on the event
  loop after the hint debounce, so a burst of writes costs one walk;
  moving it to a thread is a follow-up if large trees make it felt.
- Per-file line counts (`+a -r`) are worktree against `HEAD`, computed in
  the same walk for each dirty path.

### Gutter and hunks

- The gutter bar and hunk navigation compare the working tree with `HEAD`
  only. The `BaseKind` toggle of 0015 is gone from the gutter. A
  document keeps three texts, worktree, index, and `HEAD`; the gutter
  draws a line's mark from the worktree-vs-`HEAD` diff, coloured as in
  0006, and the bar glyph tells staging apart: `▎` when the line's hunk is
  not yet in the index, `▌` when the index already holds it. A hunk that
  is staged and then edited again shows `▎`, since that is what the reader
  would act on. Untracked files mark every line `▎` as added.
- `]g` / `[g` move to the next / previous hunk **across files**: from the
  last hunk of a file, `]g` opens the next dirty file in path order at its
  first hunk, wrapping from the last file to the first with a status
  message, as `]c` wrapped within a file. `[g` mirrors it. Helix uses
  `]g` for "next change"; the vimdiff `]c` is retired.
- `]G` / `[G` move by file: the next / previous entry of the dirty set in
  path order, opened at its first hunk.
- `]c` / `[c` become the annotation-thread keys (`c` is the comment key),
  replacing `]a` / `[a`.
- `gd` / `:diff` cycles rendered, `DIFF` (worktree against `HEAD`, the
  margin showing `▎`/`▌` per hunk), `DIFF seen` (against the last-seen
  snapshot, when one exists and differs), then rendered. The pill reads
  `DIFF` and `DIFF seen`; there is no `DIFF head`.

### Last-seen

- Last-seen snapshots, the follow queue, the sidebar `●`, the status hint,
  toasts, `]f` / `[f`, `Space j`, and auto-jump are unchanged from 0015.
  A follow-queue target is the first hunk against **`HEAD`** now, so a
  jump lands where the git gutter says something is; outside git, where
  there is no `HEAD`, it falls back to the last-seen snapshot. The
  last-seen diff remains reachable through `gd` and is the input to the
  parked diff-view animation.

### Sidebar

- A dirty file's tree row shows, after its name and one space, a state
  letter and its line counts: `mod.rs M +12 -3`, `new.rs ? +40`,
  `old.rs D -18`. Letters are `M`, `A`, `D`, `?`, in `git.unstaged` or,
  when the path is staged, `git.staged`; `+a` in `diff.plus` and `-r` in
  `diff.minus`, each omitted when zero. Nothing is padded to a column;
  colour tells the parts apart.
- A collapsed directory shows the letter of its most advanced descendant
  (`?` > `A` > `D` > `M`) and the summed counts, so a dirty tree is visible
  however it is folded. The follow `●` of 0015 sits after the git mark
  when both apply.
- Deleted files are not tree entries (nothing is on disk); they appear in
  `]G` order and in the diff view as an all-removed file, and the parent
  directory's summary counts them.

### Theme

Two keys join the schema of [0011](0011-theme-schema.md): `git.unstaged`
(default `diff.delta`'s colour) and `git.staged` (default a dimmer or
cooler variant of it, chosen per bundled theme). Unknown keys stay errors.

## Consequences

- `gix` gains the `index` feature only; the dependency policy of 0001
  already approves the crate. A status walk over a large tree is bounded
  by the index's stat cache, and the walk reports its duration at debug
  level so `--log` can show when it is worth threading.
- `View::base_kind` and `BaseKind` go; `View` holds an index text beside
  `head`. The unified diff layout gains a per-hunk staging glyph.
- The key table, guide, and `--doctor` (which reports the dirty and
  staged counts) change. `]a` / `[a` and `]c`-as-hunk are removed rather
  than aliased.
- Side-by-side diff stays parked. Staging from within Fathomable is out of
  scope: it is a viewer.
