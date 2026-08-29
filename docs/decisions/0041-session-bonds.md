---
type: Decision
title: Session bonds between hooks and the MCP server
description: The hello hook records which processes its session runs under; the MCP server finds the nearest of those among its own ancestors and signs as that session, so replies stay signed after a resume and the model no longer has to repeat its id — with a soft nudge as the fallback wherever the bond cannot be made.
resource: crates/fathomable-core/src/bond.rs
tags:
  - decision
  - sessions
  - diagnostics
---

# 0041 Session bonds between hooks and the MCP server

Status: accepted (2026-08-29)

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
- Copilot's `COPILOT_AGENT_SESSION_ID` and VS Code parity are left to
  their acceptance tests; the fallback carries them until then.
