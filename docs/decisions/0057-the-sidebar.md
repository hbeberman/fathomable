---
type: Decision
title: The sidebar
description: The left column that holds the files pane above the threads pane is the sidebar, the word it had before 0049; the rail config node becomes sidebar, the ui.rail theme keys become ui.sidebar with no old spelling accepted, the column's state moves to its own module, the files pane's module is named for what it holds, and Space w f and Space w t take the keys to the files pane and the threads pane, showing a hidden one first.
resource: crates/fathomable/src/app/sidebar.rs
related_resources:
  - crates/fathomable/src/app/files_pane.rs
  - crates/fathomable/src/app/mod.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable-core/src/config.rs
  - crates/fathomable-core/src/theme.rs
  - crates/fathomable-core/themes/default-dark.kdl
  - crates/fathomable-core/themes/default-light.kdl
tags:
  - decision
  - configuration
  - rendering
  - documentation
---

# 0057 The sidebar

Status: accepted (2026-09-05)

Amended 2026-09-14 by [0079](0079-list-focus-language.md):
`ui.sidebar.selected` is retired in favour of shared `ui.list.*` roles,
with no compatibility alias; `ui.sidebar` and `ui.sidebar.dir` remain.

Amended 2026-09-15 by [0081](0081-the-menu-bar.md): the top-level
`sidebar { width; split }` block is replaced without an alias by
`layout { menu-bar; sidebar { visible; files; threads; width; split } }`.
Every launch uses that same startup composition. `Space p s` hides and
shows the sidebar as one remembered unit.

Amended 2026-09-18 by [0091](0091-pane-focus-navigation.md): the two
sidebar panes are **File list** and **Thread list**. Bare `F` and `T`
show and focus them; `w`/`W` cycle displayed panes. The `layout.sidebar`
configuration and `files` / `threads` child keys keep their established
spellings.

## Context

[0049](0049-inline-threads-and-the-rail.md) named the left column the
**rail** and ruled "sidebar" and "dock" out without a reason beyond
taste. Before that the column was the **sidebar**
([0012](0012-workspace-mode.md), [0023](0023-sidebar-paging.md), the
`ui.sidebar*` theme keys). Reading [0056](0056-the-leader-trimmed.md)
back on 2026-09-04 the user said twice that they were not convinced by
"rail", and asked for a brainstorm before choosing.

The word has to do four things: name a column that holds two panes
without colliding with "pane", "column" (the text), or "gutter"; work
as a config node and a theme key prefix, so one lowercase word; be a
word a Helix, Vim, or VS Code user already has; and be short enough for
a header row. The brainstorm weighed sidebar, side, dock, panel,
drawer, margin, aside, wing, explorer, strip, left, and no name at all.
Sidebar was the only candidate that met all four without a caveat.
Panel is heard as pane; dock and drawer promise movement the column
does not do; aside fights prose; the fresh words are ones nobody would
guess. The one honest objection, that a VS Code sidebar swaps its
content where ours holds two fixed panes, does not reach a reader who
never saw the 0049 discussion.

## Decision

- The left column is the **sidebar**: the **files pane** above the
  **threads pane**, each shown or hidden on its own, the sidebar drawn
  while either is. "Rail" is retired; records before this one keep it
  as history, as 0056 did for "tree pane".
- The config node `rail { width; split }` is `sidebar { width; split }`.
  An old `rail {}` block is an unknown node, refused with its location
  like any other; no shim, per
  [0051](0051-retire-one-release-compatibility.md).
- The theme keys `ui.rail`, `ui.rail.selected`, and `ui.rail.dir` are
  `ui.sidebar`, `ui.sidebar.selected`, and `ui.sidebar.dir`, the
  spellings 0049 replaced. A theme naming `ui.rail*` fails to load with
  `unknown theme key`, the same as any misspelling; 0051's rule that no
  rename owes a compatibility window holds.
- The code follows the word: `SidebarConfig` and `Config::sidebar()`,
  `App.sidebar`, `sidebar_width()`, `Border::Sidebar`, `Key::UiSidebar*`,
  the theme's `sidebar*` styles, `draw_sidebar`, and the mouse and test
  helpers, so the guide, the config, and the identifiers say one word
  ([0047](0047-one-vocabulary.md)).
- The column's state, the struct 0049 called `Rail`, is `Sidebar` in a
  module of its own, `app/sidebar.rs`, with the width rule beside it.
  It lived in `threads/pane.rs`, which holds the threads pane, not the
  column.
- `app/rail.rs` is `app/files_pane.rs`. It has always held the files
  pane's app-side code and the paging rule; it keeps backing 0023.

### Windows by name

- `Space w f` gives the files pane the keys and `Space w t` the threads
  pane, from any pane; a hidden pane is shown first, with the files
  pane's highlight on the current file. 0056's window submenu is
  spatial, for the reader who knows where the pane is; these two name
  the pane, for the reader who knows which one they want. `Space w
  h/j/k/l/w` and the `Space p` hides are untouched. (`Space Space` was
  later removed as an explicit binding by the 2026-09-14 amendment to
  [0056](0056-the-leader-trimmed.md).)

## Consequences

- `app/sidebar.rs`, which this record backs, holds `Sidebar` and
  `App::sidebar_width`; `app/files_pane.rs` holds what `app/rail.rs`
  held, unchanged.
- The user's own config and theme are the only ones that name the old
  node and keys; each needs the one-word edit the error points at.
- `--config-show` prints `sidebar { width; split }`.
- The binding table gains `WindowFiles` and `WindowThreads` under
  `Space w f` and `Space w t`, labelled "files pane" and "threads pane"
  in the window submenu; `app/window.rs` gains `window_files` and
  `window_threads` over the focus helpers 0056 wrote. The guide's key
  table and [0056](0056-the-leader-trimmed.md)'s map are amended.
- `docs/guide.md` §3, §4, and §7 and `docs/charter.md`'s vocabulary say
  sidebar; [0011](0011-theme-schema.md)'s key table, 0023's module note,
  0047's vocabulary table, and 0049's vocabulary section are amended
  with dated notes.
