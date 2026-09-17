---
type: Decision
title: The header names its counts
description: Review and Threads headers name each lifecycle count (`● 2 active  ◐ 1 resolution proposed  ○ 1 resolved`); zero counts are omitted, and when the row cannot hold all words they drop together before the bare counts do.
resource: crates/fathomable/src/app/draw/counts.rs
related_resources:
  - crates/fathomable/src/app/draw/header.rs
  - crates/fathomable/src/app/threads/list.rs
tags:
  - decision
  - annotations
  - rendering
---

# 0075 The header names its counts

Status: superseded in part (2026-09-16) by
[0085](0085-thread-lifecycle-and-auto-resolve.md) and
[0086](0086-one-thread-summary-and-its-actions.md). Header and directory
counts now partition lifecycle as `● n active`, `◐ n resolution proposed`,
and `○ n resolved`; user/agent last-act counts and waiting retire. Zero
counts are omitted and all words still drop together before counts.

Amended 2026-09-17: header counts are passive. The normal review title is
`Reviews`; its checked title menu and bottom key bar own `x`, while lifecycle
counts remain display state at the right. Historical wording follows.

## Context

[0066](0066-one-circle-language.md) put the counts by colour on the
review list's header (`review  ●2 ●3 ○1`) and on the threads pane's,
and let the colour alone say what each circle counts: since
[0071](0071-author-stripes.md) the user's blue means the user has the
last word, the agents' green means an agent does. A proposed thread
(`◐`, [0053](0053-resolution-is-the-users.md)) counted under the green
`●` with the rest of the waiting threads, so the header could not say
how many of them only need a nod.

The user asked on 2026-09-11 for the review list to name itself
`review threads` and to put a word after each count. Two forms were
offered: the words alone, or shorter words when the row is tight. The
user chose one set of words and, when the row cannot hold them, no
words at all.

## Decision

- **The words.** After each count, in the dim colour the keys' words
  use, a word for whose the circle is (amended the same day: a space
  sits between the circle and its number, and the number reads in
  `ui.text` rather than dim, so the count stands out from its word;
  a hidden count's number dims with the rest): `● 2 user` (the user has the last
  word, `thread.open`), `● 3 agent` (an agent does, `thread.waiting`),
  `◐ 1 resolve?` (an agent's newest reply proposes resolving), and `○ 1
  resolved`. The review list's header reads `review threads  ● 2 user
  ● 3 agent  ◐ 1 resolve?  ○ 1 resolved`, then ` · path` while `f` narrows
  it, as before. The threads pane's header carries the same counts with
  the same words after `threads · workspace`.
- **A proposed thread has its own count.** `Counts` gains `proposed`;
  a `◐` thread counts there and no longer under waiting, so the four
  numbers partition the threads. The `?` of a detached thread still
  counts under its colour. A zero count is still not drawn. The `○n`
  count still takes the click that toggles `x` and still reads dim
  while resolved threads are hidden.
- **The words go together.** A header lays its counts out with their
  words when every count fits that way. When the row is too narrow for
  that, the words all drop at once and the counts read bare (`● 2 ● 3 ◐ 1
  ○ 1`), the 0066 form; only then do counts drop from the end, as the
  hints of [0059](0059-headers-and-the-key-bar.md) do. There is no
  shorter wording between the two: a reader sees one set of words or
  none, so the header never changes what it calls a thing. The mouse
  reads the same layout the drawing chose, so a click on a count with
  or without its word runs the same action.

## Consequences

- `app/draw/counts.rs`, which this record backs, builds the count
  hints from a `Counts` with their words; `header.rs` gives a hint an
  optional word, decides between the worded and bare forms in
  `shown`, and draws the word after the number.
- `Counts` in `app/threads/list.rs` gains `proposed`, and
  `review_counts` sorts a proposed thread there.
- 0053's counts passage and 0066's header lines carry dated notes
  pointing here; `docs/guide.md` shows the new header.
