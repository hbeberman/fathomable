---
type: Decision
title: z folds and unfolds
description: In the text `z` opens and closes threads, `Z` folds or expands the file, Enter toggles a header under the cursor without a hint, and `c` replies from a thread row or otherwise starts a comment without changing expansion.
resource: crates/fathomable/src/app/threads/fold.rs
related_resources:
  - crates/fathomable/src/app/threads/stubs.rs
  - crates/fathomable/src/app/input/bindings.rs
  - crates/fathomable/src/app/draw/header.rs
tags:
  - decision
  - annotations
  - input
---

# 0065 z folds and unfolds

Status: accepted (2026-09-05). Amended 2026-09-05 by
[0066](0066-one-circle-language.md): in the review list and the threads pane
`z` folds the cursor's file and `Z` every file. Amended 2026-09-05 by
[0067](0067-the-texts-key-bar.md): the hints `z expand` and `z fold`
are on the text's key bar, not the stub or the header. Amended
2026-09-16: `c` no longer expands, folds, or cycles threads; it starts
a comment from source text and replies from a thread row. Standalone
`C` and the standalone `r` reply binding are unbound. Amended
2026-09-16: Enter folds or unfolds a thread only while the text cursor
rests on its expanded header or folded stub, without adding a hint.

Amended 2026-09-19: pane-independent thread workflows use `Space t`;
the explicit new-thread and reply routes are `Space t c` and `Space t r`.

Amended 2026-09-20: navigation may temporarily reveal a selected folded
thread without changing these persistent fold choices. Any explicit fold
action takes ownership of the effective visible state.

## Context

[0049](0049-inline-threads-and-the-rail.md) made `c` the key that
expands a stub in place, folds the expanded thread, and walks on to
the next thread covering the line, and the same `c` comments on a line
that has no thread. The one key does three things, and which one it
does depends on the row and on how many threads cover it: a reader who
wants an open thread closed, and nothing else, has to know that `c`
folds it and then opens the next one. The review list has had `z` for
the same act since 0049, folding the selected entry to its header and
unfolding it again, and `Space c z`, which once expanded or folded the
whole file, was unbound the same day because nobody reached for a
chord. The user asked for `z` and `Z` in the text: fold and unfold the
thread here, and every thread in the file.

## Decision

- **`z` opens and closes one thread.** On an expanded thread's rows it
  folds that thread back to its stub. On a row a thread covers it
  expands the thread cursor's thread, the one the stub hint marks.
  Elsewhere it does nothing. It never cycles and never starts a
  comment. (Amended 2026-09-10: a fold from the thread's rows leaves
  the cursor on the stub, the row the header becomes, where the
  terminal cursor stays hidden and the stub's `▎` bar marks the place.
  The relayout had dropped it to the first column of the line the
  thread hangs under, a jump the user saw on every fold by `z`, the
  chevron, or a double-click; a cursor on a thread's rows now keeps
  its seat on that thread through any relayout, on its row or the stop
  before it, and a cursor resting on a stub or a header stays there
  while other threads change. [0073](0073-the-chevron.md) carries the
  same note. Since 2026-09-11 `j`/`k` stop on a stub too,
  [0076](0076-threads-fold-in-the-list.md).)
- **`Z` opens and closes the file.** It expands every stub in the file,
  or, when any thread is expanded, folds every one.
- **Enter activates a heading.** While the text cursor rests on an
  expanded thread header, Enter folds it to its stub; on that folded
  stub, Enter expands it again. The cursor stays on the heading in
  either form, so repeated presses toggle it in place. Enter does
  nothing on source or message rows. It is not added to the key bar:
  `z` remains the visible, context-independent fold key.
- **`c` writes.** It replies when the cursor rests on a collapsed stub
  or an expanded thread's rows. On a selection or source line it starts
  a new thread, whether or not another thread covers that line. It never
  changes thread expansion. Standalone `C` and standalone `r` are
  unbound; `Space t c` and `Space t r` remain the pane-independent
  explicit commands.
- **The hints name `z`.** A stub's last row ends with `(z expand)`, the
  thread key bar reads `c reply · e edit · o resolve · z fold` while
  the cursor rests in the thread's rows, and the
  right-click menu's `expand thread` / `fold thread` entry shows `z`.
  Under [0064](0064-hints-you-can-press.md) the hint is the key that
  does only that.
- The review list keeps its `z`; the two surfaces agree on the letter.
- **Navigation peeks are not folds.** A direct thread landing may reveal the
  selected File thread despite its persistent fold, and may override hidden
  stubs only for that selected thread. Local reading and focus changes keep
  the peek. A successful destination change dismisses it; a failed or
  no-target action does not. `z`, `Z`, Enter, chevrons, double-click, and
  menu folding first release the peek and then apply the requested effective
  state, so later navigation cannot undo that explicit choice.

## Consequences

- `Action::Fold` is bound on the text as well as the review list,
  `Action::FoldAll` covers the file, and `Action::Confirm` handles a
  heading under the text cursor; `App::act` gives each surface its
  meaning.
- `toggle_thread_header`, `toggle_thread_here`, and
  `toggle_expand_all` live in `app/threads/fold.rs`. The old `c` cycle
  and its session state are removed.
- `docs/guide.md` names Enter on a heading, `c` as comment or reply,
  and `z` as fold or unfold.
