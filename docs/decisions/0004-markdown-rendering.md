---
type: Decision
title: Markdown rendering and source mapping
description: Own the Markdown layout engine on top of pulldown-cmark so rendered lines map back to source ranges.
resource: crates/fathomable-core/src/layout/mod.rs
related_resources:
  - crates/fathomable-core/src/layout/blocks.rs
  - crates/fathomable-core/src/layout/wrap.rs
  - crates/fathomable-core/src/layout/text.rs
tags:
  - decision
  - rendering
---

# 0004 Markdown rendering and source mapping

Status: accepted (2026-08-26)

## Context

Users annotate the rendered view, but agents need source line ranges. Existing
Markdown-to-ratatui crates discard offsets and are small single-maintainer
projects outside the dependency policy.

## Decision

- Parse with `pulldown-cmark` using `into_offset_iter` so every event carries
  its source byte range. Enable tables, task lists, footnotes, and strikethrough.
- Implement layout in `fathomable-core`: block structure, wrapping to pane
  width, tables, nested lists, task lists, footnotes, inline code, links. Every
  rendered line records the source byte ranges it was produced from.
- Code blocks are highlighted with `syntect` (`fancy-regex` backend, bundled
  syntax set). Highlighting inline code is deferred.
- Links render with OSC 8 hyperlinks so terminals that support them (Ghostty,
  foot, kitty, WezTerm) make them clickable; local links can be opened inside
  Fathomable with a key.
- A source-view toggle shows the raw Markdown with the same annotation
  positions.
- Text wraps to pane width; code blocks never wrap (the frontend truncates,
  horizontal scroll is parked). Vertical motion is by visual line.
- Themes: true-color, using syntect themes for code and a Fathomable KDL
  theme for chrome and Markdown. Ship a default light and a default dark
  theme; the dark theme leaves the background unset so transparent terminals
  show through. A 16-color fallback follows. `--theme NAME` picks a theme per
  run.
- Mermaid stays a code block. Images render as alt text; Sixel/Kitty graphics
  are deferred.

## Consequences

- The layout engine is the largest component and the most test-heavy; it is
  pure data in, data out.
- Rendering is deterministic for a given width, which the anchoring logic
  relies on.
