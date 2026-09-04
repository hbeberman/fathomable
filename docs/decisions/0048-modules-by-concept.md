---
type: Decision
title: Modules by concept
description: The app crate's modules are grouped by the concept they serve — threads, draw, input, jump, agents, run — rather than by the milestone that added them; decision records point at the concept module they changed through related_resources, which need no backlink, so a record need not own a new file.
related_resources:
  - crates/fathomable/src/app/mod.rs
  - crates/fathomable/src/app/run.rs
  - crates/fathomable/src/app/threads/mod.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/input/mod.rs
  - crates/fathomable/src/app/jump.rs
  - crates/fathomable/src/app/agents.rs
  - scripts/okf-lint.py
  - crates/fathomable-testing/src/lib.rs
tags:
  - decision
  - architecture
  - documentation
---

# 0048 Modules by concept

Status: accepted (2026-09-03)

## Context

`app/` had grown one file per milestone: twenty-four modules in one
directory, named for the record that added them (`open_thread.rs`,
`mark_words.rs`, `thread_list.rs`, `file_threads.rs`) rather than for
the idea they serve, and `mod.rs` held the terminal loop beside the app
state. The OKF rule that a source file carries exactly one backlink,
together with the rule that every `related_resources` entry must
backlink too, meant a new record needed a new file even when it changed
an existing module, which is how the directory got that way.

## Decision

- **Group by concept.** `app/threads/` holds the thread concept: the
  store, marks, comment box, and pane in `mod.rs`, then `cursor`
  ([0046](0046-one-thread-cursor.md)), `file_pane`, `list`, `delete`,
  `detached`, `open`, `reach`, `reanchor`, `waiting`, and `words`.
  `app/draw/` renders: the frame in `mod.rs`, with `gutter`, `info`, and
  `message` building rows. `app/input/` binds and dispatches keys and the
  mouse ([0045](0045-bindings-are-data.md)). `app/jump.rs` is auto-jump,
  `app/agents.rs` wakes subscribers, and `app/run.rs` owns the terminal,
  the tokio loop, the watcher wiring, and the socket, leaving `app/mod.rs`
  to `App`, `Focus`, `Popup`, and `Options`. `view`, `sidebar`, `watch`,
  `socket`, `commands`, and `clipboard` keep their files. Code moves;
  nothing changes but paths and, in files that moved below a directory,
  `pub(super)` widening to `pub(crate)` so their siblings above can still
  see them.
- **Records point, they need not own.** A record's `resource` is the
  file it owns and the only backlink that file carries. A record that
  changes an existing module names it under `related_resources`, which
  the linter checks for existence and no longer for a backlink. Existing
  records keep their resources at the moved paths.
- **Module docs lead with the concept** and cite records once, at the
  end, per the Rust guidelines the repository follows.
- **Test scaffolding is one crate.** `crates/fathomable-testing`
  (`publish = false`, a dev-dependency of the app crate) holds `TempDir`
  and the git fixtures (`init`, `commit_and_stage`, `stage`, `amend`,
  `write_tree`); the thirteen per-module copies in the app crate are
  gone. The core crate keeps its own four, since the testing crate
  depends on the core for the workspace's git open options.

## Consequences

- A reader looking for "threads" or "drawing" finds one directory; a
  record about a concept can point at its module without a new file.
- `git/` (status, hunks) and the queue, toasts, and change hint of the
  jump concept still live in `app/mod.rs` and `view.rs`; splitting them
  out is left to the record that next changes them. So is splitting
  `draw/mod.rs` and `threads/mod.rs` further.
- `scripts/okf-lint.py` and `docs/okf.md` record the relaxed rule.
