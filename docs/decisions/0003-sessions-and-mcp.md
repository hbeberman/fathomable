---
type: Decision
title: Sessions and the MCP server
description: How a running Fathomable is discovered and driven by agents through a stdio MCP subcommand.
tags:
  - decision
  - sessions
  - architecture
---

# 0003 Sessions and the MCP server

Status: accepted (2026-08-26)

## Context

Fathomable is agent-agnostic. Agents must be able to open files, jump to
locations, and read and reply to annotations without Fathomable knowing which
agent product they are. Several Fathomable instances may run for different
repositories at once.

## Decision

- A **session** is one running TUI bound to one workspace root. On start it
  writes a session record to `$XDG_STATE_HOME/fathomable/sessions/<id>/` and
  listens on a Unix socket in `$XDG_RUNTIME_DIR/fathomable/<id>.sock`. Records
  of dead sessions are cleaned up on next start.
- `fathomable --mcp` is a stdio MCP server intended to be launched by the agent
  TUI. It binds to the session whose workspace root contains the current
  working directory (longest match). It always exposes `session_list` and
  `session_switch` so the agent can rebind explicitly.
- The MCP server is a thin client: it forwards requests over the session
  socket using a small line-delimited JSON protocol defined in
  `fathomable-core`. The TUI is the only process that owns session state.
- Initial tool surface: `session_list`, `session_switch`, `open` (file plus
  optional line range), `annotations_list` (optionally since a timestamp),
  `thread_reply`, `follow` (mark files the agent is actively working on so the
  TUI can lazily follow them).
- Transport is stdio only. HTTP is deferred.
- Agent identity in replies was left open here and is settled in
  [0014](0014-mcp-server-and-socket-v1.md): observed client identity and a
  self-declared persona are both recorded.

## Consequences

- Agents get a stable contract independent of the terminal.
- Session discovery by cwd means an agent launched outside the workspace must
  call `session_switch`; that is acceptable and explicit.
- The socket protocol is a second public surface and is versioned from day
  one.
