---
type: Decision
title: Modules by concept
description: The app crate's modules are grouped by the concept they serve — threads, draw, input, jump, run — rather than by the milestone that added them; decision records point at the concept module they changed through related_resources, which need no backlink, so a record need not own a new file.
related_resources:
  - crates/fathomable/src/app/mod.rs
  - crates/fathomable/src/app/run.rs
  - crates/fathomable/src/app/threads/mod.rs
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/input/mod.rs
  - scripts/okf-lint.py
  - crates/fathomable-testing/src/lib.rs
  - crates/fathomable-testing/src/git.rs
  - crates/fathomable-testing/src/vocabulary.rs
tags:
  - decision
  - architecture
  - documentation
---

# 0048 Modules by concept

Status: accepted (2026-09-03)

Module inventory amended 2026-09-15 by
[0082](0082-three-tool-review-core.md): the auto-jump module is removed.
The small `app/agents.rs` boundary initially remained only for the visible,
not-yet-implemented human wake action. Amended later 2026-09-18: that wake
placeholder and the now-empty module are removed too.

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
  gone. Since 2026-09-04 the core crate uses it too, as a
  dev-dependency: Cargo allows a dev-dependency on a crate that depends
  on the crate under test, so the core's own copies went the same way.
  Its `vocabulary` module also holds the identifier parser and membership
  check used only by prose-contract tests; the production names and `ALL`
  table remain in `fathomable-core`.
  Fixture-only Git types and open options live in this never-published
  crate, not the core API. Fixture options load only repository-local
  configuration: personal/system configuration, global attributes, and
  environment overrides cannot hide missing fixture setup, including when
  tests run in a commit hook. Ref and commit writes supply synthetic identities
  rather than relying on a contributor's Git identity.
  The app crate's own scaffolding, an `App` builder on such a temp dir
  and key presses against it, is `app/testing.rs`, compiled for tests
  only; the fourteen per-module `fixture`/`app`/`press` copies it
  replaced went the same day.
- **Long test modules are sibling files.** A source file of 1000 lines or
  more keeps a test module that would be 35% or more of it in
  `<module>/tests.rs`, declared `#[cfg(test)] mod tests;`; the
  `boundaries` gate (`scripts/check-rust-source-policy.py`) rejects the
  inline form since 2026-09-04. Shorter files keep their tests inline.

## Consequences

- A reader looking for "threads" or "drawing" finds one directory; a
  record about a concept can point at its module without a new file.
- `git/` (status, hunks) and the queue, toasts, and change hint of the
  jump concept still live in `app/mod.rs` and `view.rs`; splitting them
  out is left to the record that next changes them. So is splitting
  `draw/mod.rs` and `threads/mod.rs` further.
- `scripts/okf-lint.py` and `docs/okf.md` record the relaxed rule.
