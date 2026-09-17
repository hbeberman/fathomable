---
type: Decision
title: Width-bounded wrapping in every display mode
description: Wrap prose at words and hard-wrap code, source, diff, and table lines so the viewer never hides text past the pane edge.
resource: crates/fathomable-core/src/layout/wrap.rs
tags:
  - decision
  - input
  - rendering
---

# 0044 Width-bounded wrapping in every display mode

Status: accepted (2026-09-02); amended 2026-09-17 so fenced blocks in
thread messages prefer whitespace while file code remains hard-wrapped.

## Context

Rendered Markdown prose wrapped, but fenced blocks stayed on one visual row.
Verbatim descriptions and other prose carried in fences were therefore cut at
the pane edge unless the reader scrolled sideways. Source files, diffs, and
very narrow tables had the same split interaction. The reader chose one
consistent rule: long lines wrap in every display mode.

## Decision

- Every layout line is bounded by the text pane width. Prose continues to wrap
  at whitespace; fenced code in files, source files, diffs, and table fallback
  rows hard-wrap between grapheme clusters. A fenced block in a thread message
  prefers whitespace and hard-wraps only a token wider than an empty row,
  keeping prose and replacement examples readable in the narrower review lane.
- Styled runs and source byte ranges survive wrapping. Only the first visual
  row from a source line gets its gutter number.
- Nested code repeats its quote or list prefix on continuation rows. A wrapped
  diff keeps its sign on the first row and aligns continuation rows under the
  content with a blank sign cell.
- A table keeps its normal rectangular layout when its minimum columns fit.
  At narrower widths its rendered rows hard-wrap rather than becoming
  horizontally scrollable.
- Thread messages use the same Markdown renderer, so fenced blocks wrap there
  too.
- Horizontal-scroll state, `zl`/`zh`/`zL`/`zH`, the horizontal-wheel binding,
  cut-edge markers, and the `col N` status field are removed. Horizontal wheel
  events have no viewer action.

## Consequences

- Text is reachable with vertical movement alone, including in narrow panes.
- A wrapped source or code line occupies more visual rows; cursor motion,
  search, selection, annotations, and reload anchoring continue to use the
  existing source ranges.
- Hard wrapping can split a source token or a very narrow table border, but it
  never alters copied source text. Message fences avoid that split when a
  whitespace boundary is available.
- This supersedes [0029](0029-horizontal-scroll.md) and its code-block
  exception in [0004](0004-markdown-rendering.md) and
  [0037](0037-markdown-in-threads.md).
