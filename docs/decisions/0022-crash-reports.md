---
type: Decision
title: Crash reports
description: What Fathomable prints when it dies, so one paste tells an agent where it was and what it was doing.
resource: crates/fathomable/src/crash.rs
tags:
  - decision
  - diagnostics
---

# 0022 Crash reports

Status: accepted (2026-08-27)

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

A narrow-terminal panic in the sidebar killed Fathomable with no message at
all. The message was there — the default panic hook wrote it — but it went to
the alternate screen, which the terminal throws away as the process leaves it.
The user saw the shell prompt come back and nothing else.

That is the general shape of the problem, not one bug: any panic in a TUI is
invisible, and the evidence that would identify it (the terminal's size, the
open document, the view mode) dies with the process. [0009](0009-cli-and-diagnostics.md)
already says every subsystem should emit enough to reconstruct a bug report,
and the log holds most of it, but a user who has just watched the viewer
vanish needs one block to paste, not a file to go hunting for.

## Decision

A panic hook, armed for the TUI only, prints one report and writes a copy:

- The saved copy follows the
  [private-state contract](0009-cli-and-diagnostics.md#persistent-state-privacy):
  a 0600 file in a 0700 application directory, with links and unsafe existing
  files refused before writing. Reports still appear on stderr, and should
  be reviewed for private data before sharing.
- The hook **hands the terminal back before it writes**, so the report lands
  on the screen the user keeps. The alternate-screen and raw-mode state lives
  in process statics rather than in `TerminalGuard`, because the hook runs
  before the guard is dropped and cannot reach it; restoring twice is a
  no-op, so the hook and the guard can both call it.
- The report carries the `:status` rows — the open document and its view
  mode, the terminal's size, the session, the workspace, the socket, and
  every state path — snapshotted once per frame so they describe the frame
  that died. `:status` gains the document and terminal rows for this, and
  stays the one place that answers "what is this session doing".
- A backtrace is **always** captured. The panic machinery, the async
  runtime, and the process entry point are dropped from it: those frames read
  the same in every report and would otherwise crowd out the handful that
  differ. Surviving frames keep their original numbers, so the gaps show
  where the plumbing was.
- A copy goes to `$XDG_STATE_HOME/fathomable/log/<session-id>.crash`, beside
  that session's log, for a report that has scrolled away. `--doctor` counts
  the reports waiting there, so the entry point for "something is off"
  ([0009](0009-cli-and-diagnostics.md)) says when there is one to read.
- An error that ends the run gets the same block, with the `anyhow` chain in
  place of a backtrace — but only once the viewer has drawn a frame. A
  mistyped `--theme` is a mistake to correct, not a crash to report, and
  stays the one line it always was.

## Consequences

- The default panic hook is replaced rather than chained, so a panic prints
  once, in this shape.
- The report is public evidence: it names absolute paths from the user's
  machine, as `--doctor` and `:status` already do.
- Anything added to `:status` appears in crash reports too. That is the
  intent — one description of a session, used twice.
- A crash inside the per-frame snapshot itself would leave the report without
  its state rows; it is read with `try_lock` so it prints anyway.
