---
type: Decision
title: Session bonds between hooks and the MCP server
description: The MCP server learns its session from Copilot's launch environment or the hello hook's process bonds, without subscribing automatically or confusing session identity with workspace selection.
resource: crates/fathomable-core/src/bond.rs
tags:
  - decision
  - sessions
  - diagnostics
---

# 0041 Session bonds between hooks and the MCP server

Status: accepted (2026-08-29)

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

[0040](0040-agent-subscriptions-and-hooks.md) could not learn the
harness session id inside `--mcp`, so the model carries it: `hello`
prints it, `follow(id, type)` subscribes, and the connection remembers
the id to sign later `thread_reply` calls. The smoke test of 2026-08-29
(`scripts/demo-repo.sh` against `claude -p`) found the seam: after
`claude -r <id>` the harness starts a fresh `--mcp` process, the memory
is gone, the model answers the stop hook without calling `follow`
again, and its reply lands unsigned. An unsigned reply is "someone
else's" newest message, so the thread is pending for the session that
wrote it and the stop hook re-delivers it until the harness's block
cap. The same happens whenever a harness restarts an MCP server
mid-session.

What the hook and the server do share is a parent. Verified on Claude
Code the same day: the `SessionStart` hook, the `Stop` hook, and the
MCP server are each a `bash -c` child of the one harness process. The
MCP process is spawned some 40 ms before the `SessionStart` hook runs,
but its first tool call comes only after the model's first turn, which
the harness holds until the hook's output is in context.

Walking `/proc/<pid>/status` upward is available to a process for its
own user's processes under `hidepid=2`, stops at a PID namespace
boundary, is not affected by seccomp, and can be denied by a
Landlock/AppArmor profile — every one of which yields a shorter chain
or a read error, never a wrong answer.

Two false bonds were named in the discussion and rule the design: a
whole ancestor chain reaches the user's shell and multiplexer, which
every session under them shares; and a hand-run `fathomable --mcp` in
that shell would then bond to whichever session had recorded the shell.

## Decision

- `bond.rs` in `fathomable-core`, which this record backs, reads the
  calling process's **ancestors** as `(pid, start time)` pairs, nearest
  first, from `/proc`; the start time (field 22 of `/proc/<pid>/stat`,
  in `USER_HZ` ticks since boot) tells a reused pid from the process
  that held it. Any error ends the walk where it is.
- A **bond** is a register event `{id, processes: [(pid, start)],
  created}` appended by `fathomable hello`: the session id the hook was
  given and the ancestors of the hook process that **started within
  `BOND_WINDOW` (30 s) before the hook itself**. A session-scoped
  process is born with the session; the user's shell, multiplexer, and
  an outer harness that spawned this one are older and are left out.
  `hello` runs again on a resume and records the new process. It is the
  only writer of bonds; `pending` records nothing.
- `--mcp` reads its own ancestors once at start. On any call that needs
  a session id and has none — no `id` argument, no `follow` on this
  connection — it takes the **nearest** ancestor that appears in a bond
  whose session is **unique**: an ancestor recorded for two sessions is
  ambiguous and ends the search with no bond, rather than walking on to
  something shared even more widely. The bonded id then serves as the
  `follow` id, the `threads_pending`/`thread_watch` subscriber, and —
  when it is a subscriber — the `id`/`kind` a `thread_reply` is signed
  with. The connection's own memory of a `follow` still wins.
- Bonds expire with `agents.expire-after` like subscribers and are
  dropped on load. They hold pids and a session id, nothing else.
- **Fallback.** No bond, an ambiguous one, a denied `/proc`, a detached
  server (VS Code's Agent Host gateway is the suspect): the model is
  nudged instead. `hello` and the server instructions say to pass `id`
  to `follow` and, new, `thread_reply` accepts an optional `id` that
  signs the reply when it names a subscriber. Nothing is ever signed
  with a guessed id.

## Consequences

- A resumed or restarted session signs its replies again without the
  model remembering anything, and `follow` may omit `id` where the bond
  holds. The `type` is still declared by the model, so a subscription
  stays opt-in and a session that never called `follow` stays silent.
- `hello` now appends to the register; 0040's "the hooks never write"
  narrows to the thread store, which remains untouched by both hooks.
- The bond window is a heuristic with a named failure: a shell opened
  under 30 s before the harness is recorded, and a hand-run `--mcp` in
  that shell would sign as that session until another session under the
  same shell makes it ambiguous. Documented in the guide, not guarded.
- Copilot's `COPILOT_AGENT_SESSION_ID` was initially left to acceptance
  tests; the amendment below implements it. VS Code parity remains open.

### Copilot launch identity (2026-09-15)

Copilot CLI supplies `COPILOT_AGENT_SESSION_ID` to its stdio MCP
subprocesses, announced in its
[1.0.29 release](https://github.com/github/copilot-cli/releases/tag/v1.0.29)
and observed on 1.0.82. `--mcp` reads it once at startup. An empty,
whitespace-only, or non-Unicode value is ignored with a diagnostic.

Every session-aware tool resolves the explicit `id` first, then the
connection's most recent `follow` id, then Copilot's launch id, then a
process bond. An explicit `follow` can therefore rebind a connection.
An explicit id on an individual call overrides the default for that
call only. Missing identity leaves manual reading and writing available;
the error does not ask the user to retrieve an internal session id.

Discovery is not subscription. Only `follow` opts the session in, and
signatures, deliveries, and watches require a live subscription in the
addressed workspace. The id neither registers nor selects a workspace:
the explicit workspace argument, connection pin, and cwd retain their
existing precedence. A connection's cached id cannot carry another
workspace's type or persona into a signature.

The environment is launch-scoped evidence, not per-call MCP metadata.
Copilot 1.0.78 says switching sessions no longer restarts MCP servers;
whether a client retains separate session-scoped servers must not be
inferred from that wording. Session switching and subagent attribution
remain harness lifecycle questions, not guarantees of this fallback.
Hooks are still required for automatic comment delivery, but Copilot
does not need `hello` merely to identify itself. Without delivery hooks,
agents fetch comments with `threads`.

Note (2026-08-29): "`hello` … is the only writer of bonds; `pending`
records nothing" still holds for bonds, but `hello` now writes more than
bonds. On a `SessionStart` whose `source` is `resume` it also composes
the pending blob ([0040](0040-agent-subscriptions-and-hooks.md) note of
the same day), so it appends deliveries and fired-watch events too. The
resume this record was written for — where the harness restarts `--mcp`
and the model answers the stop hook — is therefore now answered twice
over: the bond signs the reply, and `hello` hands the session what it
missed while it was stopped. A `compose` error there stays silent and
exits 0, so a damaged register cannot cost the session its
`SessionStart`.
