---
type: Decision
title: Syntax highlighting and source files
description: syntect highlighting for fenced code blocks and whole non-Markdown files, the Markdown file list, and how colour crosses the core boundary.
resource: crates/fathomable-core/src/highlight.rs
related_resources:
  - crates/fathomable/src/app/highlight.rs
tags:
  - decision
  - rendering
  - configuration
---

# 0016 Syntax highlighting and source files

Status: accepted (2026-08-26), amended 2026-08-27 (the extensionless rule),
amended 2026-09-17 (asynchronous source highlighting),
amended 2026-09-18 (rendered-view eligibility)

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
- `Highlighter::highlight` returns opaque `Highlights`, and
  `Layout::source_with_highlights` applies that result at any width.
  Source views cache one result per immutable text generation, so resizing
  rewraps coloured spans without rerunning syntect. Fenced Markdown blocks
  continue to highlight inside rendered layout from the start of each block.

### Responsive source loading

- A source file first lays out plain text at its real pane width. It does not
  construct a throwaway one-column layout or wait for syntect before the
  event loop can draw.
- One dedicated worker highlights immutable source generations away from the
  terminal event loop. Its completion carries a stable document identity and
  generation; a result for a removed or reloaded document is discarded.
- The worker processes one file at a time. Before starting its next file it
  keeps only the newest queued preview, so rapidly paging the files pane
  cannot create a growing CPU backlog. A skipped document remains eligible
  and is queued again if the reader returns to it.
- Applying highlights only rebuilds width-dependent spans. An unchanged
  comparison projection, source text, or pane width keeps the existing
  cached layout.

### Markdown versus source files

- `config.kdl` gains a `markdown` block naming which files render as
  Markdown. Every other file stays in the source layout, highlighted by its
  extension. The source/rendered toggle (`Space v s` or `:source`) is
  available only for files matched by this configuration.

  ```kdl
  markdown {
      extensions "md" "markdown" "mdx"
      names "README" "LICENSE" "LICENCE" "COPYING" "CHANGELOG" \
            "CONTRIBUTING" "AUTHORS" "NOTICE"
  }
  ```

  `extensions` are matched case-insensitively without the dot; `names`
  lists the extensionless file names, matched case-insensitively, that
  are prose. Every other extensionless file (`justfile`, `Makefile`,
  `Dockerfile`, dotfiles such as `.gitignore`) is source. Both keys are
  optional; the defaults above are echoed by `--config-show`. Unknown keys
  are errors as in [0008](0008-configuration-format.md).

  *Amendment (2026-08-27).* The first cut had an `extensionless #true`
  switch that sent every non-dotfile without an extension through the
  Markdown renderer, so a `justfile` lost its blank lines and had its
  recipes fused into paragraphs. Prose files without extensions are a
  short, well-known set; build files without extensions are open-ended,
  so the allow-list replaced the switch. `extensionless` is no longer a
  key and is rejected as unknown.

  *Amendment (2026-09-18).* The same classifier governs both initial
  display and permission to enter rendered view; it is not merely a default.
  Custom extensions and extensionless names remain eligible, while excluded
  files cannot enter the Markdown renderer through a menu, keyboard, command,
  or direct view toggle. Ineligible menu actions are disabled; direct actions
  report the restriction without changing layout or highlighting. Eligible
  files retain their own source/rendered choice across file switches.
  Unified diffs continue to disable this toggle while retaining that choice.
- The source layout's language hint is the extension, or for a file with
  none its file name, so syntect's own file-name matches (`Makefile`,
  `GNUmakefile`, `Rakefile`) apply; `justfile` is mapped to the make
  grammar, the closest bundled one. `language_hint` in `highlight` owns
  this mapping.
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
- First presentation of a source file is plain for the short interval before
  its syntax colours arrive. Whole-file syntect cost no longer blocks input,
  and revisits, comparison projection, and width changes reuse the cached
  result until the text changes.
- `config.kdl` grows its third node; `Config` gains `markdown()`.
- Inline code highlighting, `.tmTheme` loading, and horizontal scroll for
  code blocks stay parked.
