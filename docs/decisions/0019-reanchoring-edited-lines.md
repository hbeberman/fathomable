---
type: Decision
title: Re-anchoring edited lines
description: How a thread follows a rewrite of the lines it was written on, how that is shown and stored, and what still detaches.
resource: crates/fathomable-core/src/reanchor.rs
tags:
  - decision
  - annotations
  - rendering
---

# 0019 Re-anchoring edited lines

Status: accepted (2026-08-27)

## Context

[0005](0005-annotations.md) anchors a thread by content hashes and shows
it *detached* when the exact lines are gone; [0013](0013-annotation-storage-and-ux.md)
fixed the hash and the search but left "smarter re-anchoring for modified
lines" as an open investigation in [parked ideas](../parked.md). In use
that is the common case: the agent acts on the comment, edits the very
lines it was written on, and the thread turns red at its old range although
the reader can see exactly where it belongs.

Settled 2026-08-27 with the recommended options; no user round was held.
The questions and choices:

- *How to find the new place?* Through the line diff between the text the
  thread was last placed in and the new text, with the `Diff` the gutter
  already uses. Alternatives (nearest heading, fuzzy line similarity) were
  set aside: the diff is already computed on every reload and is what the
  reader sees in the gutter.
- *Which text is "last placed in"?* The text the running viewer held
  before the reload. The store keeps hashes, not text, so a thread edited
  while Fathomable was not running still detaches; that limitation is
  recorded below and in parked ideas rather than solved with a snapshot.
- *How far may an edit reach and still count as an edit of the range?* One
  line of context on either side. A hunk that starts or ends further away
  is a rewrite of the surroundings, and pinning the thread to whichever
  replacement line the arithmetic lands on would mislead; those lines
  count as removed.
- *Show it?* Yes: a new placement state, its own gutter colour, and the
  thread pane's status word. The reader should know the comment no longer
  quotes what is under it.
- *Persist it?* Yes, as an event, so the move survives a restart and the
  agent's `annotations_list` reports the new range. The state clears when
  the user answers, not when an agent replies.

## Decision

### Mapping

- `fathomable_core::reanchor::map_range(old, new, range)` follows a
  1-based range of `old` into `new` through `Diff::new(old, new)`. A line
  outside every hunk keeps its position shifted by the hunks above it. A
  line inside a hunk lands at the same offset within the hunk's
  replacement, clamped to the replacement's last line; a hunk with no
  replacement removes it. The result spans the surviving lines and is
  `Moved` when no annotated line was touched, `Edited` when one was, or
  `Removed` when none survived.
- Only a local hunk is followed: its old range must lie within the
  annotated range widened by one line on each side. Anything larger
  removes the lines it swallows.

### In the viewer

- On every reload, threads of the file whose hashes no longer locate are
  followed from the range they held in the previous text (a thread that
  was already detached there has nothing to follow). `Moved` and `Edited`
  results re-anchor the thread onto the target range in the new text;
  `Removed` leaves it detached at its last known range as 0013 says.
- `Placement` gains `Edited(range)`: found, but on rewritten lines. The
  gutter draws it in `annotation.edited` (added to the
  [0011](0011-theme-schema.md) table, `purple` in the bundled themes), the
  thread pane's status word is `edited`, and the state ranks between
  detached and open when threads overlap a row.
- The snippet stays what the comment was written on; the pane therefore
  still quotes the original lines, which is the point of the colour.

### Store

- A fifth event, `relocate` (`"v": 1`), carries `thread`, `range`, the new
  `anchor` hashes, and `created`. `Store::relocate` captures the anchor
  from the new text and appends it; folding sets the thread's range and
  anchor, bumps `updated` so `since` polling sees it, and records the
  edit time. A user reply, a user resolve, or a reopen clears the edit
  state; an agent reply does not. `Thread::edited()` reports it and the
  thread record serialised over the socket (0014) carries `edited` as an
  optional field, so readers of v1 records need no change.
- Unknown events are still errors, so a store written by this version is
  not readable by an older Fathomable; the format version stays 1 because
  older records read unchanged.

## Consequences

- The parked investigation is resolved for the running viewer. Edits made
  while Fathomable is closed still detach; a store-side snapshot of the
  annotated lines is the parked follow-up.
- `MarkKind` and the theme gain one state each; `docs/guide.md` describes
  *edited* beside *detached*.
- The diff runs once more per reload for files with detached threads only;
  files without threads pay nothing.
