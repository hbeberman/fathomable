---
type: Decision
title: The persistent menu bar
description: A one-row application menu exposes layout, navigation, review, diff modes and endpoints, help, diagnostics, and project information; it centers repository and worktree identity, keeps mode-aware endpoints on the right, and shares rounded popup framing with every overlay.
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

The 2026-09-17 amendment that retained **Start comparison at current HEAD**
inside **Comparison controls...** is superseded 2026-09-18 by the diff-mode
amendment below. Both the popup and action are removed without aliases.

Amended later 2026-09-18: the Diff menu places **Head to WorkingTree**
immediately after Base and Target. It pins the current `HEAD` as Base and
selects the working tree as Target through `Space d d`. A separator before
**Save review point** keeps capture separate from endpoint selection.

Amended later 2026-09-17 by
[0068](0068-what-the-files-pane-shows.md): repository and active-worktree
identity move from the Files header to the centered menu-bar label. The
repository/worktree segment opens the worktree picker when several exist.

Amended later 2026-09-17: menu rows left-align their normal-face action
labels and right-align their subdued shortcut labels, retaining at least one
cell between the widest pair. Menu shortcuts abbreviate `Space` as `Sp`.

Amended later 2026-09-17: **Layout** uses stable **Sidebar**, **Files pane**,
and **Threads pane** labels. A checkmark means that surface is shown; toggling
it changes the mark rather than rewriting the label.

The same state-label rule applies to the other checked menu items: **Show
resolved**, **Only current file**, and **Ignore whitespace** retain their
text while their checkmark changes.

Amended later 2026-09-17: **Reviews** replaces **Review threads** as an
unchecked command. It opens and focuses the normal review view; invoking it
again does not close that view.

Amended later 2026-09-17: **Layout** is a top-level menu before **Go**. Its
first section uses a bold `▌` to select exactly one of **File view** and
**Reviews view**; independent checked Sidebar, Files pane, and Threads pane
rows follow a separator. Bare `f` selects File view and bare `t` selects
Reviews view. The centered identity no longer carries the filename, which
moves into the File surface header. Compact mode keeps Layout under `☰`.

Amended 2026-09-18: **Diff** begins with mutually exclusive **Standard
diff**, **Unified diff**, and **Diff off** rows, then Base, Target, Save review
point, and Ignore whitespace. File and every Reviews/history header gain a
rightmost mode control. Active modes show the Base-to-Target pair in the app
bar; Off shows only Target. Normal comparison provenance appears in the
bottom status line only when those endpoint controls do not actually render.

Amended later 2026-09-18: the queued live-change feature is removed, so
**Go** contains the three file pickers and jumplist Back/Forward only.
**Newest change** and `Space j j` have no replacement or compatibility route.

Amended later 2026-09-18 by [0091](0091-pane-focus-navigation.md):
Layout's main choices are **File** and **Threads**, and its sidebar choices
are **File list** and **Thread list**. Pane-title clicks keep opening their
existing settings menus; the new focus marker does not replace or move those
targets. Layout visibility changes do not take focus.

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
the left, repository identity quietly centered, mode-aware comparison
endpoints on the right, and all cursor-specific actions left beside the
content they affect.

## Decision

### One optional row

- The first terminal row is a borderless **menu bar** on `ui.menu`. It
  reserves one row; it never paints over pane headers or content.
- Its left side is `☰  Layout  Go  Review  Diff`, with no down-arrow glyphs.
  A label takes `ui.list.hover` while hovered or open.
- The repository directory name is centered against the whole terminal.
  With several worktrees, ` · <branch>` or the short detached commit follows
  it. The current filename belongs to the File surface header. If space is
  too narrow, repository identity truncates and then disappears before
  overlapping controls.
- Its right side names the selected endpoints. Standard and Unified show
  `base to target`; Off hides Base and shows the bare Target label. Every
  rendered endpoint label is a muted-blue `ui.popup.key` button: hovering
  patches `ui.list.hover`, and clicking opens that endpoint's picker. Passive
  branch or worktree, full path, mode, and major-view identity are deliberately
  absent from this right-side endpoint area.
- On narrow terminals the right-side comparison disappears first. When the
  five labels no longer fit, only `☰` remains and Layout, Go, Review, Diff,
  and Help become one-level children of that menu. The row never wraps or
  scrolls.
- `Space p m` shows or hides the row. The application menu cannot hide
  itself, so the visible UI never removes the only visible route back.

### Menus

- `☰` is titled **Fathomable**. At full width it contains the one-level
  **Help** submenu, then Status, About, and Quit; compact mode also carries
  the hidden workflow menus.
- **Layout** is a top-level menu. **File view** and **Reviews view** are
  command rows in its first section; exactly one is active and carries a
  bold `▌`, a radio-like choice rather than an independent toggle. After a
  separator, Sidebar, Files pane, and Threads pane use checkmarks beside
  each visible surface. Labels remain stable when toggled.
  `Space p s` hides the sidebar as one unit and restores the exact pane
  composition it hid. Showing a child while the sidebar is hidden opens that
  child alone. Hiding the last child remembers it as the next whole-sidebar
  restore. A configured empty composition means no sidebar; `Space p s`
  reports that there is no pane to show until a pane-specific toggle
  establishes one.
- **Help** contains Getting started (`:help`), Doctor (`:doctor`), View
  keymap (`Space ?`), and Licenses (`:licenses`,
  [0088](0088-bundled-licenses.md)). Getting started reuses the first-workspace
  page in the text column and preserves the document behind it. Doctor is a fresh,
  scrollable in-app rendering of the same structured report as
  `fathomable --doctor`. Warnings use a distinct face and do not make the
  report fail; `r` rebuilds the report so corrected permissions clear
  immediately. Licenses displays the embedded first- and third-party notices
  offline. About (`:about`) is a compact project/version, license, and
  repository view with directions to the full notices.
- **Go** contains the file pickers and jumplist Back/Forward.
  **Review** contains the review view and filters plus
  non-destructive thread creation/reply/edit/resolve actions. **Diff**
  begins with bold-`▌`, mutually exclusive **Standard diff**, **Unified diff**,
  and **Diff off** choices. Base, Target, and **Head to WorkingTree** follow,
  then a separator and **Save review point**, then a separator and **Ignore
  whitespace**. Ignore whitespace remains checked but dim while Off. The
  removed Comparison controls popup, Start comparison at current HEAD, typed
  commit batches, and `:diff` have no menu rows or compatibility aliases. The
  bracket-pair navigation commands are intentionally not copied into these
  menus.
- Menu order is stable. Unavailable actions remain present and dim. Checked
  rows use stable state labels and expose current state only through the
  checkmark. Action labels are left-aligned in the normal menu face and
  shortcut labels are right-aligned in the subdued info face. The widest
  pair determines the menu width with at least one cell between them, and
  menu shortcuts abbreviate `Space` as `Sp`. Destructive thread deletion
  stays contextual.

### Interaction

- Hover only changes colour. A click opens a menu; while one is open, moving
  over another title switches menus. Clicking the active title, clicking
  elsewhere, or `Esc` closes the stack.
- The rendered endpoint labels behave like the menu titles: each receives the
  shared accent and hover background, and a click opens its Base or Target
  picker without opening a title menu. Off has no Base label or Base hit
  region.
- With several worktrees, the centered repository/worktree segment uses the
  same accent and hover background; clicking it opens the worktree picker.
  With one worktree the repository identity is subdued and inert.
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

### Header mode control

- The File header and the headers of Reviews, Recently resolved, and Archived
  end with dim `Diff: standard`, `Diff: unified`, or `Diff: off`.
- The control outranks passive counts and filters when width is constrained.
  It disappears only when it cannot coexist with the surface title.
- Clicking it opens an anchored, three-row, non-searchable choice popup with
  the same framing, hover, cursor, and `▌` selection language as Layout.
- The control changes the session-global mode. It is not a per-file or
  per-history-view setting.

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

- The app bar and bottom status line use actual layout, not configured
  visibility, to avoid duplicated comparison provenance. Normal `CMP ...` or
  unified-diff provenance is omitted when endpoint controls render. Stale and
  error status remains visible. When the menu bar is hidden or its endpoint
  labels are too wide, the status line supplies the mode-aware fallback,
  including `OFF Target ...`.

## Consequences

- The default launch consistently shows the menu bar and both sidebar panes,
  whether a file or directory was named. A named file retains text focus; a
  workspace start gives the Files pane focus.
- `app/menu_bar.rs` owns menu contents, enabled/checked state, layout, and
  input geometry. Drawing and hit testing consume the same layout.
- `app/doctor_view.rs` owns only scroll and wrapping over the report collected
  by `doctor.rs`; CLI and TUI diagnostics therefore cannot drift.
- `layout.sidebar` owns both startup composition and geometry.
- The File and Reviews/history headers own the session-global diff-mode
  control; the app bar owns only endpoint selection.
- `Space ?` is presented as **View keymap**. It remains the complete,
  searchable binding-table view.
