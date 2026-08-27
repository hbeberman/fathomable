---
type: Decision
title: Syntax highlighting and source files
description: syntect highlighting for fenced code blocks and whole non-Markdown files, the Markdown file list, and how colour crosses the core boundary.
resource: crates/fathomable-core/src/highlight.rs
tags:
  - decision
  - rendering
  - configuration
---

# 0016 Syntax highlighting and source files

Status: accepted (2026-08-26)

## Context

[0004](0004-markdown-rendering.md) decided code blocks are highlighted with
`syntect`, [0001](0001-dependency-policy.md) approved the crate, and
[0011](0011-theme-schema.md) gave themes a `code.syntect` key, but nothing
read it: the layout emitted `Face::CodeBlock` spans without a language and
the fence info string was discarded. Separately, every file the workspace
tree opened went through the Markdown renderer, so a `.rs` file rendered
`#[derive]` as a heading. Milestone 7 closes both. The choices below were
settled in a question round on 2026-08-26.

## Decision

### Highlighter

- `fathomable-core` gains a `highlight` module wrapping syntect's bundled
  syntax set (`default-fancy`, no `onig`) and bundled theme set.
  `Highlighter::new(name)` loads both once at startup; `Highlighter::plain()`
  never colours and is what layout tests use.
- A language **hint** is either a fence info string (`rust`, `py`) or a file
  extension (`rs`); syntect resolves both by token. An unknown hint yields no
  highlighting and the text renders as today. The miss is logged once per
  hint at debug level so the file log can answer "why is this plain".
- A `code.syntect` name that syntect does not bundle is a theme error with a
  file location, as every other bad theme value in 0011. `--doctor` lists
  the bundled names. An empty `code.syntect` means no highlighting.
- Only the **foreground** of a syntect style is used. The code block
  background and modifiers come from `markup.raw.block` (or `ui.text` for a
  source file) so 0004's transparent dark theme keeps showing through.
  Syntect's own background and font styles are ignored.

### Crossing the core boundary

- `layout::Style` gains `fg: Option<theme::Color>`, an override the frontend
  applies on top of the face's theme style. `Face` is unchanged; a
  highlighted span is still `Face::CodeBlock`, so selection, search, and
  annotation faces layer as before.
- `Layout::render_with(text, width, &Highlighter)` and
  `Layout::source_with(text, width, hint, &Highlighter)` are the highlighted
  entry points; `render` and `source` keep their signatures and use the
  plain highlighter. The diff view is not highlighted.
- Highlighting runs inside layout, per relayout, from the start of the block
  or file, so syntect's state machine is always correct; there is no cache.

### Markdown versus source files

- `config.kdl` gains a `markdown` block naming which files render as
  Markdown. Every other file opens in the source layout, highlighted by its
  extension, and the existing source toggle (`gs`) still flips either kind.

  ```kdl
  markdown {
      extensions "md" "markdown" "mdx"
      extensionless #true
  }
  ```

  `extensions` are matched case-insensitively without the dot;
  `extensionless` says whether files with no extension (README, LICENSE)
  render as Markdown; dotfiles (`.gitignore`, `.env`) never do, as their
  "extension" is their whole name. Both keys are optional; the defaults above are
  echoed by `--config-show`. Unknown keys are errors as in
  [0008](0008-configuration-format.md).
- The welcome document and a file passed on the command line follow the
  same rule.

## Consequences

- `syntect` joins `fathomable-core`'s dependencies, closing the milestone-1
  TODO in [parked ideas](../parked.md). Startup pays for loading the syntax
  and theme sets once.
- syntect's bundled dumps depend on `bincode` 1.3.3, which carries an
  "unmaintained" advisory (RUSTSEC-2025-0141) but no vulnerability;
  `deny.toml` ignores that one id with a reason, amending
  [0001](0001-dependency-policy.md).
- Large source files are re-highlighted on every resize and reload; if that
  shows up, a per-document highlight cache is the next step and needs no
  API change since `Style::fg` is already the carrier.
- `config.kdl` grows its third node; `Config` gains `markdown()`.
- Inline code highlighting, `.tmTheme` loading, and horizontal scroll for
  code blocks stay parked.
