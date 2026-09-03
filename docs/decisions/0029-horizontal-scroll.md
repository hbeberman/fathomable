---
type: Decision
title: Horizontal scroll for long lines
description: Unwrapped code lines scroll sideways with zl zh zL zH and the horizontal wheel; the offset is per document, clamped to the widest line, edge markers show what is cut, and search keeps its match in view.
tags:
  - decision
  - input
  - rendering
---

# 0029 Horizontal scroll for long lines

Status: superseded by [0044](0044-wrap-all-lines.md) (2026-09-02)

## Context

[0004](0004-markdown-rendering.md) leaves code lines unwrapped — a
fenced block in Markdown, and every line of a source file or the raw
source view of [0016](0016-syntax-highlighting.md) — and says the
frontend truncates or scrolls them. Only truncation shipped. In the
narrow pane Fathomable is meant to live in, a 120-column line loses
its tail, and there is no way to see it. Settled 2026-08-28 without a
question round; the choices follow Vim's `nowrap` keys.

## Decision

- **Keys.** `zl` and `zh` scroll one column right and left; `zL` and
  `zH` scroll half the text width. A count applies (`10zl`). The
  offset is per open document, starts at 0 when a file is opened, and
  survives reloads and diff toggles. There is no column cursor: the
  offset is view state like the vertical scroll.
- **Mouse.** Horizontal wheel events (`ScrollLeft`/`ScrollRight`, which
  terminals send for a trackpad or Shift+wheel) scroll four columns
  per tick over the text pane.
- **What moves.** Only unwrapped lines shift: code block lines, source
  lines, and table rows wider than the pane. Wrapped prose, headings,
  the gutter, line numbers, thread brackets, and the diff markers stay
  where they are. In a Markdown document a code block therefore
  scrolls under prose that does not move, which is what the reader
  asked for and no worse than the alternatives.
- **Bounds.** The offset clamps to the widest unwrapped line in the
  document minus one, so the last column can always be reached and
  the view cannot scroll into nothing; `zh` at 0 is a no-op and a
  document with no line wider than the pane ignores the keys with no
  notice.
- **Edge markers.** A shifted line that is cut on the left shows a dim
  `‹` in its first cell; one cut on the right shows `›` in its last
  cell, both in the faint face over whatever face the cell had.
  Unshifted, truncated lines show `›` too, so a cut is visible before
  the reader knows to scroll. Selection and copy read the full source
  line, never the visible slice.
- **Search.** `n`, `N`, and a fresh `/` adjust the offset the least
  amount that brings the match's first cell into view, keeping a
  four-column margin; a match on a wrapped line does not move it.
  `:N` and thread jumps leave the offset alone.
- **Status.** `:status` shows `col N` when the offset is not 0.

## Consequences

- `app/hscroll.rs`, which this record backs, holds the offset per
  document, the clamp against the layout's widest unwrapped line, and
  the key and wheel handling; `ui` slices unwrapped rows by the offset
  (by display cells, not bytes, so wide characters are cut whole) and
  paints the edge markers. The layout engine exposes the widest
  unwrapped width per document so the clamp needs no second pass.
- `z` becomes a prefix in the view; the thread list's `z`/`Z` folds
  are unaffected since they act in a different focus.
- `docs/guide.md` gains the keys and the marker legend. The parked
  "horizontal scroll for code blocks" TODO is removed.
