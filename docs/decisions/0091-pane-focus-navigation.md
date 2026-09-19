---
type: Decision
title: Pane focus navigation
description: >-
  Direct pane keys, focus cycling, live list preview, visible focus ownership,
  and minimum terminal composition.
resource: crates/fathomable/src/app/window.rs
tags:
  - decision
  - input
  - rendering
---

# 0091 Pane focus navigation

Status: accepted (2026-09-18)

Amended later 2026-09-18: in File list, `z` on a file toggles its immediate
parent directory and moves the cursor there when folding, so the next `z`
unfolds that same directory. A root-level file remains unchanged.

Supersedes the window-focus contract in
[0056](0056-the-leader-trimmed.md), amends the pane names in
[0057](0057-the-sidebar.md) and [0081](0081-the-menu-bar.md), extends the
focus language in [0079](0079-list-focus-language.md), and updates the
cross-pane terminology in [0090](0090-direct-workspace-navigation.md).

## Context

Pane navigation mixed a leader sequence, direction relative to the main
column, and two ambiguous uses of **Threads**. Focus was visible mainly
through a selected list row, so an empty pane or a main content surface did
not clearly say where ordinary navigation keys would go. List movement also
opened content by implication, and a terminal too small for the configured
split could leave useful controls outside the visible composition.

## Decision

### Four pane names and direct focus

The main column is either **File** or **Threads**. The sidebar panes are
**File list** and **Thread list**. Their direct keys are `f`, `t`, `F`, and
`T`, respectively. A direct key shows its target when hidden and focuses it;
using it on an already visible, focused target is idempotent.

Bare `w` cycles forward and `W` cycles backward through the displayed panes:
the current main surface, File list, and Thread list. Hidden panes are
skipped. `Space w` has no navigation bindings. The existing `Space d w`
whitespace action and the Diff menu remain.

Layout controls only show or hide panes. They do not take focus. Hiding the
focused sidebar pane returns focus to the displayed main surface. Focus
changes do not alter selections, content, folds, or scroll positions.

`Esc` first cancels transient UI. From either sidebar pane it returns to the
current main surface. From main Threads it no longer switches to File; `f`
does that explicitly. Bare arrows and `h`/`j`/`k`/`l` remain local. `Tab`/`Shift-Tab` retain
open-thread traversal; shifted arrows and `H`/`J`/`K`/`L` traverse comparison
changes and changed files; `Alt-Left`/`Alt-Right` retain history traversal.

In File list, `z` toggles the selected directory or the immediate parent of a
selected file. Folding a file's parent moves the cursor to that directory, so
the next `z` unfolds it; a root-level file has no visible parent row and stays
unchanged. `Z` recursively unfolds directories admitted by the current
filters, or folds them all when already expanded. Recursive unfolding skips
symlink directories. Folding keeps the selected path when visible, otherwise
its nearest visible ancestor; it never opens a file or transfers focus.

### Lists preview without stealing focus

Moving or clicking in File list or Thread list updates the main surface as a
live preview while navigation stays in the list. Clicking a row focuses its
list, not the main column. `Enter` explicitly opens
and focuses File source, or the Threads evidence fallback when source cannot
be displayed. Hover never focuses.

Preview never resumes a parked draft; explicit File activation does. A
Thread-list selection reveals that same thread or its folded representation
in main Threads, without changing its filters or folds. A directory preview
remains displayed across focus changes and cannot act on a retained hidden
source document.

The mouse wheel changes only the pointed viewport. It never changes a file
or thread cursor, selection, preview, main surface, or focus. Pane-title
clicks keep their existing settings menus; plain header or content space may
focus the pane without replacing the title-menu target.

### One visible navigation owner

Every pane header reserves its first cell for a `▏` marker. When
`App::pane_has_navigation` says that pane is the current normal keyboard
owner, the marker and pane name use `ui.pane.focus`; all other title content
stays neutral. The reserved cell keeps filenames, counts, filters, status,
and controls at the same columns when focus changes.

The treatment applies to File, File list, Threads, and Thread list, including
empty panes and File directory, information, no-document, and Threads
history surfaces. Exactly one pane is focused during normal navigation.
Menus, popups, pending prefixes, Compose, and command or search input suspend
all pane-focus treatment. Existing `ui.list.active` and
`ui.list.inactive` selection rows remain independent.

`ui.pane.focus` is a foreground role. The built-in dark and light themes use
their existing purple palette values, `#c397d8` and `#7d3c98`; the header
continues to use the `ui.header` background.

### Minimum composition

Fathomable keeps the configured split rather than replacing it with a compact
layout. `App::minimum_pane_size` reports the minimum terminal dimensions for
the currently displayed composition, and `App::panes_fit` is the single
fit decision used by drawing and input.

With the app bar shown, the inclusive minimums are `20×5` for the main
surface alone, `28×5` with File list, `28×6` with Thread list, and `28×8`
with both sidebar lists. Hiding the app bar subtracts one required row. These
thresholds include the persistent status row and enough pane chrome and body
space to keep every displayed pane visible.
The sidebar leaves at least 20 main columns. Thread list shows its action
footer only while it owns navigation and otherwise gives that row back to
entries. At minimum File height, the footer yields to a deletion banner
rather than hiding all content.

When the terminal is below that minimum, pane contents and their hit regions
are replaced by an explicit size warning rendered in the actual viewport.
The warning reports required and current dimensions and directs the user to
resize, use Layout to hide a pane, or quit. It clips safely at tiny and
zero-sized viewports. Resize and Layout recovery restore the same focus,
selection, preview, and scroll state.
Hidden overlays accept no typing, paste, mouse actions, or submission;
`Esc` dismisses or parks them, and quit confirmation and Layout recovery remain
usable. Passive relayout preserves source selections and independently scrolled
viewports instead of pulling them back to the keyboard cursor.

## Consequences

- `app/window.rs` owns pane order, direct focus, visibility recovery, and the
  definition of which pane currently owns navigation.
- Drawing consumes `minimum_pane_size`, `panes_fit`, and
  `pane_has_navigation` rather than re-deriving behavior state.
- Header drawing and title hit testing continue to share the same title
  widths; the reserved marker replaces the former leading blank cell.
- The theme schema gains `ui.pane.focus`; custom themes inherit it from
  `default-dark` unless they override it.
- Existing behavior-level tests outside `app/draw` must use the new names and
  direct keys while preserving the Review menu's workflow terminology.
