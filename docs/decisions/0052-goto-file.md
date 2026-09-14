---
type: Decision
title: One action opens linked files and URLs
description: gf, a Ctrl-click, and one context-menu entry open local file references in the viewer at their line and URLs through xdg-open; the jumplist records file hops so Alt-Left returns.
resource: crates/fathomable-core/src/link.rs
related_resources:
  - crates/fathomable/src/app/goto_file.rs
  - crates/fathomable/src/app/input/keys.rs
  - crates/fathomable/src/app/input/mouse.rs
  - crates/fathomable/src/app/input/menu.rs
  - crates/fathomable/src/app/input/bindings.rs
  - crates/fathomable/src/app/jumplist.rs
tags:
  - decision
  - input
  - rendering
---

# 0052 One action opens linked files and URLs

Status: accepted (2026-09-04)

Amended 2026-09-14: `gf` is the single `open linked file/URL` action.
Local references still open in the viewer; URLs use the desktop opener.
`gy`, `gx`, and the separate context-menu copy/open-link entries are
removed. The original separation described in Context is superseded.

## Context

[0050](0050-mouse-menus-and-gestures.md) gave a rendered Markdown link
two keys: `gy` copies its URL and `gx` hands it to `xdg-open`. Both treat
every link alike, so `[0049](0049-inline-threads-and-the-rail.md)` in a
record, or `view.rs:870` in a review comment, leaves the viewer for the
desktop's opener instead of showing the file where the reader already is.
The documents Fathomable is built to show are full of such references:
records point at each other and at the code that backs them, agent
replies name `path:line`, and the guide names files by path. The user
asked (2026-09-04) for a key and a mouse gesture that open such a
reference *in Fathomable*, with a way back.

Helix has no key that opens a link in a browser; `gx` is Vim's, from
netrw. Helix's `gf` is *goto file*: open the path under the cursor in
the editor. The user chose in a question round the same day: `gf` and a
Ctrl-click for the hop, `Alt-Left` for the way back, Markdown links and
bare `path:line` words as the forms, and anything with a scheme left to
`gx`.

## Decision

- **A reference** is either the destination of the rendered link
  under the cursor or, when the cursor is not on a link, the word under
  it in the source: the longest run of path/URL characters (letters,
  digits, `/ . _ - ~ : # @ + % = ? &`) around the cursor, with wrapping quotes and brackets and
  trailing sentence punctuation trimmed, so `(see docs/guide.md).` and
  `` `view.rs:870` `` both yield their path. Reading the source preserves
  a reference's portions across wrapped rows and a URL's query/fragment.
- **Parsing** (`fathomable_core::link::parse`) splits the reference into
  a path and an optional 1-based line. The line comes from a trailing
  `:LINE` or `:LINE:COL` (compiler style; the column is accepted and
  ignored) or a `#LLINE`, `#LLINE-LEND`, or `#LLINEC…` fragment (code
  host style); any other `#fragment` is dropped. `%XX` escapes in a link
  destination are decoded. `is_external` classifies references carrying
  `://` or `mailto:` (case-insensitive for the latter) as URLs rather
  than files. Empty or fragment-only references do not launch an opener.
- **Resolution** is against the current document's directory first and
  the workspace root second, as a Markdown renderer and a compiler
  message respectively would read it; an absolute path must lie inside
  the workspace. The first candidate that is a file wins; a directory or
  a path that is not there gets a notice and nothing moves. Resolution
  is the app's, since only it knows the root and the open document.
- **`gf` on a file** opens it as any far move does (focus to the text, the
  popup closed, the jumplist recording where it left) and lands the
  cursor on the line, or on line 1 when the reference has none. It is a
  far move in the sense of [0049](0049-inline-threads-and-the-rail.md),
  so **`Alt-Left`** is the way back and `Alt-Right` returns; there is no
  second stack. On a URL it leaves the document and cursor in place and
  returns an external-open effect; no jumplist entry is added.
- **The mouse.** A **Ctrl-click** in the text places the cursor on the
  cell and runs `gf` there. Terminals forward a Ctrl-click under mouse
  capture; Shift-click stays the terminal's own escape hatch and the
  viewer's extend-selection gesture. The **context menu** offers
  `open linked file/URL` (`gf`) for an external URL or a local file that
  exists; any word parses as a path, so the menu checks the file system
  where the key only notices. There are no separate link-copy or
  external-open entries.
- **The desktop opener** is `xdg-open` on the viewer host. Its actual
  launch detects availability; a missing command names `xdg-utils`,
  other launch errors and unsuccessful exits are reported, and the
  viewer remains responsive while the child is reaped. No shell
  interprets the URL. A working desktop handler is still required;
  launching the process does not prove that a browser displayed it.
  OSC 8 hyperlinks and capability detection are not implemented.

## Consequences

- `fathomable-core` gains `link`, a pure module: `parse` and `word_at`,
  tested at the boundary on the forms above. `layout::Line` gains
  `byte_at`, the byte offset in its text under a display column, which
  the word lookup needs.
- `app/goto_file.rs` holds the reference lookup on the view, the
  resolution against the workspace, and the `gf` action; `is_far_move`
  lists `GotoFile`; the mouse and the menu call the same action.
- A reference inside an expanded thread's message rows is not reachable
  by `gf` yet: those rows are drawn from their own layout, not the
  document's, so the cursor cannot rest on their text. Extending the
  lookup there is a follow-up, not a change to this record.
- `docs/guide.md` gains the key, the gesture, and the menu entry, and
  the jumplist row names `gf` among the far moves.
