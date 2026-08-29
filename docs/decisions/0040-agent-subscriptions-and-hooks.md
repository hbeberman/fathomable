---
type: Decision
title: Agent subscriptions, pending threads, and harness hooks
description: An agent subscribes to a workspace once with its harness session id and a configured agent type; a thread is pending for it when the newest message is someone else's; a harness stop hook delivers each pending thread once as a self-contained prompt; watches wake an agent when another thread moves; and no unsubscribed session ever hears from Fathomable.
resource: crates/fathomable-core/src/agents.rs
related_resources:
  - crates/fathomable/src/hooks.rs
  - crates/fathomable/src/app/wake.rs
tags:
  - decision
  - sessions
  - annotations
  - configuration
---

# 0040 Agent subscriptions, pending threads, and harness hooks

Status: accepted (2026-08-29)

## Context

Every bullet of the [charter](../charter.md) has shipped. The weakest
link left in its loop — *the user reads and responds while the agent
writes* — is the delivery of a comment to the agent.
[0014](0014-mcp-server-and-socket-v1.md) left agents to poll
`annotations_list since=<ts>` and own their cursor, and parked a
server-side cursor "until polling proves inadequate". It has: an agent
in the middle of a task does not poll, so a comment sits until the
task ends or the user pastes it into the chat by hand. The human side
has *waiting* threads, toasts, and `]r` ([0030](0030-waiting-threads.md));
the agent side has nothing.

Settled in a long discussion on 2026-08-28 and 2026-08-29, with research
into the four harnesses the project cares about (Claude Code, Codex CLI,
Copilot CLI, VS Code agent mode). What the research established, and
what it rules out:

- **A held MCP call is not a delivery mechanism.** Every harness times
  MCP calls out (30 s to 28 h), none lets a server wake the model, and
  the 2026-07-28 spec says blocking is "impractical beyond a few
  seconds". Server notifications reach the model in none of the four.
- **All four harnesses run command hooks with one contract.** A
  `SessionStart` hook's stdout is added to the model's context; a
  `Stop` hook (Copilot: `agentStop`) runs synchronously when the model
  wants to end its turn, and may answer *block* with a `reason` that the
  harness feeds back as the next prompt, so the same turn continues.
  All carry `stop_hook_active` on the continuation; three cap the loop
  at 8, VS Code does not. Hook stdin carries a session id and `cwd`.
- **Subagents never see `SessionStart`.** Claude and Codex subagents
  share the parent's `session_id` and carry `agent_id`; their `Stop` is
  main-thread only. Copilot subagents get their own `sessionId` and do
  run `agentStop`, but a block there is ignored. VS Code subagents carry
  `agent_id`. In every case a subagent has no way to have registered.
- **The MCP server cannot learn the harness session id** in Claude,
  Codex, or VS Code (Copilot sets `COPILOT_AGENT_SESSION_ID`); the
  model has to be told it.
- **Tool hygiene.** Tool definitions are re-sent every turn and Claude
  Code now defers MCP tools behind a search that indexes names and
  descriptions; spec defaults mark a tool without annotations as
  destructive; Codex runs MCP calls serially unless opted in; Claude
  Code caps a result at 25k tokens.

The discussion's turning points, kept here because they shape the
design:

- Delivery must be *opt-in per session and silent otherwise*. A
  coworker who tried Fathomable once must not have it reading their
  other repositories or spending their tokens. The hook therefore
  does nothing unless the session registered itself over MCP first.
- Delivery must be *idempotent*: a pending thread is sent to a session
  once, and if the agent ignores it that is the agent's problem. A
  configurable nag stands in for repetition.
- Routing threads to agent types was rejected as too much concept.
  Agent types are labels: an agent declares one, the viewer shows it,
  and the only rule is that an agent is not woken by its own messages.
- The type list is configuration, not an enum in core, so a user can
  shape it without a release and an agent cannot invent one at runtime.
- "Defer" was the wrong primitive: a delivered thread already stays
  quiet until someone else speaks. What an agent needs is a *watch*:
  "remind me about this thread when that one resolves".
- OMP (oh-my-pi) is out of scope; its hooks are TypeScript modules and
  it can push into the model itself, so an integration belongs on its
  side. VS Code is in scope but last: its Agent Host gateway strips
  `_meta`, hook placement there is undocumented, and users report
  `agentStop` not firing after session start (vscode#300193), stop
  hooks double-firing (#301797), and Claude's `Stop` hook silent inside
  the extension (claude-code#40029); parity is an acceptance test, not
  an assumption.

## Decision

### Subscriptions

- `agents.rs` in `fathomable-core`, which this record backs, holds the
  workspace's **agent register**: an append-only JSONL file beside the
  thread store, `$XDG_STATE_HOME/fathomable/workspaces/<hash>/agents.jsonl`,
  written the way `threads.jsonl` is ([0032](0032-placement-and-state.md):
  one `write_all` per event, so a viewer, a headless `--mcp`, and a
  hook process may all append).
- A **subscriber** is `{id, type, name, client, created, seen}`. `id` is
  the harness session id, passed by the agent; `type` is one of the
  configured agent types; `name` and `client` are the persona and MCP
  client of [0014](0014-mcp-server-and-socket-v1.md). The user is
  never a subscriber.
- `follow` gains optional `id` and `type`. With both, the call
  **subscribes** the session (or refreshes its paths) before it tells
  the viewers; with neither it only tells the viewers, as today. `id`
  without `type`, or a `type` not in the configuration, fails the call
  and names the allowed types. A subscriber's type is fixed for its
  life: `follow` with another type is refused. `follow` no longer fails
  when no viewer runs; it says how many it reached.
- `unfollow {id}` removes the subscription, its deliveries, and its
  watches. A subscription also **expires** when `seen` — refreshed by
  every `follow`, `threads_pending`, and hook lookup — is older than
  `agents.expire-after`. Expired records are dropped on load.
- A subscriber's **scope** is: every thread on a path in its follow
  list (an empty list means the whole workspace), plus every thread it
  has posted in. Nothing else is registered per thread.

### Who wrote what

- `Author::Agent` gains `id: Option<String>` and `kind: Option<String>`
  (the agent type; `kind` in code because `type` is a keyword). A reply
  through a subscribed `--mcp` connection carries both; an unsubscribed
  one carries neither, as today. The wire form stays the untagged
  `AuthorWire`; the new fields are optional and older files load.
- The thread pane, the file-threads pane, and the thread list label an
  agent's message `name (type)` — `name (client)` when it has no type.
  (Filtering a list by author was considered and left for a later
  record; nothing in the thread list searches yet.)

### Pending

- A thread is **pending for subscriber S** when it is open, in the
  current git scope ([0024](0024-workspace-sessions.md)), in S's scope,
  and its newest message — the comment when there are no replies — was
  not written by S (`Author::Agent { id: Some(S) }`). The predicate is
  `Thread::pending_for(&Subscriber)`, beside `awaits_user`
  ([0030](0030-waiting-threads.md)); *waiting* is the same idea seen
  from the user's chair.
- A **delivery** is `{subscriber, thread, at}` where `at` is the
  `created` of the newest message when it was delivered. A pending
  thread is **deliverable** when no delivery exists for its newest
  message. Delivering appends the record, so one message reaches one
  session once, however many hooks fire.
- A **check** is appended each time a subscriber's hook asks and finds
  a thread delivered but still pending. Every `agents.nag-after`
  consecutive checks with such threads, the hook emits a one-line
  reminder naming their paths, no bodies; `0` disables it.

### Watches

- `thread_watch {id, on, when, remind}`: wake subscriber `id` when
  thread `on` next gets a message from someone else (`when: "message"`)
  or is resolved (`when: "resolved"`), and re-deliver the threads in
  `remind` (possibly empty) in full at that time. A watch fires once
  and is removed. `thread_unwatch {id, on}` removes it early. Watches
  are register events; a fired watch is appended as such.
- The thread pane's header lists `watched by name (type)` under a
  watched thread.

### Delivery

- `hooks.rs` in the `fathomable` crate is the hook side: two
  subcommands that read the harness's hook JSON on stdin, print for the
  harness named by `--hook`, and exit 0 with no output whenever there
  is nothing to say. Both resolve the workspace from `cwd` in the hook
  JSON (else the process cwd) exactly as `--mcp` does, and both exit 0
  silently when no store exists there. Neither ever writes to the
  thread store.
- `fathomable hello --hook <harness> [--id ID]` runs from
  `SessionStart`. It prints one paragraph: that Fathomable is watching
  this workspace, the session id, and the instruction to register with
  `follow(id, type, paths)` naming the configured types. It is the
  only way the model learns its own id. Claude and Codex read plain
  stdout; Copilot and VS Code read the `additionalContext` JSON they
  expect.
- `fathomable pending --hook <harness> [--id ID] [--prompt]` runs from
  `Stop` (`agentStop`). It exits 0 silently when the id is not a
  subscriber, when `stop_hook_active` is true, when `agent_id` is
  present (Claude, Codex, VS Code subagents), when Copilot's
  `sessionId` is not the session of its `transcriptPath`, or when
  nothing is deliverable and no nag or watch is due. Otherwise it
  records the deliveries and blocks the stop with the **blob** as the
  reason: exit 2 and stderr for Claude and Codex, the `decision: block`
  JSON for Copilot and VS Code. `--prompt` prints the blob to stdout
  and exits 0 instead, for piping into `claude -r`, `codex queue`, or
  `copilot --resume -p` to wake an idle session.
- The blob is plain text: a first line with the count and the agent's
  type, the instruction to act and then answer every thread in one
  `thread_reply` call, and per thread its id, `path:range`, status,
  message count, up to six snippet lines, and its newest two messages
  with author labels. Fired watches come first, headed by what fired
  them. Nothing beyond `agents.max-lines` lines: past it, the rest are
  listed as `id path:range` with "call `threads_pending`". The blob
  never carries file content beyond the snippet.
- `app/wake.rs` is the viewer's side. `Space w` runs `agents.wake` — a
  command template whose `{id}` and `{prompt}` become shell positional
  parameters — for the one subscriber, or the one picked from a picker,
  with the blob as `{prompt}`; the command runs detached with no
  terminal, so it must be non-interactive (`codex queue`,
  `claude -p -r`). A subscriber with nothing pending is left alone. It
  is the user's key, not an automatic wake; Fathomable does not own the
  agent. `:status` lists the subscribers; the thread pane header says
  `watched by name (type)` under a watched thread.

### Tools

| Tool | Change |
|---|---|
| `follow` | optional `id`, `type`; subscribes; works headless |
| `unfollow` | new: `id` |
| `threads_pending` | new: `id`, optional `limit`; the deliverable threads for a subscriber, recorded as delivered; the hookless fallback |
| `thread_reply` | optional `replies` array of `{thread, body, resolve, line, end_line}` taken instead of the single fields; one call answers a batch on every harness |
| `thread_watch`, `thread_unwatch` | new |
| `annotations_list` | optional `limit` (default 50) with a "N more; pass since=…" tail |

- Every read-only tool (`session_list`, `annotations_list`,
  `threads_pending`) declares `readOnlyHint`; the writers declare
  `destructiveHint: false`, `openWorldHint: false`, and `idempotentHint`
  where it holds. Descriptions grow to three or four sentences that say
  what the tool does not return; the server instructions keep their
  first 512 characters self-contained.
- `--mcp` loads the configuration for the type list and limits, as the
  TUI does. A connection remembers the `id` it last subscribed with, so
  a `thread_reply` on that connection is signed with it without the
  agent repeating it.

### Configuration

```kdl
agents {
    types "coder" "reviewer" "planner"   // what `follow` may declare
    nag-after 5          // stop-hook checks between reminders; 0 never
    expire-after 24      // hours a silent subscription lives
    max-lines 40         // longest blob before the rest is listed
    wake "claude -r {id} {prompt}"   // Space w; empty disables the key
}
```

### What is deliberately not here

- No routing: a thread has no addressee. Every subscriber whose scope
  holds it, other than the last speaker, is woken.
- No per-tool-call nudges (`PostToolUse`, a trailer on `follow`
  results): both fire inside subagents and on every edit, which is the
  spam this record exists to avoid.
- No server-initiated MCP traffic, no long-poll, no tasks extension.
- No hook file ships in this repository. The guide shows each
  harness's snippet and where the per-user copy goes (Claude
  `settings.local.json` or user settings; Codex must trust it via
  `/hooks`; Copilot ignores repo hooks until the folder is trusted;
  VS Code reads the same `.github/hooks` file as Copilot).

## Consequences

- An agent's turn ends normally unless it registered, someone else
  spoke on a thread in its scope, and that message has not been shown
  to it — in which case it sees one prompt and answers in one call.
  A session without the hooks, or without a `follow` call, costs
  nothing and hears nothing.
- The register is a second JSONL file with the thread store's
  concurrency properties and its strict loader; it holds no thread
  content and can be deleted to forget every subscription.
- `Author` grows two optional fields; the socket protocol's `ThreadReply`
  carries them inside `author`, so the wire version is unchanged.
- The `--mcp` process gains a second piece of state (the connection's
  subscriber id) beside the pin of 0014.
- The 0014 "server-side per-agent cursor" investigation closes: the
  delivery record is that cursor, keyed by message rather than time.
- `docs/guide.md` gains the `hello`/`pending` subcommands, the `agents`
  config node, `Space w`, the new tools, and a hooks section per
  harness, in the change that ships each.
- VS Code's `Stop` has no loop cap; `stop_hook_active` is the only
  guard, and the hook honours it before anything else.
