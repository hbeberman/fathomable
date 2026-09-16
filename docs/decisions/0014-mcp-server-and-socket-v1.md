---
type: Decision
title: MCP server and socket protocol v1
description: The stdio MCP server built on rmcp 3 against the 2026-07-28 spec, the v1 session socket operations it forwards, per-request agent identity, and session binding.
resource: crates/fathomable/src/mcp/mod.rs
related_resources:
  - crates/fathomable/src/app/socket.rs
tags:
  - decision
  - sessions
  - annotations
  - architecture
---

# 0014 MCP server and socket protocol v1

Status: accepted (2026-08-26)

Tool surface and routing superseded 2026-09-15 by
[0082](0082-three-tool-review-core.md): `fathomable --mcp [DIR]` binds one
repository checkout and exposes only `threads`, `thread_start`, and
`thread_reply`. There is no MCP viewer control, workspace listing or
override, subscription, or watch. Startup discovers the checkout without a
workspace marker. Socket protocol v4 retains only `ThreadStart` and
`ThreadReply` requests and `Threads`/`Error` responses; `ThreadsList`, `Open`,
and `Follow` are removed, with MCP reads going directly to the store. The
transport history below remains.

Identity and routing amended 2026-09-15 by
[0080](0080-automatic-chat-identity.md): harness-qualified identity is
automatic, every annotation write requires it, and no connection-wide
subscriber cache or workspace pin remains. `fathomable --mcp [DIR]`
anchors the default at startup; per-call workspace overrides remain.
The tool table and identity rules below describe the original decision.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

Amended 2026-09-04 by [0051](0051-retire-one-release-compatibility.md):
the socket op `annotations_list` is `threads_list`, the name of the tool
it serves; the protocol stays v1.

## Context

[0003](0003-sessions-and-mcp.md) decided that `fathomable --mcp` is a stdio
MCP server acting as a thin client of the session socket, and listed the
tool surface. [0012](0012-workspace-mode.md) shipped socket protocol v0,
which answers only `ping` and `session_info`. Milestone 4 of the
[roadmap](../roadmap.md) needs the real operations.

Meanwhile MCP published the 2026-07-28 revision, its largest since launch.
It removes the `initialize`/`initialized` handshake and protocol-level
sessions: a client may start with `server/discover`, and every request then
carries protocol version, client info, and capabilities in `_meta`.
Server-to-client requests become multi-round-trip results (`InputRequired`
with an opaque `requestState`), tasks are an official extension, and roots,
sampling, and logging are deprecated. `rmcp` 3.x implements this while
staying compatible with the `initialize` flow, which agent hosts still send
over stdio. The choices below were captured in a question round on
2026-08-26.

## Decision

### MCP server

- `rmcp` 3.x with features `server`, `transport-io`, `macros`, in the
  `fathomable` binary crate only (per [0001](0001-dependency-policy.md) and
  [0002](0002-crate-layout.md)). Socket request and response types stay in
  `fathomable-core`.
- Transport is stdio only; HTTP stays parked. The 2026-07-28 stateless HTTP
  work targets load-balanced deployments; an agent-launched subprocess gains
  nothing from it and would add an authorization surface.
- The server accepts both the legacy `initialize` flow and discovery-first
  startup. No code depends on handshake state: client identity is read from
  the request context on every call.
- Every tool completes in one round trip and returns `Complete`. No
  multi-round-trip requests, no tasks extension, no `request-state` feature.
- No use of MCP `logging/*`, roots, or sampling. Diagnostics go to the file
  log of [0009](0009-cli-and-diagnostics.md).
- Tool results carry `structured_content` JSON for anything list-shaped
  (`session_list`, `annotations_list`) plus a short text summary, since some
  hosts render only text.

### Tools

| Tool | Arguments | Effect |
|---|---|---|
| `session_list` | – | Live session records, marking the current default. |
| `session_switch` | `session` | Sets the default session for later calls. |
| `open` | `path`, optional `line`, `end_line`, `session` | Open a file in the TUI and scroll to the range (shown, not selected; see the 2026-08-28 note). |
| `follow` | `paths`, `session` | Record the files the agent is working on. |
| `annotations_list` | optional `since` (Unix seconds), `path`, `session` | Threads and replies created or changed since `since`. |
| `thread_reply` | `thread`, `body`, optional `resolve`, `persona`, `session` | Append a reply, optionally resolving. |

Every tool accepts an optional `session` id that overrides the default.
The default is chosen at startup by longest workspace-root match on the
current directory, as in 0003, and changed by `session_switch`. That
default is the only state the `--mcp` process holds.

`follow` in this milestone only stores the list in the session and shows a
status-line marker; hints and jumping are milestone 6.

### Agent identity

A reply written through `thread_reply` records two things: the client
`name` and `version` observed in the MCP request context, and an optional
self-declared `persona` argument. `Author::Agent` grows from a bare name to
`{ name, client }`, where `name` is the persona if given and the client
name otherwise; existing JSONL that stores a plain string still loads
(`#[serde(default)]` on `client`). The thread panel shows `persona (client)`
when they differ. Impersonation among same-uid processes cannot be
prevented, so Fathomable shows provenance rather than enforcing it. This
closes the "agent identity" investigation of 0003.

### Annotation consumption

Agents poll with `annotations_list since=<ts>`; the agent owns its cursor
and the server stores none. This matches the stateless direction of the
spec. A server-side per-agent cursor is parked until polling proves
inadequate.

(Amended 2026-08-29.) Polling proved inadequate: an agent mid-task does
not poll. [0040](0040-agent-subscriptions-and-hooks.md) adds a per-
subscriber delivery record keyed by message, a `threads_pending` tool,
and harness stop hooks; `annotations_list since=` stays as the plain
read and gains `limit`.

### Socket protocol v1

- Line-delimited JSON as before, `"v":1`. Requests are a serde enum tagged
  by `op`, replacing the hand-written v0 parser. A v0 `ping` or
  `session_info` is still answered.
- Operations mirror the tools one to one: `ping`, `session_info`, `open`,
  `follow`, `annotations_list`, `thread_reply`. Responses are
  `{"ok":true,...}` or `{"ok":false,"error":...}`.
- Authorization is the mode-0700 `$XDG_RUNTIME_DIR` plus a peer-uid check
  (`SO_PEERCRED`) that rejects other users. No token.
- The TUI removes its session record and socket on SIGTERM and SIGHUP as
  well as on clean quit (`tokio` `signal` feature), so `session_list` stays
  accurate for agents.

Note (2026-09-04): [0055](0055-six-tools.md) reduces the tool surface
to six — `workspaces`, `open`, `follow`, `threads`, `thread_reply`,
`thread_watch` — and takes `follow` off the socket: protocol version 3
has no `follow` request and answers `thread_reply` with the thread. The
tools live in `mcp/tools.rs`; this record keeps the transport in
`mcp/mod.rs`.

Note (2026-09-05): [0062](0062-one-version-no-compatibility.md) takes
`ping` and `session_info` off the socket, refuses every protocol
version but the one the binary speaks, and restarts the number at 1.

## Consequences

- The tool surface is a pure function of the request plus the socket reply,
  so switching a host to discovery-first startup changes nothing in
  Fathomable.
- Identity in threads is honest about its source: what the host said and
  what the agent claimed are both kept.
- The socket enum makes adding an operation a one-arm change on each side;
  the v0 compatibility arm can be dropped once no released binary speaks it.
- The `annotations.rs` author schema changes shape; the JSONL stays
  readable by older builds only for replies without a `client` field.
- (Amended 2026-08-28.) `open` with `end_line` used to select the range,
  and an agent that "showed the file" by passing its full extent left
  the whole file selected. The range is now brought on screen with the
  cursor on `line` and nothing selected: `View::reveal_source_range`
  scrolls so the end row is visible when the range fits the screen and
  keeps the start visible when it does not. The tool description tells
  the agent a range is for the lines it is pointing at.
