---
type: Decision
title: Automatic chat identity
description: Harness-qualified caller identities come from launch environment or per-call MCP metadata, independently of workspace routing, optional profiles, and opt-in delivery.
resource: crates/fathomable/src/caller.rs
related_resources:
  - crates/fathomable/src/mcp/identity.rs
tags:
  - annotations
  - architecture
  - decision
  - sessions
---

# 0080 Automatic chat identity

Status: accepted (2026-09-15)

Amended 2026-09-15 by [0082](0082-three-tool-review-core.md):
automatic harness-qualified identity remains exactly for authorship; reads
may be anonymous and writes require it. Subscription profiles, delivery,
watches, per-call workspace routing, and viewer navigation tools are removed.
The harness channels and lifecycle limitations below remain authoritative.

Supersedes [0041](0041-session-bonds.md). Amends the identity and
subscription contracts of [0040](0040-agent-subscriptions-and-hooks.md),
[0055](0055-six-tools.md), [0058](0058-the-user-has-the-last-word.md),
and [0061](0061-agents-start-threads.md), the routing contract of
[0014](0014-mcp-server-and-socket-v1.md), and the bootstrap part of
[0042](0042-turn-start-delivery.md).

## Context

A chat's identity should not be something a model copies from a hook,
invents, or remembers to pass. Process ancestry and the last subscription
on a connection are not reliable chat boundaries: a harness may share an
MCP server between chats or subagents. They also conflate four separate
questions: who called, which workspace the call addresses, how the author
is displayed, and whether comments should be delivered automatically.

The supported harnesses expose native identity channels. Use those
channels directly, with their actual lifecycle limits, rather than
maintaining process bonds or shipping native harness extensions.

## Decision

### One harness-qualified identity per caller

Select the adapter from the MCP client's harness identity, then read
only that adapter's channel. An inherited variable belonging to another
harness is never fallback evidence. Per-request `clientInfo` takes
precedence over legacy initialization's client information through
rmcp's request context.

| Harness | Identity channel | Stored caller and subscriber key |
| --- | --- | --- |
| Copilot CLI | launch environment `COPILOT_AGENT_SESSION_ID` | `copilot:<native>` |
| Claude Code | launch environment `CLAUDE_CODE_SESSION_ID` | `claude:<native>` |
| VS Code | each call's `params._meta["vscode.conversationId"]` | `vscode:<conversation>` |
| Codex | each call's `params._meta.sessionId` and `params._meta.threadId` | `codex:<thread>` |

Codex requires both fields. `params._meta.sessionId` is root/family
context, not the concrete resumable chat identity; the stored key uses
`threadId` alone. Two threads under one root remain two callers and two
subscriptions. A missing `threadId` cannot be repaired by substituting
`sessionId`, and a call missing either field has no usable identity.

These are capability contracts, not a claim that every stable release
emits them:

- A live Copilot CLI 1.0.84-8 experiment found distinct MCP children for
  separate top-level chats and a shared process for their subagents.
  This supports launch identity for those top-level chats; it does not
  validate every older version or give subagents distinct identities.
- Claude identity is fixed when the MCP child launches. To use a
  different Fathomable identity, start a **fresh Claude invocation**.
  To resume a particular chat, use a fresh invocation with
  `claude --resume <id>`. `/clear` and in-process resume are unsupported
  as distinct identity boundaries. Implicit `--continue` or `--resume`
  without an explicit identifier is not a guarantee that the intended
  chat's launch identity reaches MCP.
- Copilot and Claude subagents sharing the parent's identity are an
  accepted limitation. The adapter does not synthesize a subagent id.
- Codex source was inspected at v0.155-alpha. Deployment requires a
  version that actually emits the metadata above; older or stable
  versions are not covered merely because they speak MCP.
- VS Code requires the conversation metadata on the call. A gateway
  that strips it does not meet the contract.

Unknown clients and missing identity channels may read
threads and use viewer navigation, but cannot write annotations,
subscribe, or manage watches. Supplied identity metadata of the wrong
type or with an empty value is rejected, not treated as an anonymous
request. Errors identify the expected harness
channel, not a UUID the user should retrieve. No folder, PID, process
ancestor, remembered subscription, or tool argument supplies identity.

### Identity, profile, and delivery are separate

- Remove explicit caller `id` arguments from `follow`, `threads`,
  `thread_reply`, `thread_start`, and `thread_watch`. Removed arguments
  have no compatibility aliases and are rejected as unknown arguments.
- Every annotation write carries the automatically resolved caller id,
  even before `follow`. Without a subscription, its display name comes
  from the harness and it has no subscribed type or persona.
- `follow` takes `type`, `persona`, `end`, and `workspace`. A first
  subscription requires a configured `type`; `persona` is optional.
  These establish a display profile for the subscription, not identity.
  A live subscription's profile is looked up in the addressed workspace
  on each call, never cached across the connection or borrowed from
  another workspace. Its name and type cannot change while that
  subscription is live.
- `follow` without `type` reports status without mutation, unless
  ending the subscription; identifying a caller does not subscribe it.
  `follow` with `end` ends that caller's subscription,
  deliveries, and watches in the addressed workspace.
- Automatic delivery requires both a live subscription and supported
  delivery hooks. `threads` is usable without identity; only a live
  subscriber's read records deliveries and consumes its fired watches.
  `thread_watch` requires both identity and a live subscription.
- The user's-last-act rule, delivery ledger, expiry, and watch behavior
  of [0040](0040-agent-subscriptions-and-hooks.md) and
  [0058](0058-the-user-has-the-last-word.md) remain unchanged.

### Workspace routing is per call, never chat state

`fathomable --mcp [DIR]` uses `DIR`, or its startup cwd when omitted,
as its immutable default workspace anchor. Per-call `workspace` accepts
a worktree root or a viewer name or id; `open` retains its `viewer`
selector. An override applies only to that call.

`workspaces` takes no arguments. It lists known workspaces, their roots
and viewers, and the default determined from the startup anchor. Remove
`workspaces.switch` and the connection-wide mutable pin. Missing or
ambiguous routes fail explicitly and direct the caller to list
workspaces and provide a per-call selection; they are not guessed.

Metadata chat identity never chooses a workspace. Shared MCP clients
must provide a per-call override when they want a workspace other than
the server's startup default. A subagent using another worktree must
likewise address it explicitly if its server was launched elsewhere.
The shared-store and worktree rules of
[0070](0070-one-workspace-many-worktrees.md) still apply.

### Hooks deliver; they do not bootstrap identity

Remove the `hello` CLI subcommand and text, its recommended
`SessionStart` hook, the process-bond module, and register bond
events/APIs. Connecting does not add a session-identification paragraph
or extra prompt metadata/context. Native harness extensions are not
needed.

Keep `fathomable pending` on the supported prompt-submit, post-tool,
stop, and Copilot notification events of
[0042](0042-turn-start-delivery.md). It is silent unless a subscription
exists and something is due under the delivery, watch, or reminder
rules. Identity detection alone establishes no delivery channel. There
is no initial resume delivery: the next supported prompt or stop hook
handles pending work.

Hook payloads supply raw native ids, normalized with the selected
harness to the same qualified keys used by MCP. The diagnostic/manual
CLI option `pending --id` remains: with `--hook`, pass a raw native id;
without `--hook`, pass the already-qualified subscription key. This is
not an MCP identity override.

The viewer stores qualified subscriber keys but passes the **raw native
id** as `{id}` to `agents.wake`; harness resume commands need their
native identifier, not Fathomable's namespace.

## Consequences

- A model cannot rebind a shared connection by choosing an id or
  workspace pin. Per-call metadata isolates chats where the harness
  provides it; launch identity has explicit, narrower lifecycle limits.
- Reading remains useful on unsupported clients. Writing fails rather
  than creating an unattributed annotation or signing as another chat.
- This is an approved **alpha reset** boundary under
  [0062](0062-one-version-no-compatibility.md): no old argument aliases,
  bond compatibility, or subscription-key migration. Old bond-bearing
  agent registers are incompatible; unqualified subscriber keys do not
  identify current callers. Affected agent state must be deliberately
  reset and subscriptions established anew, not silently migrated.
  Removing a code path does not itself reset state, authorize wiping
  annotation stores, or edit installed harness configuration. An old
  Fathomable `hello` hook entry must be removed from that configuration;
  the delivery hooks remain.
- `caller.rs` owns harness selection and identity normalization;
  `mcp/identity.rs` adapts MCP request context to it. The historical
  bond record remains discoverable without owning the deleted module.
  The [guide](../guide.md#connect-an-agent) describes the current
  setup, not the superseded bootstrap.
