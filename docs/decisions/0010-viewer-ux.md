---
type: Decision
title: Viewer UX conventions
description: Cursor, gutter, status line, selection-to-clipboard, search, and reload behaviour of the rendered view, modelled on Neovim and Helix.
resource: crates/fathomable/src/app/view.rs
related_resources:
  - crates/fathomable/src/app/clipboard.rs
  - crates/fathomable/src/app/view/navigation.rs
tags:
  - decision
  - input
  - rendering
---

# 0010 Viewer UX conventions

Status: accepted (2026-08-26); amended 2026-09-05 by
[0060](0060-one-diff-two-sides.md): the diff badge names its base
(`DIFF HEAD`, `DIFF seen`, `DIFF cp 2/3`, `DIFF a1b2c3d`), and `CHECK`
is gone.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

[0007](0007-key-grammar-and-mouse.md) fixes the key grammar but leaves the
look and feel of the rendered view open. The user wants Fathomable to feel
like Neovim and Helix: minimal by default, one status line at the bottom, a
gutter that carries information by colour rather than icons, and mouse
selection that lands on the clipboard without a further key. These choices
were captured in a question round on 2026-08-26 and are recorded here so the
viewer, the layout engine, and later themes agree.

## Decision

### Cursor

- The view has a real row-and-column cursor like Neovim, not a scroll-only
  viewport. `h`/`l` move by grapheme cluster on the rendered line, `j`/`k`
  by visual (rendered) line; the column is remembered across vertical moves
  as in Vim (`curswant`).
- The cursor line is highlighted subtly (theme key `ui.cursorline`); the
  cursor cell itself uses the terminal cursor.
  - Amended 2026-09-04: **the cursor row is not tinted.** The terminal
    cursor alone marks the position; `ui.cursorline` leaves the theme
    vocabulary ([0011](0011-theme-schema.md)) and a theme naming it is
    an unknown key. The user dropped the tint on 2026-09-04 because it
    read as a second highlight beside the thread and selection tints.
- `scrolloff` is 3 rendered lines. `Ctrl-d`/`Ctrl-u` move half a page,
  `gg`/`G` go to the first and last rendered line.
- Amended 2026-09-14: `gh` and `gl` go to the start and end of the
  logical source line across wrapped rows. Start includes indentation,
  end is the last displayed grapheme, and both extend a selection.
  Hidden Markdown syntax is not a cursor stop; synthesized and thread
  rows without source keep row-local boundaries. `0`/`$` and
  `Home`/`End` retain their rendered-row behavior.
- The viewport scrolls to one `~` gutter row after the document. Its
  usable height excludes the text's key bar while shown, so the EOF
  marker and the last content row remain above the bar. No extra cursor
  stop or numbered source line is added.
- Synthesised lines (table rules, block spacing) are valid cursor rows; they
  simply carry no source range.
- Amended 2026-09-04: **`h` and `l` wrap.** At the first column `h` moves
  onto the last column of the rendered row above and at the last column
  `l` onto the first column of the row below, as Helix does, in every
  mode; a blank or synthesised row is one stop with column 0, so a
  selection that crosses a paragraph break passes through it. `h` at
  column 0 no longer focuses the tree ([0012](0012-workspace-mode.md));
  `Space e` and the mouse reach it. The user chose the wrap on 2026-09-04
  because a fragment inside a paragraph could not be selected across
  its rows with the keyboard.

### Gutter

- Left of the text, always reserved, never shifting content:
  `[line number][diff bar] text`. [0013](0013-annotation-storage-and-ux.md)
  (2026-08-26) adds a fourth cell for annotations after the diff bar;
  [0006](0006-git-access.md) (2026-08-26) then moves it to the far left:
  `[note][line number][space][diff bar] text`.
- The line number is the **absolute source line** of the first source byte the
  rendered line came from. Wrapped continuation lines and synthesised lines
  show a blank number. Width is the digit count of the largest source line
  number plus one space of padding. Relative numbers are a later config
  option, not v1.
- The diff bar is a one-cell column immediately right of the numbers, showing
  `▎` coloured by theme keys `diff.plus`, `diff.delta`, `diff.minus`
  (Helix's layout). Removed hunks mark the line after the removal with a
  thin rule `▔` along the top of its cell (0006). In
  milestone 1 the bar is present but empty; [0006](0006-git-access.md) fills
  it. No sign icons; colour carries the information, and the theme keys
  exist so colour-blind users can pick their own palette.

### Status line

- One line at the bottom, Helix style: a mode pill on the left in inverse
  video (`NOR`, `SEL`, `CMD`, `SRC` for source view) using theme keys
  `ui.statusline.normal` and friends, then the file path (relative to the
  workspace root or as given), a `[+]` marker when the file changed on disk
  since it was opened, then on the right `line:col` in **source** coordinates,
  the percentage through the document, and the session id from
  [0009](0009-cli-and-diagnostics.md).
- Amended 2026-09-03: **the pill says one thing** — the mode (`NOR`,
  `SEL`, `CMD`, `SRCH`) while the text has focus, else the focused pane
  (`TREE`, `THREAD`, `LIST`, `FILE THREADS`). How the text is shown is a
  badge after the path (`SRC`, `DIFF`, `DIFF seen`) and so is auto-jump
  (`AUTO`), so neither disappears when focus moves. `DELETED` is the
  banner only. The right block reads `line:col`, the percentage, `+a -r`,
  then every count as `N word`: `2 waiting  3 threads  1 followed`.
- `:` and `/` input replace the status line while active, as in Vim. Pending
  key sequences and transient messages ("search hit BOTTOM, continuing at
  TOP", "copied 3 lines") use the same line and clear on the next key.
- Amended 2026-09-03: a **notice** answers the reader's own key on the
  status line and clears on the next one; a **toast** reports what
  happened without the reader (a reply landing, auto-jump switching off)
  and fades on its own. The app and the view each raise notices; the
  status line reads them through one accessor.

### Selection and clipboard

- Mouse drag selects like a terminal: the anchor is the press position, the
  head follows the pointer, both in rendered row-and-column coordinates.
  Releasing the button originally **copied immediately**; since
  [0013](0013-annotation-storage-and-ux.md) (2026-08-26) release leaves the
  selection in `SEL` mode and `y` copies, so a drag can also start a
  comment with `c`. The selection stays highlighted until the next click,
  Esc, or a motion.
- What is copied is the **source Markdown** for the selected range: the
  rendered cells map back through their source ranges and the slice of the
  source text from the first selected byte to the last is placed on the
  clipboard. Selecting rendered text therefore yields text that can be pasted
  into an agent chat verbatim. In source view the mapping is the identity.
- The clipboard is reached through OSC 52. No clipboard crate and no
  `wl-copy`/`xclip` subprocess; terminals that do not implement OSC 52 get a
  status-line notice from `--doctor`, not a fallback.
- Keyboard selection (`v` characters, `V` lines; the same key again exits,
  the other switches kind; `x` selects the line and extends down on repeat,
  as in Helix) uses the same selection model and also copies on
  `y`; this is the path annotations take in milestone 3.

### Search

- `/` and `?` are **regex** searches using the `regex` crate (approved in
  [0001](0001-dependency-policy.md)). Matching runs over rendered text so
  what the user sees is what matches.
- Incremental: the view jumps to the first match as the pattern is typed.
  Smart case: case-insensitive unless the pattern contains an uppercase
  letter. Wrap-around with a status message. All matches are highlighted
  (theme key `ui.search.match`) until Esc or `:noh`.
- An invalid pattern shows the regex error in the status line and leaves the
  view where it was.

### Source view

- `Space v s` toggles between rendered Markdown and the raw source
  (`:source` does the same); the cursor keeps its source line across the
  toggle. The original `gs` alias was removed on 2026-09-14 to reserve
  `g` for navigation.

### Width and wrapping

- Prose wraps to the full pane width minus the gutter. There is no maximum
  measure and no centring; the user sizes the pane.

### Live reload

- On a change to the file on disk the document is re-parsed and re-laid-out
  at the current width. The cursor's **source line and column** are
  remembered and it is placed on the rendered line for that source line, or
  the nearest rendered line that still has a source range if the line is
  gone. The viewport is scrolled so the cursor keeps its previous screen row
  when possible.

### Quitting and Esc

- Only `:q` quits, as in vim/helix; `Ctrl-c` does nothing (dropped 2026-08-26). `q` is reserved. Esc clears, in order, the
  active input line, a pending key sequence, the selection, and search
  highlights; it never quits.
- Amended 2026-09-03: **Esc leaves, toggles close.** Esc in the thread
  pane or the file-threads pane hands the keys back to the text and
  leaves the pane where it is; the `Space` key that opened a pane closes
  it when the pane has focus (`Space a`, `Space t`), and focuses it when
  it is open without focus. The thread list is the text column's
  replacement, not a side pane, so Esc closes it and `Space A` toggles
  it. `Left` and `Right` mean `h` and `l` on every surface
  ([0046](0046-one-thread-cursor.md)).
- Amended 2026-09-04 by [0049](0049-inline-threads-and-the-rail.md): the thread pane
  and the file-threads pane are gone. A thread shows as a stub under its
  lines and `c` expands it in place; the left column is the **rail**
  with a tree pane and a threads pane, shown and hidden by
  `Space e`/`E` and `Space t`/`T`. The pills are `TREE`, `THREADS`,
  `REVIEW`, and `CHECK`; Esc in a rail pane returns to the text and
  leaves the pane where it is.

## Consequences

- The layout structure from [0004](0004-markdown-rendering.md) must expose,
  per rendered line, the source byte range and per rendered span its source
  range, so column-level selection can map back to source bytes.
- Line numbers require the layout to know the source line of a byte offset;
  `fathomable-core` keeps a line index for the document.
- `regex` joins the dependency set; the 0001 table is amended.
- Themes need the keys named above; the full vocabulary and file format are
  in [0011](0011-theme-schema.md).
