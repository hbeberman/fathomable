---
type: Decision
title: Deleting threads and the list that drives the pane
description: d d deletes a thread from the thread pane, the file-threads pane, or the thread list, recorded as a tombstone; the file-threads pane drives the thread pane without taking focus from it; and the pane opens at its end, marked END.
resource: crates/fathomable/src/app/delete.rs
tags:
  - decision
  - annotations
  - input
  - rendering
---

# 0034 Deleting threads and the list that drives the pane

Status: accepted (2026-08-28); amended 2026-09-03 by
[0046](0046-one-thread-cursor.md): the three surfaces arm and delete the
one thread cursor's thread, and `h` on the file's first thread, not
`Left`, hands the keys back to the file-threads pane.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

A thread, once written, could be resolved but never removed: a comment
made on the wrong line, or a test of the keys, stayed in the store and
the lists for good. The reader asked (2026-08-28) for deletion with a
confirmation that costs one key and cancels on any other.

The same request reshaped how the file-threads pane of
[0027](0027-revisiting-threads.md) and the thread pane of
[0013](0013-annotation-storage-and-ux.md) relate. A click on the
file-threads pane opened the thread pane *and* focused it, so reading
several threads in turn meant clicking, reading, and clicking again;
`j`/`k` in the pane moved the cursor but left the thread pane on
whatever it showed. The reader wants to walk the list and watch the
pane change, stepping into the pane only to scroll or reply. And the
pane opened at its top, on the snippet, when what is new in a thread is
at its bottom.

Settled in a question round on 2026-08-28; the choices are recorded
below.

## Decision

### The store

- `Event::Delete { thread, created }` is a tombstone. On load the
  thread is dropped and its id remembered, so a later event on it — a
  headless `--mcp` reply that raced the deletion — is ignored rather
  than failing the whole file as an unknown thread. The file stays
  append-only ([0032](0032-placement-and-state.md)); rewriting it was
  rejected for breaking the one-write contract the watcher relies on.
- A deleted thread is gone from every reader: the marks, the panes,
  the thread list, `annotations_list`, and the waiting count. There is
  no MCP tool to delete; only the user deletes. Undeleting from the
  tombstoned history is noted in `.todo.md`, not designed here.

### `d d`

- `d` in the thread pane, the file-threads pane, or the thread list
  arms deletion of the thread that surface is on: the shown thread,
  the highlighted one, the selected entry. The status line reads
  `d again to delete this thread · any other key cancels`.
- A second `d` deletes. Any other key cancels, is *swallowed* (it is
  not also acted on, as Helix drops a pending chord), and the status
  line reads `delete cancelled`. A click cancels the same way and is
  then handled. The arming is one field on the app, not a mode: it
  holds the thread id, so a store reload that removes the thread
  between the two keys makes the second `d` a no-op.
- After deletion the thread pane, when it showed the thread, moves to
  the next thread in the file as `n` would, and closes when the file
  has no other thread. The file-threads pane's highlight follows the
  cursor as it always did; the thread list keeps its position, taking
  the entry that now sits there.

### The file-threads pane drives the thread pane

- While the file-threads pane has focus, the thread pane is open on
  the highlighted thread: `Space t` and a click on the pane's header
  open it, `j`/`k`, the wheel, and a click on a row change it. None of
  these move focus; the pill stays `FILE`.
- `j`/`k` step by entry, not by line start: two threads that begin on
  one line are two steps, where 0027's line-based step collapsed them.
  The cursor still moves to each thread's first line.
- `Enter`, `l`, and `Right` in the file-threads pane hand focus to the
  thread pane. `Left` in the thread pane hands it back, showing
  the tree first when it was hidden; with no threads pane to go to they
  do nothing. (`h` became previous-thread paging on 2026-08-30.)
- `Esc` in the file-threads pane closes the thread pane and returns
  the keys to the text, so the two surfaces the list opened go away
  together. A reply from the file-threads pane keeps the thread pane
  up, which now shows the reply.

### The pane opens at its end

- The thread pane opens scrolled so its last row and newest selected
  message are visible, and
  returns there when the shown thread gains a reply. The scroll is kept
  only while the same thread is shown and nothing was added.
- The body ends with a dim `─── END ───` row after the last message,
  visible only when scrolled fully, so the reader can tell the end from
  a pane that has more (`▼ N more`) without counting.
- The scroll limit is computed from the same wrapping the drawing
  uses, at the text column's width, so `k` from the bottom moves at
  once. 0013's clamp to the unwrapped line count went with it.

## Consequences

- `app/delete.rs`, which this record backs, holds the arming state,
  the confirm/cancel step the three key handlers share, and
  `delete_thread`. `Store::delete` and `Event::Delete` join
  `annotations.rs`; the wire version stays 2, since an older reader
  never sees the event as anything but an unknown one it already
  rejects.
- 0027's "a click on a row opens the thread pane, as `Enter` does" and
  its `Enter` are superseded as above; 0013's pane opens at its end.
  `docs/guide.md` gains `d d`, `l`/`h`, and the END marker in the same
  change.
