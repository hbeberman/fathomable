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
  it (`menu`), which is what the `Space` menu, the `Space j` submenu, and
  the old `g` hint list now are. A pane header asks `hint(place, action)`
  how a key is spelled, so a hint cannot name a key that is not bound
  there. The status line shows the prefix as the table spells it.
  (Amended 2026-09-04 by [0049](0049-inline-threads-and-the-rail.md): the which-key
  menu leads with a row naming the prefix and its group word, `Space c
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
