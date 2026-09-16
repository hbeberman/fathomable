---
type: Decision
title: CLI and diagnostics
description: Command-line shape, admin flags, and logging so agents can debug Fathomable from its own output.
resource: crates/fathomable/src/main.rs
related_resources:
  - crates/fathomable/src/doctor.rs
  - crates/fathomable/src/logging.rs
  - crates/fathomable/src/seed.rs
  - crates/fathomable-core/src/xdg.rs
tags:
  - decision
  - diagnostics
---

# 0009 CLI and diagnostics

Status: accepted (2026-08-26)

Agent-delivery CLI removed 2026-09-15 by
[0082](0082-three-tool-review-core.md): `pending` and hook diagnostics no
longer exist. The obsolete `--register` command is also removed because
`--mcp [DIR]` discovers its checkout directly and viewers maintain the
markers used by viewer diagnostics and worktree state. Existing installed
hook entries are removed manually; the program does not edit user
configuration or erase legacy state.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

Fathomable will be developed largely by agents. The user reports bugs by
pointing an agent at Fathomable's own logs and state, so the binary must be
self-inspectable.

## Decision

Command line:

- `fathomable [PATH]`: a file opens the single-file view; a directory or no
  argument opens the workspace rooted there (workspace root is the enclosing
  git root when one exists, otherwise the directory itself).
- `fathomable --mcp [DIR]`: run the stdio MCP server instead of the TUI;
  `DIR`, or startup cwd when omitted, anchors its immutable default
  workspace ([0080](0080-automatic-chat-identity.md)).
- `--config PATH` overrides the config file; `--theme NAME` selects a theme
  from `$XDG_CONFIG_HOME/fathomable/themes/` for this run.
- Admin flags, all non-interactive and printing to stdout:
  `--doctor` (terminal capabilities, XDG directories, config parse, git,
  live sessions), `--sessions` (list sessions and sockets),
  `--dump-state` (session and thread state as JSON), `--replay-log`
  (re-emit the log for a session in order), `--config-show` (effective
  configuration after defaults and overrides).
- The hook subcommand `pending` (0040, 0042, 0080) takes
  `--verbose`: an account of every lookup — stdin, workspace, viewers,
  config, register, subscriber, thread counts — and why the hook
  stayed silent, on stderr so the harness's hook log carries it and
  stdout still means what it did; on stdout only when stderr is the
  answer (a Claude or Codex stop block).

Logging:

- `tracing` writes to `$XDG_STATE_HOME/fathomable/log/<session-id>.log`
  always; level from `FATHOMABLE_LOG` or config, default `info`.
- Log lines are structured (JSON) so an agent can grep and parse them; the
  session id appears in the TUI status so the user can name it.

Note (2026-08-29): `--register [PATH]` writes the workspace marker of
[0024](0024-workspace-sessions.md) for the root around `PATH` without
starting a viewer, and prints the root and its state directory. It exists
so scripts and tests (`scripts/demo-repo.sh`) can make a directory known
to headless `--mcp` and the hooks of
[0040](0040-agent-subscriptions-and-hooks.md) without mirroring the
marker format.

Note (2026-09-05): `fathomable seed FILE [--workspace DIR]`, hidden from
`--help`, writes the threads, replies, resolutions, subscribers, and
watches a JSON file declares through `annotations::Store` and
`agents::Register` (`crates/fathomable/src/seed.rs` documents the
shape). It replaces the Python in `scripts/demo-repo.sh` that wrote
`threads.jsonl` and `agents.jsonl` by hand from a copy of the serde
shapes, so the formats have one writer. It exists for the demo and for
tests, not for users; the viewer and the tools are how threads are made.

## Consequences

- Every subsystem is expected to emit enough tracing to reconstruct a bug
  report; this is part of code review.
- Admin flags reuse `fathomable-core` readers, so they never need a terminal.
