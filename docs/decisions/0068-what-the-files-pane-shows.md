---
type: Decision
title: What the files pane shows
description: The files pane gains three session toggles under `Space F`, only changed files, hide untracked files, and show ignored files, each entry naming what pressing it does now; the pane's header row moves onto `ui.header` and names the active filters after the repo's counts; the right-click menu carries the same three; a which-key entry may carry a live label.
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

### Three toggles

```
Space F c    only changed      /  all files
Space F u    hide untracked    /  show untracked
Space F g    show ignored      /  hide ignored
Space F i    open incl. ignored                      (unchanged)
Space F r    recent files                            (unchanged)
```

- **Only changed** lists the files that differ from `HEAD` as the
  status walk of 0017 reports them, `M`, `A`, `D`, and `?`, so untracked
  files count as changed. Directories with nothing to show are not
  drawn, expanded or not. A file that becomes clean leaves the pane on
  the next status; one that changes appears.
- **Hide untracked** drops the `?` files. Under *only changed* it leaves
  the tracked changes; on its own it leaves everything git knows.
- **Show ignored** lists what `.gitignore` hides, as the picker at
  `Space F i` finds it; `.git` itself stays hidden. It changes what the
  tree reads, so the pane re-reads its listings when it flips. An
  ignored file is never a changed one, so *only changed* wins when both
  are on.
- All three are **session toggles** starting off, with no config block.
  They work from any pane, since they change what the pane shows, not
  what its cursor does. Toggling while the files pane is hidden changes
  the pane all the same and a status-line notice names the new state.
- The **open file may drop out** of the pane. Nothing pins it; the pane
  highlights it again when it qualifies.
- `]f`, `]g`, `]G`, the picker, `follow`, and every other walk of the
  workspace are unchanged: the filter is the pane's view, not the
  workspace's.

### Entries say what a press does

- A which-key entry may carry a **live label**: the binding table's
  label is the static form (`only changed`, `hide untracked`, `show
  ignored`, what `Space ?` lists), and while the toggle is on the popup
  reads `all files`, `show untracked`, `hide ignored`. The label states
  the outcome of pressing the key now, in the fewest words.
- The **right-click menu** on a files pane row carries the same three
  entries with the same live wording, after `copy path`.

### The header names the state

- The files pane's header row is a `Header` on **`ui.header`**, as the
  threads pane's is (0066): the repo's directory name bold in the
  directory colour at the left, then against the right edge the repo's
  `+n -m` counts (0017) and the active filters, one word each, naming
  what is on screen: `changed` while only changed files are listed,
  `tracked` while untracked files are hidden, `ignored` while ignored
  files are shown. A ` · ` separates the counts from the first word.
- `demo +12 -3 · changed tracked`. Items drop from the end as the column
  narrows, as every header's do: the words first, then the counts.

### Amendments

- 0056's *Fewer entries*: the ignored toggle returns, as `Space F g`
  and a filter on the pane, not a key on it; the picker at `Space F i`
  stays.
- 0056's map: `Space F` is `c` / `u` / `g` / `i` / `r`.
- 0050's files pane menu gains the three toggles; the header row of the
  files pane takes no clicks.
- 0017's tree bullet: the pane may list a subset; the letters, the
  counts, and the root totals are unchanged.

## Consequences

- `app/files_shown.rs`, which this record backs, holds the three
  toggles' actions, the live labels, the notice while the pane is
  hidden, and the header's filter words.
- `fathomable_core::tree` gains `Shown`, what the tree lists: three
  rules with `Shown::all()` as the start, toggled by `Facet`. `Tree`
  keeps a `Shown` and the paths it admits under the current `Status`;
  `Tree::set_shown` re-reads the listings when the ignored rule flips
  and `Tree::sift` re-applies the rules to a new status. The rows are
  filtered as they are built, so the cursor, clicks, and `reveal` see
  only listed rows.
- `App::take_status` sifts the tree after a status lands.
- `bindings::menu_entries` and `bindings::menu` take a relabel
  function; `App::which_key` supplies the live labels, and the drawing
  and the mouse go through it. The binding table gains `FilesChanged`,
  `FilesUntracked`, and `FilesIgnored` under `Space F`.
- `draw::tree_lines` builds the header through `Header`; `Tone` gains
  the diff colours for the counts.
- The guide's key table, its files pane passage, and its mouse passage
  name the three toggles and the header's words.
