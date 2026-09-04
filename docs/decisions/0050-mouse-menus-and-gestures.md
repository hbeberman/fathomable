---
type: Decision
title: Mouse menus and gestures
description: A right-click opens a context menu of the actions that apply where the pointer is, in the text, the rail, and the review list, each entry showing its key from the binding table; the Space menu and Space ? take clicks; the gutter, double- and triple-click, and Shift-click select; pane-header hints take clicks; and links copy or open from the menu.
resource: crates/fathomable/src/app/input/menu.rs
related_resources:
  - crates/fathomable/src/app/input/mouse.rs
  - crates/fathomable/src/app/input/bindings.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/view.rs
  - crates/fathomable/src/app/rail.rs
  - crates/fathomable/src/app/threads/pane.rs
  - crates/fathomable/src/app/threads/list.rs
  - crates/fathomable/src/app/run.rs
tags:
  - decision
  - input
  - rendering
---

# 0050 Mouse menus and gestures

Status: accepted (2026-09-04)

## Context

The mouse has been first-class since [0007](0007-key-grammar-and-mouse.md):
a click places the cursor, a drag selects, the wheel scrolls the pane it
is over, and the borders drag. But everything a selection or a thread
can *do* was keyboard-only. A reader who has dragged over a few lines
must know that `c` comments and `y` copies; a reader on a stub must know
`r`, `o`, `e`, and `dd`. The `Space` menu lists keys but does not take a
click, so the mouse could open nothing and run nothing. The tree, the
threads pane, and the review list took a click to move and a wheel to
scroll and no more. Links in rendered Markdown were coloured and inert.

The user asked (2026-09-04) for a right-click menu on a selection in the
text, "comment, copy to clipboard and whatnot", and for other ways to
make the viewer mouse-accessible. Settled in two question rounds the
same day; the choices are below.

## Decision

### The context menu

- A **right-click** opens a context menu at the pointer. It lists the
  actions that apply to what is under the pointer, each with the key
  that runs it from the keyboard, so the menu is also how the keys are
  discovered. The key spellings come from the one binding table
  ([0045](0045-bindings-are-data.md)) through `hint(place, action)`;
  the labels are the menu's own, worded for the context (`comment on
  selection`, not the table's generic label). A test checks that every
  entry's action is bound on the place the menu opened for, so no entry
  is ever keyless.
- **Before the menu opens**, a right-click in the text behaves as a
  GUI editor's does: inside the selection it keeps the selection;
  outside it clears the selection and places the cursor on the clicked
  cell (a stub row lands on the row it hangs under, as a left-click
  does). In the tree it moves the highlight to the row and shows the
  file as the wheel does, without expanding a directory. In the threads
  pane and the review list it moves the thread cursor to the entry. The
  actions then act on the cursor as their keys would; the menu needs no
  target of its own.
- **The entries**, in the text, in this order and only those that apply:
  - over a selection: `comment on selection` (`c`), `new thread on
    selection` (`C`), `copy selection` (`y`), `clear selection` (`Esc`);
  - on a stub or an expanded thread's rows: `expand thread` / `fold
    thread` (`c`), `reply` (`r`), `resolve` / `reopen` (`o`), `edit
    message` (`e`, when the message under the cursor or the user's
    newest is theirs), `delete thread` (`dd`);
  - on a rendered link: `copy link` (`gy`), `open link` (`gx`);
  - on any line without a selection: `comment on line` (`c`, or `C`
    when a thread already covers the line so `c` would expand it),
    `select line` (`x`), `copy line` (`y`). `y` with nothing selected
    now copies the cursor line, so the entry has a key; it copied
    nothing before.

  In the tree: `open` (`Enter`), `checkpoint this file` (`Space v c`,
  on a file), `copy path` (`y`, new), `re-read the tree` (`R`), `toggle
  ignored` (`I`). "Reveal" was in the proposal and is dropped: it
  reveals the current file, which a right-click has just made the row.
  In the threads pane and the review list: `go to` (`Enter`), `reply`
  (`r`), `resolve` / `reopen` (`o`), `edit message` (`e`), `delete
  thread` (`dd`).
- **Delete from the menu deletes at once.** `dd` needs two presses
  because a stray key must not delete ([0034](0034-deleting-threads.md));
  a click on an entry that says `delete thread` is already deliberate.
  The entry still shows `dd`. Arm-then-confirm in the menu was offered
  and rejected.
- **While the menu is open**: moving the mouse highlights the row under
  it, a left-click on a row runs it, and typing runs the entry whose
  key it is (a two-key entry such as `dd` or `gx` waits for its second
  key, as the prefix does elsewhere). `Esc`, any key that is not an
  entry, or a left-click outside the menu closes it and is swallowed
  (on the selection menu `Esc` is the `clear selection` entry, so it
  also clears); a right-click outside closes it and opens the menu for
  the new position. The menu is one more `Popup`, so the wheel and
  drags do nothing under it, as under the pickers.
- **Drawing.** The menu's top-left corner is the pointer cell, shifted
  left or up when it would leave the screen. A first row in the pill
  colour names what the menu acts on (`selection`, `line 42`, `thread`,
  `README.md`), as the which-key breadcrumb does. Entries are `key
  label` rows in the popup faces (`ui.popup`, `ui.popup.key`); the
  highlighted row uses `ui.picker.selected`. No new theme key.
- The comment box keeps the keys and the mouse works around it
  ([0018](0018-comment-editor.md)); a right-click in the box does
  nothing. Under the help, status, and picker popups the mouse is
  ignored as before.

### Menus take clicks

- The which-key menu that a prefix opens (`Space`, `g`, `[`, `]`, `d`)
  takes the mouse: hovering highlights an entry, a left-click on it is
  that key typed, so it runs the binding or descends into the submenu,
  and a click elsewhere drops the prefix as `Esc` does. The layout that
  places entries in rows and columns is one function the drawing and
  the mouse share, so a click lands on the entry that was drawn there.
- `Space ?` takes a click on a row: the binding runs when it applies
  on the focused surface (its place, or `Any` on a pane), else the
  popup closes as any key closes it. A click closes `:status` too.
  Columns of the help that the terminal is too narrow to show are cut
  off as before and cannot be clicked.

### Selection gestures

- A **left press in the gutter** selects that line, linewise, and a
  drag that started in the gutter extends the selection by whole lines
  (as `V` then `j`). A drag that started in the text keeps selecting by
  columns.
- A **double-click** selects the word under the pointer, a
  **triple-click** the line. A word is a run of letters, digits, and
  underscores, else a run of other non-blank characters (vim's `iw`).
  Clicks count when they land on the same cell within 400 ms; the count
  is kept beside the drag state, not as a mode.
- **Shift-click** extends the selection to the pointer, from the cursor
  when there is none. Most terminals keep Shift with the mouse for
  their own selection (Ghostty, kitty, and foot do), in which case the
  event never reaches the viewer; the gesture is there for terminals
  that pass it on.
- Every gesture ends in `SEL` mode like a drag does
  ([0013](0013-annotation-storage-and-ux.md)), so `y`, `c`, `C`, and the
  context menu apply.

### Chrome takes clicks

- The hints a pane header shows right-aligned (`Esc close`, `r reply`,
  `h/l page`, and the rest) are drawn from the binding table, and now
  carry their action: a left-click on a hint focuses that pane and runs
  it. A hint that names two keys (`h/l`) is split at its slash. The
  headers are the threads pane's, the review list's (while it has
  focus; unfocused, a click focuses it), the checkpoint view's, an
  expanded thread's in the text, and the comment box's. Each header is
  built once as words and hints (`Header` in `draw`) and both the
  drawing and the mouse read it, so a hint the row was too narrow to
  show is not clickable either.
- A click on the threads pane's header text toggles its reach between
  the file and the workspace, as `s` does.
- In the checkpoint header a click on the base name opens the base
  picker and a click on the target name the target picker, as `b` and
  `t` do.
- The status line carries no hints, only the state, so nothing on it is
  clickable.

### Links

- Rendered Markdown keeps a link's URL in its face (`Face::Link`), so
  the text under the pointer knows its link. `gy` copies the URL
  through OSC 52 (`Effect::Copy`, as `y` does); `gx` opens it with
  `xdg-open` (vim's `gx`), the one place the viewer starts a process.
  Opening is an `Effect` the run loop performs, so `App` stays a pure
  state machine under test; a failed spawn is a notice. A left-click on
  a link still only places the cursor.
- Linux only, as the charter says; `xdg-open` is looked up on `PATH` at
  the moment of use and reported missing in the notice.

### Not done

- A draggable scrollbar on the text pane, and a key that opens the
  context menu at the text cursor for terminals that keep right-click,
  are [parked](../parked.md).

## Consequences

- [0007](0007-key-grammar-and-mouse.md) is amended: the mouse also
  opens menus, and the right button is taken.
- [0013](0013-annotation-storage-and-ux.md) is amended: the gutter,
  double- and triple-click, and Shift-click select as a drag does.
- [0034](0034-deleting-threads.md) is amended: the menu's `delete
  thread` entry deletes without the second `d`.
- [0045](0045-bindings-are-data.md) is amended: the menus the table
  renders take the mouse, and hints carry their action; `gx`, `gy`,
  and the tree's `y` are new rows.
- Right-click reaches the viewer only where the terminal forwards it
  under mouse capture, which Ghostty, kitty, foot, WezTerm, and
  Alacritty do; a terminal that keeps the button for its own menu gets
  no context menu, and the `Space` menu, now clickable, is the fallback.
- The event loop already redraws after every event; the extra hover
  events cost a redraw only while a menu is open.
- `crates/fathomable/src/app/input/menu.rs` is new: the menu state, its
  entries per context, and the layout that both the drawing and the
  mouse read. Mouse routing stays hand-written in `mouse.rs`.
