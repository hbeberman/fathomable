---
type: Decision
title: Bindings are data
description: Every key binding is one row of a table that dispatch, the Space and prefix menus, the help popup, the pane hint bars, and the guide's key section are all read from or checked against, so a key cannot exist unlisted or be listed without existing.
resource: crates/fathomable/src/app/input/bindings.rs
tags:
  - decision
  - input
  - documentation
---

# 0045 Bindings are data

Status: accepted (2026-09-03)

Amended later 2026-09-19: `Space d n/u/o` selects Normal, Unified, or Off,
and `Space d s/t` opens the Source or Target endpoint picker. `Space d b` and
the `"standard"` mode value are retired without aliases.

Amended 2026-09-19: the `Space d` labels use direct workflow language:
**normal diff**, **unified diff**, **diff off**, **pick source…**, **pick
target…**, **show uncommitted changes**, **show latest commit**, **show a
specific commit…**, **save review point**, **manage review points…**, and
**ignore whitespace**. The binding descriptions retain their `diff: ` prefix
so the derived submenu breadcrumb removes it in one place.

Amended 2026-09-18: `Alt-Space` is a binding-table action that opens and
focuses the top-left Fathomable menu. Dispatch recognizes this one global
accelerator before popup and text-input precedence so it is as reachable as
the menu bar's mouse target.

Amended 2026-09-18 by
[0090](0090-direct-workspace-navigation.md): shifted arrows and
`H`/`J`/`K`/`L` traverse comparison changes and changed files, while
`Tab`/`Shift-Tab` traverse open threads from every normal pane. The
bracket-prefixed comparison and thread traversal sequences retire without
aliases; event conversion retains shifted arrows and normalizes shifted
`Tab` to `Shift-Tab`.

Amended 2026-09-04 by [0050](0050-mouse-menus-and-gestures.md): the which-key menu and `Space ?` take clicks and hover, pane-header hints carry their action and take clicks, and `gx`, `gy`, and the tree's `y` are new rows.

Amended 2026-09-14 by
[0078](0078-all-keys-stays-reachable.md): `Space ?` still derives every
row from this table, but groups the rows into one or two responsive
lanes and makes the complete result scrollable and filterable.

Amended 2026-09-14: the pending-prefix helper anchors to the whole
viewer's bottom-right corner, above the status line, independent of
focus. Text inside remains left-aligned. The text's `g` menu is now
`g goto top`, `e goto bottom`, `l goto line end`, `h goto line start`,
and `f open linked file/URL`. The line motions cross wraps as
[0010](0010-viewer-ux.md) describes; [0052](0052-goto-file.md) unifies
opening. `gy`/`gx` are removed; `gs`/`gd`/`gD` are removed in favor
of `Space v s`/`Space d d`/`Space d D`, which already work from every
pane. Pane cycling is `Space w w`.

Amended 2026-09-14: the explicit `Space Space` binding and menu entry
are removed. A second Space is simply an unmatched continuation, so the
existing miss behavior still clears the pending chord without running
the broader `Esc` action.

Amended 2026-09-17: picking the comparison base uses `Space d b`; the
text-focused `b` alias is removed. `Space d s` no longer starts a comparison
at current `HEAD`; that action remains in **Comparison controls...**.
`Space d r` and the redundant temporal-focus model are removed; a review
point can instead be selected directly as the comparison base.

Amended 2026-09-18 by
[0087](0087-global-comparisons-and-board-history.md): the preceding
Comparison controls retention is superseded. The popup and Start at HEAD
action are removed; `Space d n/u/o` selects Normal, Unified, or Off.
`:diff` has no compatibility alias.

Amended later 2026-09-18: `Space d d` is restored with one direct meaning:
pin the current `HEAD` as Source and select the working tree as Target. It does
not toggle presentation or reopen the removed comparison controls.

Amended 2026-09-19: lowercase leaders name workflows and uppercase leaders
name sidebar panes. `Space f f/i/r` opens ordinary, ignored-inclusive, and
recent file pickers; `Space F c/o/u/i` changes File-list settings and
`Space F Z` folds or unfolds all File-list directories without moving focus.
Those same suffixes work bare while File list has focus. Thread workflows move
from `Space c` to `Space t`; `Space T s/x` changes Thread-list scope and
resolved visibility, while `Space T Z` folds or unfolds every file group
without moving focus. Mixed leader menus draw section rules from binding
metadata. `Space d l/c/p` means HEAD-parent comparison, picked commit-parent
comparison, and save-and-select review point.

Amended later 2026-09-19: pending-prefix helpers consume those semantic
sections rather than separator-shaped rows. Their card reflows the ordered
entries column-major for the current terminal, prefers a compact eight-row
body, and grows vertically when narrow space makes that more legible. A
section stays together when practical and otherwise continues in the next
column without a false boundary. Column dividers and full-width section rules
join as one border. Labels shorten only after comfortable layouts fail; keys
remain whole. The card never scrolls, and an impossibly small viewport gets an
explicit size warning rather than clipped or omitted actions.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

The key map lived in three hand-written places: a 530-line `keys.rs`
of `match` arms per focus, a 45-row `HELP` constant for `Space ?`, and
the hint strings each pane header composed. They drifted. The review of
2026-09-03 found the help popup listing `n`/`p` in the thread pane,
which no longer existed, and missing `h l Tab e d Left PgUp PgDn`; the
list row omitted six keys; `d d`, `:follow on|off`, `0 $ Home End`, and
the box's `Ctrl-c` were absent everywhere but the code. The guide's §3
table was a fourth copy, checked by nobody. The `Space` menu and the
`Space j` submenu were two more constants, and the `g` prefix drew a
menu from a fifth list in `ui.rs`.

[0007](0007-key-grammar-and-mouse.md) asked for "hints so the keymap is
learnable without a manual" and parked remapping through KDL. Neither
is possible while the map is prose.

## Decision

- **One table.** `app/input/bindings.rs` holds `BINDINGS`: one
  `Binding { keys, place, action, label, group }` per thing a key does.
  `keys` is one or more sequences of `Chord`s (a key plus Ctrl/Alt), so
  `j` and `Down`, or `gg`, `]c`, and `Space j a`, are rows, not code.
  `place` is a `Where`: the text, the tree, the thread pane, the
  file-threads pane, the thread list, the comment box, the picker, the
  command line, or `Any` — every pane, never a popup. `action` is an
  `Action`; one action may be bound on several surfaces and `App::act`
  gives it that surface's meaning (`MoveDown` moves a row in the text,
  a message in the thread pane, an entry in the picker).
- **Dispatch reads the table.** `keys.rs` turns a terminal event into a
  `Chord`, appends it to the keys typed so far (`App::prefix`), and asks
  the table: an exact match runs the action and clears the prefix, a
  prefix waits, a miss is dropped whole. The `g`, `[`, `]`, `Space`,
  `Space j`, and `d` prefixes are therefore one mechanism; `App.pending`
  and `View.pending` are gone. The first `d` of `dd` still arms the
  thread id ([0034](0034-deleting-threads.md)): a miss after it cancels
  with the same notice, and a click cancels both the prefix and the
  arm. Only text entry falls through: an unbound character is inserted
  in the box, filters the picker, or extends the command line.
- **Everything that shows a key reads the table.** `Space ?` renders
  `help()`, grouped by `group`, in as many columns as the popup needs.
  A pending prefix draws a which-key menu of the sequences that continue
  it (`menu_sections`), which is what the `Space` menu, the `Space j`
  submenu, and the old `g` hint list now are. The card computes columns from
  the current terminal instead of binding data naming a side. A pane header
  asks `hint(place, action)` how a key is spelled, so a hint cannot name a key
  that is not bound there. The status line shows the prefix as the table
  spells it.
  (Amended 2026-09-04 by [0049](0049-inline-threads-and-the-rail.md): the which-key
  menu leads with a row naming the prefix and its group word, `Space t
  · threads`, and re-renders at every level. Amended 2026-09-14 by
  [0078](0078-all-keys-stays-reachable.md): help reads `BINDINGS`
  directly, wraps into one or two lanes, and scrolls instead of creating
  clipped columns.)
- **Tests make it load-bearing.** Every `Action` is bound at least once;
  on each surface no sequence has two meanings and none is the start of
  another; no binding uses a zellij lock chord (`Ctrl-g p t n h s o q
  b`); and every backticked key in the guide's §3 table is a bound
  sequence or the start of one. The guide stays hand-written prose,
  gate-checked as [0043](0043-agent-vocabulary.md) checks §8.
- **Two keys change with it.** The picker moves on `Ctrl-j`/`Ctrl-k`,
  because `Ctrl-n`/`Ctrl-p` are zellij locks the test forbids (0021 had
  removed `Ctrl-j`/`Ctrl-k` as duplicates; they are now the only pair).
  `:` opens the command line from the thread pane too, as `Any` binds it.
- **Spelling.** A sequence of bare characters runs together (`gg`,
  `]c`, `dd`); anything with a modifier, a named key, or the space bar
  is space-separated (`Ctrl-d`, `Space j a`, `Alt-Enter`).

## Consequences

- Adding a key is one table row; forgetting the help, the hints, or the
  guide is no longer possible, and a stale guide fails `cargo nextest`.
- The `Space` menu, the `Space j` submenu, and the `HELP` constant are
  gone as data; `Popup::Space` and `Popup::Jump` are gone as state, the
  prefix being the state.
- The mouse stays hand-written in `app/input/mouse.rs`: it goes to the
  pane under the pointer, which is not a key.
- Remapping through KDL ([parked](../parked.md)) is now an overlay on
  this table rather than a rewrite of `keys.rs`.
- [0007](0007-key-grammar-and-mouse.md) keeps the grammar and takes
  `app/input/keys.rs` as its resource; the per-focus `match` it
  described is superseded by this record.
