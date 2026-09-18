---
type: Decision
title: Retire the one-release compatibility
description: The old spellings kept "for one release" by the vocabulary rename go before any release exists — the `annotation.*` and `ui.sidebar*` theme keys, the hidden `--sessions` flag, the sweep of the old `sessions/` directory, and the socket op `annotations_list`, which becomes `threads_list` to match the tool it serves.
related_resources:
  - crates/fathomable-core/src/theme.rs
  - crates/fathomable-core/src/session.rs
  - crates/fathomable-core/src/xdg.rs
  - crates/fathomable/src/main.rs
  - crates/fathomable/src/doctor.rs
tags:
  - decision
  - sessions
  - configuration
  - diagnostics
---

# 0051 Retire the one-release compatibility

Status: accepted (2026-09-04)

Socket surface retired 2026-09-18 by [0089](0089-store-only-mcp.md).
There is no replacement socket protocol or compatibility reader. The other
retirements in this record remain; the socket narrative below is historical.

## Context

[0047](0047-one-vocabulary.md) renamed the viewer's words in one sweep
and kept four old spellings alive "for one release": the `annotation.*`
theme keys loaded as `thread.*` and `--doctor` named each one; `--sessions`
stayed as a hidden alias of `--viewers`; dead records were still swept from
the `sessions/` directory that `viewers/` replaced; and the closing note
said a later record would remove them. [0049](0049-inline-threads-and-the-rail.md)
added `ui.sidebar*` for `ui.rail*` on the same terms. The socket kept its
op `annotations_list` while the MCP tool it serves became `threads_list`,
so the one wire name that still said *annotation* sat between two
processes of the same binary.

Fathomable is at 0.1.0 and has never been tagged. "One release" of
compatibility for a rename nobody outside the repository has used is
code that guards nothing: the theme loader carries a rename table and a
report channel to `--doctor`, the sweep walks a directory no build has
written since 2026-09-03, and the guide still had a section called
*Annotations*. The user chose on 2026-09-04, in a cleanup round, to remove
all of it now rather than wait for a first tag.

## Decision

- **Theme keys.** `annotation.*` and `ui.sidebar*` are unknown keys, the
  same error any misspelling gets. The rename table, the `DeprecatedKey`
  report, and the `--doctor` note that read it go.
- **CLI.** `--sessions` is gone; `--viewers` is the only spelling.
- **State directory.** `sweep_dead` walks `viewers/` only. A `sessions/`
  directory left by a build before 2026-09-03 is inert and can be
  deleted by hand.
- **Socket.** The op is `threads_list`, the wire name of
  `Request::ThreadsList`; arguments and the `Threads` response are
  unchanged. The protocol stays v1: the viewer and `--mcp` are one
  binary, upgraded together, and an older `--mcp` process against a
  newer viewer gets the ordinary unknown-op error until it restarts.
- **Guide.** Section 4 is *Threads*, and the gutter's marks are thread
  marks. Older records keep their wording under the
  [0047](0047-one-vocabulary.md) rule: a dated line, not a rewrite.

## Consequences

- A theme written against the pre-0047 or pre-0049 keys fails to load with
  the offending key named; the fix is the rename the error points at.
- `--doctor` loses one kind of note and the theme type two public items.
- The word *annotation* remains only where a record's history uses it and
  in the store's file and type names, which [0047](0047-one-vocabulary.md)
  kept on purpose.
- No further compatibility window is owed by any earlier record; a rename
  after the first tag will need its own.
