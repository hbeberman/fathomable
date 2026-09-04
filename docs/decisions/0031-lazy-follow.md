---
type: Decision
title: Lazy follow
description: Auto-jump is a monitor, not a leash; it switches itself off when the reader navigates away, leaves the visible file alone while its hunk is on screen, and prefers the file the agent says it is editing.
resource: crates/fathomable/src/app/autojump.rs
tags:
  - decision
  - input
---

# 0031 Lazy follow

Status: accepted (2026-08-28)

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

[0015](0015-follow-mode.md) shipped auto-jump with a queue debounce and
a fixed set of guards: no jump while a selection, comment box, diff
view, thread pane, or popup is open, or within three seconds of any
key or mouse action. The charter's "follow the agent" promise was
otherwise left as an open investigation in [parked ideas](../parked.md):
how long after the agent touches a file the viewer should move, and how
to avoid moving while the reader is reading.

The guards cannot tell reading from absence. Three seconds of stillness
looks the same whether the reader has walked away or is studying a
hunk, so a slow read of one change is interrupted by the next. Longer
holds and engaged/idle hysteresis were considered on 2026-08-28 and
rejected for a simpler rule from the user: auto-jump is a monitor, and
the moment the reader takes the wheel it steps aside. Re-enabling is
one key.

## Decision

- **Navigation turns auto-jump off.** While `AUTO` is on, a deliberate
  move to somewhere else switches it off: opening a different file by
  the picker, the tree, `]f`/`[f`, `]g`/`[g`, `]r`/`[r`, the thread
  list, or a `:` command; opening a thread pane, the thread list, or the
  diff view; starting a selection or the comment box. The status pill
  drops `AUTO`, a toast reads `auto-jump off`, and the queue, badges,
  and hint are untouched. An auto-jump's own file switch, and an agent
  `open`, do not count.
- **Looking is not leaving.** Scrolling, searching, and cursor motion
  inside the file auto-jump landed on keep it on; they only hold the
  next jump back for the three-second activity window of 0015, which
  stays. So a reader who pages through a hunk and then sits still is
  taken to the next change; a reader who goes to look at something else
  is not.
- **The visible file.** A change to the file on screen re-renders as
  before. Auto-jump then scrolls to its first hunk only when that hunk
  is off screen, under the same debounce and hold; a hunk already in
  view settles the entry with no movement.
- **Bursts.** After the queue has been quiet for `follow.jump-debounce`,
  auto-jump opens the newest entry whose file is on the agent's `follow`
  list ([0014](0014-mcp-server-and-socket-v1.md)); when none is, the
  newest entry, as before. The rest of the burst stays queued for
  `]f`/`[f`.

No configuration changes: `follow.auto` still sets the starting state
and `follow.jump-debounce` the quiet period.

## Consequences

- `app/autojump.rs`, which this record backs, holds the tick, the
  guards, the target choice, and the off-switch; `open`, the thread and
  diff entry points, and selection start call the off-switch, and
  auto-jump's own `open` bypasses it.
- The "Lazy follow heuristics" open investigation leaves
  [parked ideas](../parked.md). Snapshot bounds and the seen heuristic
  stay parked.
- `docs/guide.md`'s auto-jump paragraph is rewritten in the same change.
