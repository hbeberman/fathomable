---
type: Decision
title: Markdown in the thread pane
description: Comment and reply bodies in the thread pane render as Markdown through the file renderer, with a single newline kept as a line break and fenced code coloured by its language.
resource: crates/fathomable/src/app/draw/message.rs
tags:
  - decision
  - annotations
  - rendering
---

# 0037 Markdown in the thread pane

Status: accepted (2026-08-28)

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

Agents answer threads in Markdown: lists of points, `inline code`,
fenced blocks with the code they changed, bold words. The thread pane of
[0013](0013-annotation-storage-and-ux.md) drew every body as plain text
wrapped to the pane, so the reader saw the asterisks and backticks and
the fences, and a code block wrapped like prose. The reader asked
(2026-08-28) for the bodies rendered, and the renderer of
[0016](0016-syntax-highlighting.md) already lays out a Markdown file
with fenced code coloured by its info string.

Settled in a question round on 2026-08-28:

- *Which bodies?* Every one: the user's comments and the agents'
  replies. One code path; a plain sentence renders as itself, so a
  comment that uses no Markdown looks as it did. Rendering agent
  replies only was offered and not taken.
- *Fenced code?* Coloured by the fence language, through the app's
  highlighter, as a file's fences are. A plain code-block face was
  offered; the reader's view was that the pane is not so narrow that
  wrapping is a worry, and a fence is a code block either way.
- *Newlines.* Not asked, found by the tests: a comment typed with
  `Ctrl-Enter` line breaks ([0034](0034-deleting-threads.md)) is one
  Markdown paragraph, and CommonMark lays a single newline out as a
  space, which would have joined the lines. A comment's newline is a
  line break, as GitHub renders comments; a file's is still a soft
  break.

## Decision

- `app/draw/message.rs`, which this record backs, takes `message_lines` and
  `thread_body_rows` from `ui.rs`. A body is laid out by
  `Layout::render_message` at the pane's width less the message indent,
  and each rendered line's spans are drawn through `face_style`, so
  headings, code, links, quotes, and emphasis take the same theme keys
  they do in a file. The row count the pane scrolls by comes from the
  same layout, so the debug assertion of 0034 still holds.
- For an inline expanded thread, `ExpandedLayout` retains every rendered
  message body together with the thread revision and pane width. Stub
  placement computes a missing or invalidated layout once; row counts and
  drawing share it, and the app retains it across draws, draft keystrokes,
  and file switches. A new thread revision or width invalidates it. Fenced
  code highlighting therefore never runs from per-row lookup or drawing.
- `fathomable_core::layout::Breaks` says what a single newline means:
  `Soft` (CommonMark, the default for files) or `Hard`.
  `Layout::render_message` is `render_with` with `Hard`; the block
  parser turns a soft break into a hard one when asked. No other
  rendering changes.
- `App::highlighter()` exposes the shared highlighter of 0016 so the
  pane colours fences the way the view does.

## Consequences

- A reply that says `**Fixed** in \`foo\`:` followed by a fence reads
  as it was meant to; an agent's list is a list.
- Code-block lines are unwrapped in the renderer
  ([0029](0029-horizontal-scroll.md)); the file view scrolls them
  sideways, the pane clips them at its right edge. Sideways scrolling
  in the pane is left until it is missed.
- A comment can now contain Markdown by accident: a line starting with
  `#` becomes a heading, `*` a bullet. The source text is unchanged in
  `threads.jsonl`; only the drawing differs.
- The compose box still shows what is typed, not a rendering.

The unwrapped code-block consequence above was superseded by
[0044](0044-wrap-all-lines.md): code blocks wrap in both files and messages.

The review list of [0025](0025-thread-list.md) had gone on wrapping
bodies as plain text; since 2026-09-09 its body rows come from the same
`Layout::render_message`, so a thread reads the same there as expanded
in its file.

[0071](0071-author-stripes.md) gave the expanded thread a two-cell gutter
of its own, so the message indent is four cells and a body wraps two
cells narrower; each message's rows sit on its author's stripe.
