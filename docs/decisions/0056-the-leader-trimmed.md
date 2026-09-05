---
type: Decision
title: The leader, trimmed
description: The Space menu loses the tree actions and the change-queue clear, gains Helix's window submenu on Space w and a Space Space pane cycle, keeps the pane hides under Space p f and Space p t, moves the review list to Space r, the file picker's extras under Space F, wake under Space a, and the new-thread entry to Space c c; every label is a few words; the tree pane is the files pane; and the menu draws on its own ui.menu surface.
resource: crates/fathomable/src/app/window.rs
related_resources:
  - crates/fathomable/src/app/input/bindings.rs
  - crates/fathomable/src/app/input/menu.rs
  - crates/fathomable/src/app/files_pane.rs
  - crates/fathomable-core/src/theme.rs
  - crates/fathomable-core/themes/default-dark.kdl
  - crates/fathomable-core/themes/default-light.kdl
tags:
  - decision
  - input
  - rendering
---

# 0056 The leader, trimmed

Status: accepted (2026-09-04); amended 2026-09-05 by
[0057](0057-the-sidebar.md): the rail is the sidebar, and `Space w f`
and `Space w t` join the window submenu, naming the files pane and the
threads pane.

## Context

[0049](0049-inline-threads-and-the-rail.md) laid out the `Space` menu
with submenus for the rail, threads, view, jump, and, a day later,
panes. Reading it back on 2026-09-04 the user found entries that earn
nothing: `Space r` held three tree actions (re-read, toggle ignored,
reveal this file) that the watcher, the picker, and the focus toggle
already cover; `Space c n` did what `C` does; `Space j c` cleared a
change queue that clears itself; and the labels ran to sentences
("tree pane: focus, or return", "threads: new thread on the cursor
line") in a box whose breadcrumb already says where you are. The
pane keys were arbitrary letters: `e` for the tree pane, `T` for the
threads pane, and the hides on `p e` / `p t`, with the review list on
`t` as of that morning.

The user asked for fewer entries, terse labels, Helix's window
language for moving between panes, and a menu surface that is not the
status line's grey. Two question rounds the same day settled the
points below; each took the recommended answer unless noted.

## Decision

### The map

```
Space f           open file
Space F i / r     files: open file incl. ignored / recent files
Space r           review list
Space w h/j/k/l   window: left / down / up / right
Space w w         window: next
Space Space       next pane
Space p f / t     panes: toggle files pane / toggle threads pane
Space c c/r/o/e/d threads: new thread, reply, resolve, edit, delete
Space c x         threads: toggle resolved stubs
Space v s/d/D     view: source, diff vs HEAD, diff vs last seen
Space v c/C/r/g   view: checkpoint file / workspace, checkpoint diff, diff vs commit
Space j j/a       jump: newest change, auto-jump
Space a w         agent: wake
Space ?           all keys
```

`Space e`, `Space o`, `Space t`, `Space T`, `Space r r`, `Space r i`,
`Space r .`, `Space c n`, `Space j c`, and `Space w` as wake are
unbound.

### Windows, not letters

- `Space w` is Helix's window submenu, so the keys are spatial and
  nothing has to be memorised: `h` focuses the pane to the left of
  the text (the files pane, or the threads pane when the files pane
  is hidden; the files pane is shown if neither is), `l` returns to
  the text, `j` and `k` step between the files pane and the threads
  pane when both are shown, `w` cycles text, files pane, threads
  pane, skipping hidden panes. `Space Space` is the same cycle at
  two keys. "The text" is the review list while it is open.
- A move with nowhere to go does nothing, quietly. `Esc` in a pane
  and `h` at column 0 keep doing what they did.
- Hides stay under `Space p`, "panes": `Space p f` toggles the files
  pane, `Space p t` the threads pane, each shown or hidden without
  taking the keys. The user chose `f` over `T`; no shifted letter
  for a pane.
- Wake moves to `Space a w` under an "agent" submenu, the place for
  agent commands until one earns a top-level key.

### Fewer entries

- The tree actions go: re-read (the watcher shows new, deleted, and
  renamed files on its own), toggle ignored (the picker at `Space F
  i` finds ignored files), reveal this file (focusing the files pane
  reveals it). The pane's `R` and `I` keys and the right-click
  menu's "re-read the tree" and "toggle ignored" entries go with
  them. `Tree::refresh` stays for the watcher.
- `Space j c` goes. A change leaves the queue when its target is on
  screen; nothing needs to forget one unseen.
- `Space c c` is the one "start a thread here" entry, what `C` does
  and what `Space c n` did; `c` under `c` mirrors the bare key.
- `Space r` is the review list, which was on `Space t` for a day.
  `Space F` is a "files" submenu with the two rarer pickers, `i`
  including ignored files and `r` the recent files.

### Labels

Every label under `Space` is a few words. The breadcrumb names the
submenu, so an entry says the thing and no more: "review list", "new
thread", "toggle files pane", "left", "wake". Nothing says "here",
"on the cursor line", "or return", or "or close"; that the action is
on the cursor and toggles is implicit, as it is on the bare keys.

### The files pane

The tree pane is the **files pane**: in the guide, the labels, the
status pill (`FILES`), and the code's comments. Earlier records keep
the old name as history. The code's identifiers (`Focus::Tree`,
`rail.tree`) stay; the rail keeps its name for now, the user not yet
settled on another.

### The menu surface

The `Space` menu and the right-click menu draw on a new theme key,
`ui.menu`, as Helix's do; pickers, the help, and the status popup stay
on `ui.popup`. The built-in themes give it a cool slate ground
(`#222a36` dark, `#e3e9f2` light) that reads as an overlay against
the grey status line and lifts the yellow keys. A terminal cell has no
alpha, so translucency is not a thing the viewer can draw; a theme that
wants the terminal's own transparency behind the menu sets `ui.menu`
with no `bg`, and the cleared menu area shows the terminal through.

## Consequences

- `app/window.rs`, which this record backs, holds the focus moves:
  left, right, up, down, and next, over the panes that are shown and
  the review list when it is open.
- The binding table drops `TreeRefresh`, `TreeIgnored`, `TreeReveal`,
  and `ClearChanges`; `App::refresh_tree`, `toggle_ignored`,
  `reveal_in_tree`, and `clear_queue` go with them. `TreeToggleFocus`
  and `ThreadsPaneFocus` are no longer bound; the tree's `Esc` and the
  text's `h` at column 0 keep their paths.
- `SUBMENUS` names `F` files, `w` window, `p` panes, `c` threads, `v`
  view, `j` jump, `a` agent.
- `Key::UiMenu` joins the theme vocabulary; [0011](0011-theme-schema.md)'s
  table is amended.
- The welcome screen names `Space f`, `Space w h`, `Space r`, and
  `Space ?`.
- `docs/guide.md`'s key tables, its rail paragraph, and its wake
  paragraph are rewritten in the same change; the binding test that
  checks the guide's keys against the table holds.
