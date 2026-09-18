---
type: Decision
title: Hints you can press
description: A key hint is drawn only where pressing it now runs the named action; thread and comparison hints follow current focus, and header surfaces reach through the gutter.
resource: crates/fathomable/src/app/draw/header.rs
related_resources:
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/threads/cursor.rs
tags:
  - decision
  - rendering
  - input
---

# 0064 Hints you can press

Status: accepted (2026-09-05). Amended 2026-09-05 by
[0067](0067-the-texts-key-bar.md): the rule stands; the thread header's
keys and the stub's `(z expand)` moved to the text's key bar, which
shows the thread cursor's keys while the text has focus.

Amended 2026-09-17: the local diff header retires in favor of the File
surface header. Comparison hints remain governed by this rule on the text
key bar.

Amended later 2026-09-17: key bars read action then hotkey. Each visible
hint's label, gap, and hotkey are one clickable region with the shared hover
background; inter-action separators remain passive.

## Context

[0049](0049-inline-threads-and-the-rail.md) gave an expanded thread a
header row with its keys, `r reply · e edit · o resolve · c fold`, and
[0059](0059-headers-and-the-key-bar.md) put every header on the
`ui.header` surface and moved the review list's keys to a bar on its
bottom row, which already says `click or Space w h to focus` when the
list has no focus. The thread header did not follow: every expanded
thread showed the four keys, though `r`, `e`, and `o` act on the
thread cursor's thread ([0046](0046-one-thread-cursor.md)), which in
the text is the thread under the cursor line, and none of them act at
all while the files pane, the threads pane, or the review list has
focus. `e edit` showed on a message that was not the user's. The diff
header's `h/l page … Esc close` and a stub's `(c expand)` showed with
the focus elsewhere too. The user's rule: if a key hint is on the
screen, pressing that key does what it says; if it would not, the
hint is not drawn.

The same header row also stopped short of the left edge. `with_gutter`
paints the four gutter cells in front of every row of a thread block
on `thread.inline`, so a header row on `ui.header` began at the text
column with a strip of the other colour before it.

## Decision

- **The rule.** A key hint is drawn only where pressing that key now,
  with the focus and cursor as they are, runs the action it names. A
  hint that would not is left out, not dimmed and not replaced by
  directions; a hint of words alone, such as the review bar's
  `click or Space w h to focus`, stays because a click does that.
- **Applied.** The thread header shows its keys only on the thread
  cursor's thread and only while the text has focus; `e edit` only
  when the cursor's message is the user's, as the review bar already
  decides it. Every other expanded thread's header is its words alone.
  The diff header's keys and a stub's `(c expand)` show only while the
  text has focus. (Amended 2026-09-06 by
  [0069](0069-the-diffs-keys-on-the-bar.md): the diff's keys are on
  the text's bar, and `h/l page` is drawn on a checkpoint base only.) The review bar and the draft's row already obey the
  rule and are unchanged.
- **A header reaches the left edge.** A header row inside a thread
  block (the thread header, the draft's `comment on …` row, and the
  draft's author row) paints its gutter cells on `ui.header` as well,
  the mark glyph staying in its cell; message rows keep
  `thread.inline`.

## Consequences

- `expanded_header` takes whether the thread is the cursor's and
  whether the text has focus; `expanded_block_lines` passes them. A
  click on a hint that is not drawn runs nothing, and the header's
  `action_at` follows from what is drawn, so the mouse agrees.
- Comparison hints are omitted from the text key bar when another pane has
  focus; the stub's `hinted` flag requires the text's focus.
- `with_gutter` paints the gutter cells with the row's own style when
  the row carries one, so any header row's surface spans the row.
- This record takes the backlink of `header.rs` from 0059, which keeps
  the file as a forward pointer.
- Every later hint is held to the rule: the binding table says what a
  key is called, and the drawing says whether it works here.
