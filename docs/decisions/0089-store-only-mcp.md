---
type: Decision
title: Store-only MCP and independent thread refresh
description: MCP writes directly to the shared thread store; viewers observe it without a socket protocol or the workspace debounce.
resource: crates/fathomable/src/mcp/mod.rs
tags:
  - annotations
  - architecture
  - decision
  - sessions
---

# 0089 Store-only MCP and independent thread refresh

Status: accepted (2026-09-18)

Supersedes the remaining viewer-socket transport in
[0014](0014-mcp-server-and-socket-v1.md),
[0024](0024-workspace-sessions.md), and
[0082](0082-three-tool-review-core.md). It amends the watcher contract of
[0028](0028-live-workspace.md) and the socket-specific version rules in
[0062](0062-one-version-no-compatibility.md) and its amendments.

## Context

The three repository-bound MCP tools operate on durable discussions. Reading
already uses the store directly. Starting and replying used a selected
viewer's socket when one appeared to be running, and the store otherwise.
Both paths ultimately used the same locked annotation operations. Other
viewers already observed the resulting store changes through the filesystem.

The extra transport supplied neither stronger caller identity nor a
terminal-render acknowledgement. It added viewer discovery, a second
protocol, listener lifetime, version coupling, duplicate write wrappers, and
failures caused by stale or unavailable viewers. Low-latency thread
observation belongs in the filesystem watcher, not another write path.

## Decision

### One bound store path

`fathomable --mcp [DIR]` always reads and writes the shared thread store for
its startup-bound repository and checkout. Viewer registrations, names,
availability, and active comparisons never choose a write path. The public
MCP surface remains `threads`, `thread_start`, and `thread_reply`, over stdio.
No socket, signal, broker, or replacement viewer-control protocol is added.

The [MCP contracts](0084-explicit-mcp-contracts.md) and
[atomic reply authority](0085-thread-lifecycle-and-auto-resolve.md) remain:
whole-batch prevalidation, request-order item results, per-item locked
transactions, caller-scoped idempotency, and explicit partial failures.
One-shot permission consumption, relocation, resolution, and the original
retry outcome remain part of the same durable reply operation. A successful
call reports the store operation, not that any terminal has displayed it.

Harness identity still records authorship; it does not select the checkout
or authenticate against hostile same-UID processes. Fresh source reads stay
[confined to the bound checkout](0061-agents-start-threads.md#checkout-confined-reads).
The [private-state contract](0009-cli-and-diagnostics.md#persistent-state-privacy)
continues to protect persisted data against other local users.

### Checkout placement does not depend on a viewer

MCP placement and line relocation use the stored path in the bound checkout.
An absent line source is not recovered through a viewer's remembered rename.
File-wide replies and matching idempotency retries keep their existing rules.
If the original path is recreated, MCP uses that bound-checkout path rather
than an unrelated viewer-local projection.

The viewer keeps its ephemeral rename projection for display, as specified
by [0087](0087-global-comparisons-and-board-history.md). It is not promoted
into a shared rename registry or a second agent-write interpretation.

### Thread observation is independent of workspace debounce

Relevant thread-store notifications request reconciliation without waiting
for `watch.debounce`. Workspace and Git changes retain their configurable
quiet-period batching. Thread notifications are coalesced, and notification
processing is bounded per loop turn so a continuously refilled queue does
not monopolize the viewer. Access-only notifications do not trigger reloads.

Reconciliation reloads the durable board, observes agent activity, and
refreshes reach, marks, and visible content without changing the user's
navigation, comparison, or draft. Startup seeds activity without replaying
history. Duplicate notifications and keyed retries do not announce an
operation again; several newly observed operations may produce one counted
activity toast. Replaced or regressed histories retain the existing silent
cursor-reseeding behavior rather than promising exactly-once delivery across
arbitrary external log replacement.

The viewer reconciles after installing or replacing watches to close the
initial-read/watch-install gap. It also reconciles after returning from an
external editor. MCP may persist operations while that editor blocks the
viewer; terminal rendering resumes only after control returns.

This is not a fixed millisecond latency guarantee. Scheduling, queued work,
store reads, and rendering still take time. Once observed, thread dirtiness
does not wait for unrelated filesystem activity to become quiet.

### Known watch failures recover explicitly

State observation watches the store directory and its immediate parent,
non-recursively, with parent notifications restricted to structural changes
affecting that store directory. Relevant replacement, removal, rescan, or
notifier-error signals invalidate stale watch bookkeeping and request
reattachment and reconciliation.

Failed thread refresh or coverage is visible as degraded thread updates.
Recovery retries at a bounded rate while degraded, without allowing incoming
events to continually postpone the deadline. Retries stop once coverage and
reconciliation recover; there is no healthy-mode polling.

If a previously observed store disappears, the viewer keeps the last
successful board and activity cursor while reporting the failure. Read,
parse, and format failures likewise do not install an empty success-shaped
replacement. Refresh does not recreate or overwrite missing data. A genuinely
absent store on first launch still represents an empty board.

These measures recover known loss; they do not guarantee notification
delivery across arbitrary silent operating-system or filesystem failures.

### Remove transport, retain viewer metadata

The viewer listener, socket requests and responses, protocol version, peer
checks, socket paths, runtime-directory preparation, and socket-specific
diagnostics retire. `--viewers` no longer reports a socket column, and
`--doctor` no longer checks runtime socket paths. No product socket requires
`XDG_RUNTIME_DIR`.

Viewer IDs, names, registrations, workspace markers, worktree bookkeeping,
logs, and crash context remain. Dead-record cleanup removes records, not old
socket paths. Existing runtime artifacts are unused and are not swept during
startup or upgrade.

The annotation format remains **5**. This change adds no migration, reset,
compatibility transport, or replacement protocol counter. Restart viewers
and MCP processes when upgrading; unchanged storage does not promise
mixed-build interoperability. The broader
[alpha upgrade policy](0083-single-user-alpha-clean-slate.md#enthusiast-alpha-contract)
and explicit operator control of incompatible state remain.
