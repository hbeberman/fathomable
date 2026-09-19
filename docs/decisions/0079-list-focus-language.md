---
type: Decision
title: List focus language
description: Files, threads, review, and picker results share active and remembered selection styles, distinct from neutral headers; help and menus use hover alone, and author stripes and thread-state colours remain independent.
resource: crates/fathomable/src/app/draw/selection.rs
tags:
  - configuration
  - decision
  - input
  - rendering
---

# 0079 List focus language

Status: accepted (2026-09-14)

Amended 2026-09-18 by [0091](0091-pane-focus-navigation.md): selection
styling remains unchanged, while every pane header now reserves a `▏` cell
and uses `ui.pane.focus` on that marker and the pane name only when the pane
owns normal navigation. Overlays, prefixes, Compose, and command/search input
suspend both the header treatment and underlying active-list treatment.

## Context

The files pane's selected row looked like its header, and files, threads,
the review list, and pickers disagreed about whether a highlight meant
keyboard focus or a remembered position. The user approved one rule:
active selection is a blue-tinted row with a bright left-edge bar;
remembered selection is a quieter tint without the bright bar; hover is
subtle; headers stay neutral. Thread-state colours and
[author stripes](0071-author-stripes.md) must keep their meanings.

## Decision

### One focus language

- **Active selection** uses `ui.list.active` background and a left-edge
  cursor bar in `ui.list.cursor` foreground, only while that list owns
  the keys. The marker ignores any `bg` on `ui.list.cursor`, keeping the
  row's background one continuous band.
- **Remembered selection** uses `ui.list.inactive` background without a
  bright bar. Losing the keys does not erase the selected row.
- **Hover** uses only `ui.list.hover` background on the existing help and
  menu hover targets. These surfaces scroll, filter, or match shortcuts;
  they have no selected keyboard item and never draw a cursor bar.
  This does not add hover navigation to the other lists.
- **Headers** retain neutral `ui.header`; it is not a selection colour.
  A selectable file or thread header inside a list is an entry, not pane
  chrome, and takes the selection treatment when selected.

The built-in active and inactive styles set only the background, with no
forced bold. The active bar supplies the non-colour focus cue; changing
focus does not change text weight.

### Where it applies

- **Files:** every selectable file and directory row. Reserve one cursor
  cell before the existing git gutter; keep the status letters, their
  colours, and the remaining tree layout rather than replacing a git
  mark with the selection bar.
- **Threads pane:** both rows of a thread and file-group rows, folded
  groups included. The sidebar's current-file `thread.focus` tint remains
  a separate context cue; actual active or remembered selection overrides
  it, without changing which file or thread the cursor names.
- **Review list:** selectable file rows, thread headers, and folded file
  or thread rows use the shared treatment. A selected message's author
  and body rows retain `thread.user` / `thread.agent` stripes and author
  name colours instead of taking the list tint. Their selection bar uses
  `ui.list.cursor` and is bright only while review owns the keys.
  Only the bar changes with focus on those message rows. The selected
  thread's header remains selected alongside its message: active tint
  and bar while review owns the keys, remembered tint otherwise, as the
  existing thread cursor model requires. The ancestor file's bar is
  muted `ui.statusline.info` context, not another active selection.
  Selecting the file row itself gives it the active or remembered
  treatment.
- **Pickers:** every `PickerKind` result uses the same selection rules,
  not just file results. `ui.picker.match` still styles matched
  characters.

Thread-state circles, state text, author stripes, drafts, and the
document gutter's thread bracket keep their separate roles. The inline
thread cursor in the document retains `thread.cursor`; the shared list
cursor replaces it for review selection, not for every thread rendering.

Amended 2026-09-14: the document uses the same `ui.list.cursor`
foreground on the current source line number while it owns navigation,
instead of tinting the text row. Its number returns to `ui.linenr` when
another pane, overlay, prefix, or command/search input owns the keys;
see [0010](0010-viewer-ux.md).

### Ownership of the keys

Help, Status, Picker, context Menu, Compose, and a pending key prefix
deactivate underlying lists: their selected rows use the remembered
treatment and no bright selection bar. A picker still owns its own
result selection while open. A key menu or help row can be hovered
without suggesting keyboard selection.

Closing an overlay, finishing or cancelling a prefix, or returning focus
restores the appropriate active highlight. This is a drawing rule, not
a navigation change: it does not move a cursor, change a fold or scroll
position, or alter an existing key, click, or action's routing.

### Theme contract and migration

| Theme key | Role | Channel |
| --- | --- | --- |
| `ui.list.active` | `UiListActive` | `bg` |
| `ui.list.inactive` | `UiListInactive` | `bg` |
| `ui.list.cursor` | `UiListCursor` | `fg` |
| `ui.list.hover` | `UiListHover` | `bg` |

[0011](0011-theme-schema.md) owns the exact built-in colours. Both
built-ins give these roles dedicated colours distinct from `ui.header`;
palette values are not additional schema keys.

`ui.sidebar.selected` and `ui.picker.selected` are removed, not aliases,
under [0051](0051-retire-one-release-compatibility.md). A theme containing
either fails with an unknown-key error. Remove those entries and either
inherit the shared defaults or set the four roles above. A previous
selected background maps to `ui.list.active`; choose a quieter
`ui.list.inactive`, a distinct `ui.list.cursor` foreground, and a subtle
`ui.list.hover` background rather than using one old style for all four.
Keep `ui.sidebar`, `ui.sidebar.dir`, and `ui.picker.match` unchanged.

## Consequences

- `app/draw/selection.rs` owns the shared presentation rules; list
  renderers consume them without owning separate focus palettes.
- The guide and theme vocabulary describe one focus language, while
  earlier selection and context-bar decisions carry dated supersession
  notes. No existing resource changes its documentation owner.
- Focus can be recognised without reading a pane title, and remembered
  positions, thread state, and authorship remain visible without
  competing for the active cursor.
- The title marker added by 0091 is a redundant pane-level cue for empty and
  non-list surfaces; it does not replace or recolour list selection.
