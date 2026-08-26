---
type: Decision
title: CLI and diagnostics
description: Command-line shape, admin flags, and logging so agents can debug Fathomable from its own output.
tags:
  - decision
  - diagnostics
---

# 0009 CLI and diagnostics

Status: accepted (2026-08-26)

## Context

Fathomable will be developed largely by agents. The user reports bugs by
pointing an agent at Fathomable's own logs and state, so the binary must be
self-inspectable.

## Decision

Command line:

- `fathomable [PATH]`: a file opens the single-file view; a directory or no
  argument opens the workspace rooted there (workspace root is the enclosing
  git root when one exists, otherwise the directory itself).
- `fathomable --mcp`: run the stdio MCP server instead of the TUI.
- `--config PATH` overrides the config file; `--theme NAME` selects a theme
  from `$XDG_CONFIG_HOME/fathomable/themes/` for this run.
- Admin flags, all non-interactive and printing to stdout:
  `--doctor` (terminal capabilities, XDG directories, config parse, git,
  live sessions), `--sessions` (list sessions and sockets),
  `--dump-state` (session and thread state as JSON), `--replay-log`
  (re-emit the log for a session in order), `--config-show` (effective
  configuration after defaults and overrides).

Logging:

- `tracing` writes to `$XDG_STATE_HOME/fathomable/log/<session-id>.log`
  always; level from `FATHOMABLE_LOG` or config, default `info`.
- Log lines are structured (JSON) so an agent can grep and parse them; the
  session id appears in the TUI status so the user can name it.

## Consequences

- Every subsystem is expected to emit enough tracing to reconstruct a bug
  report; this is part of code review.
- Admin flags reuse `fathomable-core` readers, so they never need a terminal.
