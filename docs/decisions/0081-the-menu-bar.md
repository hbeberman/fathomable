---
type: Decision
title: The persistent menu bar
description: A one-row application menu exposes layout, navigation, review, comparison endpoints, help, diagnostics, and project information to the mouse without displacing contextual key bars; it centers the current filename in subdued text, keeps its compact comparison on the right, is optional and keyboard-navigable once open, and shares rounded popup framing with every overlay.
resource: crates/fathomable/src/app/menu_bar.rs
related_resources:
  - crates/fathomable/src/app/doctor_view.rs
tags:
  - configuration
  - decision
  - diagnostics
  - input
  - rendering
---

# 0081 The persistent menu bar

Status: accepted (2026-09-15)

The Auto-jump entry described below was removed by
[0082](0082-three-tool-review-core.md); the rest of this decision remains
current.

Amended 2026-09-17: **Start comparison at current HEAD** and the **All changes
/ Since...** focus remain available inside **Comparison controls...** but are
no longer duplicated in the **Diff** menu.

## Context

Fathomable already made the mouse a peer: pane headers and key bars take
clicks, right-click opens contextual actions, and `Space` and `Space ?`
render the binding table. Those affordances remain local to a pane or
require the reader to know where to click. There was no stable place to
discover whole-viewer workflows, hide the whole sidebar, or move among
files, review, and diff state with the mouse.

A permanent toolbar full of cursor actions would duplicate the contextual
key bars and obscure the quiet reading surface. A conventional
File/Edit/View menu taxonomy would also spend much of a narrow terminal on
categories that do not fit a read-only reviewer.

The user chose a compact hybrid application shell: stable workflow menus on
the left, the current filename quietly centered, compact comparison controls
on the right, and all cursor-specific actions left beside the content they
affect.

## Decision

### One optional row

- The first terminal row is a borderless **menu bar** on `ui.menu`. It
  reserves one row; it never paints over pane headers or content.
- Its left side is `☰  Go  Review  Diff`, with no down-arrow glyphs. A label
  takes `ui.list.hover` while hovered or open.
- The current document's basename is centered against the whole terminal in
  subdued, dim text. It truncates with an ellipsis and disappears before it
  could overlap the workflow menus or comparison controls. Getting started
  and the review list have no filename label.
- Its right side names only the active comparison. The base and target labels
  are muted-blue `ui.popup.key` buttons: hovering patches `ui.list.hover`, and
  clicking opens that endpoint's picker. Passive branch or worktree, full
  path, and major-view identity are deliberately absent.
- On narrow terminals the right-side comparison disappears first. When the
  four labels no longer fit, only `☰` remains and Go, Review, and Diff become
  one-level children of that menu. The row never wraps or scrolls.
- `Space p m` shows or hides the row. The application menu cannot hide
  itself, so the visible UI never removes the only visible route back.

### Menus

- `☰` is titled **Fathomable**. It contains one-level **Layout** and
  **Help** submenus, then Status, About, and Quit.
- **Layout** contains Hide/Show sidebar, Files pane, and Threads pane.
  `Space p s` hides the sidebar as one unit and restores the exact pane
  composition it hid. Showing a child while the sidebar is hidden opens that
  child alone. Hiding the last child remembers it as the next whole-sidebar
  restore. A configured empty composition means no sidebar; `Space p s`
  reports that there is no pane to show until a pane-specific toggle
  establishes one.
- **Help** contains Getting started (`:help`), Doctor (`:doctor`), and View
  keymap (`Space ?`). Getting started reuses the first-workspace page in the
  text column and preserves the document behind it. Doctor is a fresh,
  scrollable in-app rendering of the same structured report as
  `fathomable --doctor`. About (`:about`) is a compact project/version,
  license, and repository view.
- **Go** contains the file pickers, jumplist Back/Forward, Newest change, and
  Auto-jump. **Review** contains the review view and filters plus
  non-destructive thread creation/reply/edit/resolve actions. **Diff**
  contains comparison selection, side pickers, whitespace, checkpoints, and
  mark-seen state. The bracket-pair navigation commands are intentionally not
  copied into these menus.
- Menu order is stable. Unavailable actions remain present and dim. Checked
  rows expose current state; toggle labels say what they will do where that
  is clearer. Destructive thread deletion stays contextual.

### Interaction

- Hover only changes colour. A click opens a menu; while one is open, moving
  over another title switches menus. Clicking the active title, clicking
  elsewhere, or `Esc` closes the stack.
- The compact comparison labels behave like the menu titles: each receives
  the shared accent and hover background, and a click opens its base or target
  picker without opening a title menu.
- A submenu opens on hover, click, `l`/Right, or Enter. Its top border aligns
  with its parent row and touches the parent box. `h`/Left returns to the
  parent. Nesting stops at one level.
- `j`/Down and `k`/Up move among enabled rows without wrapping. Up from the
  first root item focuses its title-bar label; there `h`/Left and `l`/Right
  move among menu labels and `j`/Down re-enters the first enabled row.
  Enter runs an item. Existing displayed key sequences also run their item.
- A menu too tall for the terminal remains anchored and scrolls in place by
  keyboard or wheel. Its border indicates hidden rows. Resizing closes an
  open title menu so changed responsive geometry cannot leave an invisible
  selection active.
- The menu bar remains visible and clickable over every popup and input
  mode. Opening a title menu closes the transient popup or parks the draft.

### Popup frame and configuration

- Every visual popup uses a thin rounded border (`╭╮╰╯`). A title-bar menu,
  `Space`/prefix helper, and right-click menu draw their title or breadcrumb
  in that border rather than spending a body row. Help, pickers, Status,
  Doctor, and About use the same rounded frame on `ui.popup`.
- Picker rows take `ui.list.hover` under the pointer. A left click chooses the
  pointed row, and the wheel moves the picker selection and viewport.
- Menu shortcut columns and borders use the subdued
  `ui.statusline.info` face. Surfaces remain `ui.menu` or `ui.popup`;
  `ui.list.hover` remains the hover treatment. No theme key is added.
- Startup layout is one configuration tree, replacing the old top-level
  `sidebar` block without a compatibility alias:

  ```kdl
  layout {
      menu-bar #true
      sidebar {
          visible #true
          files #true
          threads #true
          width 32
          split 8
      }
  }
  ```

  These are startup defaults only. Runtime toggles do not rewrite config.
  Both child booleans may be false, which implicitly means no sidebar.

## Consequences

- The default launch consistently shows the menu bar and both sidebar panes,
  whether a file or directory was named. A named file retains text focus; a
  workspace start gives the Files pane focus.
- `app/menu_bar.rs` owns menu contents, enabled/checked state, layout, and
  input geometry. Drawing and hit testing consume the same layout.
- `app/doctor_view.rs` owns only scroll and wrapping over the report collected
  by `doctor.rs`; CLI and TUI diagnostics therefore cannot drift.
- `layout.sidebar` owns both startup composition and geometry.
- `Space ?` is presented as **View keymap**. It remains the complete,
  searchable binding-table view.
