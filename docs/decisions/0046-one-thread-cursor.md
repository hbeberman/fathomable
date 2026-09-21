---
type: Decision
title: One thread cursor
description: Thread actions share one contract while each visible surface retains the logical thread and message it owns.
resource: crates/fathomable/src/app/threads/cursor.rs
tags:
  - decision
  - annotations
  - input
---

# 0046 One thread cursor

Status: accepted (2026-09-03)

Workspace traversal amended 2026-09-18 by
[0090](0090-direct-workspace-navigation.md): `Tab`/`Shift-Tab` directly
cycle active and resolution-proposed threads from every normal pane.
Resolved and archived threads are excluded. File-local and workspace
bracket traversal retire; list-local `j`/`k` movement remains.

Surface ownership amended 2026-09-20 by
[0090](0090-direct-workspace-navigation.md): File, main Threads, and Thread
list retain separate logical thread/message seats. Shared actions still use
one implementation, but resolve their target from the surface that dispatched
them.

Main Threads movement amended 2026-09-20: `j`/`Down` and `k`/`Up` walk its
visible review order message by message and continue across thread and file
boundaries. On a selected thread, `h`/Left collapses and `l`/Right expands;
they do nothing on file-group rows. `Tab`/`Shift-Tab` remain the direct
open-thread traversal. File retains ordinary cursor movement.

## Context

Three surfaces showed threads and each kept its own idea of which one:
the thread pane held a thread id and a selected message, the thread
list held another pair, and the file-threads pane derived its highlight
from whichever of the two was open, else from the text cursor. The
pane also carried a `Tab` scope, `local` or `global`, with a remembered
selection per scope, so `h` in the pane could mean two things and the
header said which. Every surface had its own reply, edit, resolve, and
delete methods that did the same thing to a different field. The review
of 2026-09-03 counted the duplicates and found the surfaces could
disagree: opening the list forgot the pane, and the pane's scope
survived a jump the list made.

The same review settled the paging keys: `PgUp` and `PgDn` are not on
the user's keyboard, and the thread surfaces had no `gg`, `G`, or
`Ctrl-d`. The `x` that resolved a thread in the pane was the `x` that
selects a line in the text, one focus away.

## Decision

- **One cursor.** `App` holds a `ThreadCursor`: a thread and a message
  index (zero for the comment, then the replies). The thread pane shows
  the cursor's thread and highlights its message; the file-threads pane
  highlights the cursor's thread among the file's; the thread list
  highlights the cursor's message. The pane keeps only its scroll, the
  list its open flag, filter, folds, and scroll, the file-threads pane
  its height.
- **The text drives it once the reader moves.** While the thread pane
  or the thread list is open, the cursor is what the last motion, click,
  `c`, or `Space a` set, and the text follows it. With both closed the
  cursor keeps that thread as long as the text cursor rests on the row
  the motion left it on — two threads folded into one rendered
  Markdown row are still told apart — and once the reader moves it
  rides the text cursor: the thread starting on the cursor line, else
  the first on its row, else the nearest starting above, at its newest
  message. Reading a file therefore walks the file-threads pane, and a
  pane opened with `Space a` opens where the reader is. Closing the
  list keeps its place only until the reader moves in the text.
- **One action set.** Reply, edit, resolve or reopen, arm delete, open
  in the file, step a thread, step a message, first and last message
  are methods on `App` that act on the cursor. The binding table maps
  the same `Action` from every thread surface to them; the per-surface
  copies are gone.
- **The owning surface chooses the cursor (2026-09-20 amendment).** File
  actions use the thread/message seated in File, main Threads actions use its
  retained selection or temporary reveal, and Thread-list actions use its
  independent sidebar selection. A retained Thread-list seat is actionable
  only while its thread belongs to the pane's current file/workspace scope and
  lifecycle filter. File, scope, and filter changes reconcile it to an
  admitted displayed thread, or to no target when the pane is empty; focusing
  the pane and dispatching any action repeat that check. A focus-only
  transition does not otherwise copy one surface's cursor over another.
  Keyboard, context-menu, app-menu, footer, mouse, lifecycle,
  reply/edit/delete, navigation-history, and permission routes all resolve
  through that same ownership rule.
- **File inference is not a traversal seat (2026-09-20 amendment).** A pinned
  File cursor or a cursor physically resting in an expanded message is an
  explicit thread selection. Ordinary source can still infer a nearby thread
  for actions, but `Tab`/`Shift-Tab` use the source position as an insertion
  point: forward reaches the first thread at or after it and backward reaches
  the last thread before it. Passive File resize, rewrap, and delayed syntax
  completion restore only File's logical message seat; they never update main
  Threads or Thread-list cursors even when one of those surfaces owns focus.
- **Lowercase steps in the file, uppercase across the workspace.**
  `]c` / `[c` in the text and `l` / `h` in the thread pane step to the
  next or previous thread of this file, wrapping. `]C` / `[C` and
  `L` / `H` step across the workspace: files in path order, threads in
  line order, wrapping, opening the file the thread is in. From the
  text with the pane closed, a step is relative to the cursor line, so
  `[c` reaches the thread the reader is below; from a thread surface it
  is relative to the highlighted thread. The pane follows a step from
  the text only when it is already open; `]r` keeps opening it, since
  reading the reply is that motion's point. `h` on the first thread of
  the file, in the pane, hops to the file-threads pane, as `h` at column
  0 hops to the tree. The `Tab` scope, its notice, and its remembered
  selections are gone.
- **The header counts both ways.** `thread 2/5 in file · 7/40 overall`,
  then the lines, the placement and state words, and the watchers.
- **`o` resolves.** `o` toggles the cursor's thread between open and
  resolved on every thread surface; `x` is the text's select-line key
  only. `]o` / `[o` remain the opened-file history: a prefixed pair
  in the text focus does not collide.
- **Paging without `PgUp` / `PgDn`.** `Ctrl-d` / `Ctrl-u` scroll the
  thread pane by half its rows and move the list's highlight by half a
  page of rows; `gg` / `ge` / `G` go to the first or last message in the
  pane and the first or last thread in the list; `Alt-j` / `Alt-k` (and
  `Alt-Down` / `Alt-Up`) scroll the thread above the comment box. The
  `PageUp` and `PageDown` keys are removed from the key type, so a
  binding cannot name them.
- **Arrows equal `h` / `l`.** `Left` and `Right` are bound beside `h`
  and `l` in the thread pane and the list; the pane's `Left` no longer
  has a meaning of its own.

## Consequences

- On first focus a surface may inherit the currently visible logical seat;
  after deliberate selection it retains its own. Previewing a thread can
  deliberately synchronize the destination surface without making unrelated
  focus changes overwrite either selection.
- Main Threads' `j` / `k` walk its visible review order, while
  `Tab` / `Shift-Tab` walk only open threads in the current view.
- A reply sent from any surface moves the cursor to the new message,
  as the pane and list each did before.
- [0007](0007-key-grammar-and-mouse.md) is amended: the lateral
  contexts share a cursor and `Tab`, `PageUp`, `PageDown`, and the pane's
  `Left` are gone. [0025](0025-thread-list.md),
  [0027](0027-revisiting-threads.md), and
  [0034](0034-deleting-threads.md) are amended by this record where
  they describe a per-surface selection or the `Tab` scope.
- Amended 2026-09-04 by [0049](0049-inline-threads-and-the-rail.md): the thread pane
  and its `h`/`l`/`H`/`L` are gone; the cursor keeps its meaning and an
  expanded thread's message rows, walked by `j`/`k` in the text, are
  where its message index shows. `]o`/`[o` retire with the history.
