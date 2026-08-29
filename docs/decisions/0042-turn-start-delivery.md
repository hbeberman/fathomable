---
type: Decision
title: Delivery at both ends of a turn
description: The pending hook also runs from the harness's prompt-submit event and, optionally, after every tool call, where it adds the subscriber's undelivered threads to context and exits 0 instead of blocking; a comment that lands while an agent waits reaches it on the wake itself, one that lands mid-task reaches it after the next tool result, and no agent needs to poll for comments on any harness.
resource: crates/fathomable/src/hooks.rs
tags:
  - decision
  - sessions
  - annotations
---

# 0042 Delivery at both ends of a turn

Status: accepted (2026-08-29)

## Context

[0040](0040-agent-subscriptions-and-hooks.md) delivers pending threads
from the harness's *stop* hook: when the model wants to end its turn,
the hook blocks the stop and the threads become the next prompt. A
live session on 2026-08-29 (Claude Code 2.1.251) showed the seam. The
agent was told to review, then wait a minute. The stop hook ran at
17:56:25 and found nothing; the user's reply landed at 17:56:26; the
background `sleep` finished at 17:57:23 and the harness woke the model
with a synthetic `<task-notification>` prompt. A wake is a turn
*start*, so no hook ran, and the model — having been told nothing —
spent a tool call on `threads_pending`, which the user rejected. An
interrupted turn runs no stop hook either, so the reply surfaced only
at the end of the next human-authored turn, four minutes later.

Measured the same day, with cheap models, against the harnesses the
project cares about:

- **Claude Code** refuses a foreground `sleep` and forces
  `run_in_background`; the task's completion wakes the model with a
  synthetic user-role prompt. `UserPromptSubmit` fires for every
  prompt, that wake included; whatever the hook prints to stdout is in
  the model's context on that same turn (a hook exit 2 there would
  *erase the prompt*). The payload carries no origin field — its
  schema defines `source` but the emitter is compiled out — only
  `prompt`, which is the literal notification XML on a wake. `Stop`
  is cancelled with an interrupted turn, and its payload lists the
  session's `background_tasks`.
- **Codex CLI** runs `sleep` inside the turn, so the stop hook already
  covers anything posted meanwhile. Its background `exec_command` is
  model-polled and wakes nothing. `UserPromptSubmit` fires once per
  human prompt; a stop-hook block is injected as `<hook_prompt>` in
  the same `turn_id` and does not re-fire it. `additionalContext`
  lands as a `developer` message.
- **Copilot CLI** runs `sleep` inside the turn too. A detached shell's
  completion fires the `notification` hook, not a turn — but that
  hook's `additionalContext` is queued as a `system`-sourced user
  message that starts a turn of its own, on an idle agent as well
  (verified on 1.0.82 with gpt-5-mini: a prompt that ended with
  "started" was woken and answered the hook's context). It never
  lands mid-turn; while a turn runs it waits for the turn to end.
  `userPromptSubmitted` re-fires on **every** `agentStop` block with
  no field to tell a continuation from a human prompt; a config-file
  command hook's `additionalContext` reaches the model, contrary to
  its documentation.
- **VS Code** could not be run; its `Stop` is documented as per
  session, its `UserPromptSubmit` injection as unclear.

Two properties of 0040 make more delivery points safe: a delivery is
recorded per message, so a thread handed over at one point is silent
at every other; and the nag is counted in *checks*, which only a
turn-end records. The same ledger answers 0040's reason for rejecting
per-tool-call hooks — "they fire inside subagents and on every edit,
which is the spam this record exists to avoid": a subagent is silent
by `agent_id`, and a hook that says nothing unless a *new* message
exists is not spam, only a process spawn. Verified on Claude Code
2.1.251 the same day: a `PostToolUse` hook's
`hookSpecificOutput.additionalContext` is in the model's context
before its next step of the same turn.

## Decision

- `fathomable pending --hook <harness>` recognises the **prompt-submit
  event** — `hook_event_name` of `UserPromptSubmit` or
  `userPromptSubmitted`, or, where a harness omits the name, a `prompt`
  string in the hook JSON, which no stop payload carries — and then
  composes the blob as *context* rather than a blocked stop: plain
  stdout for Claude, `hookSpecificOutput.additionalContext` for Codex
  and VS Code, `additionalContext` for Copilot, exit 0 in every case,
  and exit 0 with nothing whenever there is nothing deliverable. The
  same silences as before apply: no subscriber, a subagent, no store.
- It recognises the **post-tool-use event** the same way —
  `PostToolUse`/`postToolUse`, or Copilot's `toolName` — and answers
  it as context too, `hookSpecificOutput.additionalContext` under the
  event's own name (`additionalContext` for Copilot). Copilot's
  **`notification`** event (`hook_event_name` `Notification`) is a
  fourth, Copilot-only point answered as context the same way; the
  harness queues it as a message that opens a turn, so it is the one
  hook there that reaches an idle agent, gated on a detached shell of
  the agent's own finishing.
  This hook is **optional**: it costs a process spawn and two small
  file reads per tool call, and buys delivery *within* a long turn,
  after the next tool result. The guide presents the three points as
  a cadence the user picks from; the stop hook is the one that must be
  installed, the other two refine it.
- The occasion is the one `hello` uses on a resume, now named
  `Occasion::Context`: fired watches and fresh deliveries are recorded,
  **no check is counted and no reminder is ever composed** — the nag
  cadence stays measured in turn-ends, so a harness that runs its
  prompt-submit hook on every stop-block continuation (Copilot), or a
  post-tool-use hook on every call, spends nothing on it.
- The blob is the same text at both ends. The instruction line stays
  "act on each, then answer every thread in one `thread_reply`"; a
  wake is a turn the model is about to take, so it needs no different
  framing.
- `hello`, the server instructions, and the `follow` and
  `threads_pending` descriptions now say that comments arrive as a
  turn starts and ends and that the model should **not poll**
  `threads_pending` for them: it exists for the overflow the blob
  lists by id and for harnesses without hooks. After a wait, the right
  move is to end the turn.
- The guide's snippets for all four harnesses gain the prompt-submit
  and post-tool-use lines; `scripts/demo-repo.sh` installs all three
  for Claude.

## Consequences

- On Claude Code a comment posted during a background wait is in
  context when the wait ends, with no tool call. On Codex and Copilot
  a wait is in-turn and the stop hook was already enough; the
  prompt-submit hook there adds the user's fresh comments at the start
  of a human turn and costs nothing otherwise.
- Delivery at a turn start is context the model may ignore, as on a
  resume; the stop hook remains the forced channel and the nag remains
  its reminder. A thread shown at a turn start and ignored counts as
  *stale* at the turn's end like any other.
- `hooks.rs` now backs this record; 0040 keeps `agents.rs` and
  `wake.rs`, and its "no per-tool-call nudges" is superseded by the
  optional post-tool-use hook. `--prompt` is unchanged and still
  composes as a turn-end, since `Space w` and a hand wake are forced
  turns.
- A thread delivered after a tool result mid-turn is context the model
  may act on at once or after finishing its step; either way it is
  recorded and the stop hook will not repeat it. On a harness with no
  usable post-tool-use output the hook is silent and harmless.
- Copilot's `notification` hook (`shell_detached_completed`) was
  tested and adopted as a context point; a VS Code run is left as an
  acceptance test and does not change the shape decided here.
