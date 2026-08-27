---
type: Decision
title: Viewer UX conventions
description: Cursor, gutter, status line, selection-to-clipboard, search, and reload behaviour of the rendered view, modelled on Neovim and Helix.
resource: crates/fathomable/src/app/view.rs
related_resources:
  - crates/fathomable/src/app/clipboard.rs
tags:
  - decision
  - input
  - rendering
---

# 0010 Viewer UX conventions

Status: accepted (2026-08-26)

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
- `scrolloff` is 3 rendered lines. `Ctrl-d`/`Ctrl-u` move half a page,
  `gg`/`G` go to the first and last rendered line.
- Synthesised lines (table rules, block spacing) are valid cursor rows; they
  simply carry no source range.

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
- `:` and `/` input replace the status line while active, as in Vim. Pending
  key sequences and transient messages ("search hit BOTTOM, continuing at
  TOP", "copied 3 lines") use the same line and clear on the next key.

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

- `gs` toggles between rendered Markdown and the raw source (`:source` does
  the same); the cursor keeps its source line across the toggle. The key was
  chosen during implementation (2026-08-26) as an unused `g` prefix; change
  it here if a better one emerges.

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

- Only `:q` and `Ctrl-c` quit. `q` is reserved. Esc clears, in order, the
  active input line, a pending key sequence, the selection, and search
  highlights; it never quits.

## Consequences

- The layout structure from [0004](0004-markdown-rendering.md) must expose,
  per rendered line, the source byte range and per rendered span its source
  range, so column-level selection can map back to source bytes.
- Line numbers require the layout to know the source line of a byte offset;
  `fathomable-core` keeps a line index for the document.
- `regex` joins the dependency set; the 0001 table is amended.
- Themes need the keys named above; the full vocabulary and file format are
  in [0011](0011-theme-schema.md).
