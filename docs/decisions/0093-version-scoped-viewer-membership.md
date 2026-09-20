---
type: Decision
title: Version-scoped viewer membership and durable landing
description: Normal viewer surfaces follow the accepted comparison, while immutable origin evidence and exact landed commits preserve review identity across HEAD transitions.
resource: crates/fathomable/src/app/landing.rs
related_resources:
  - crates/fathomable/src/app/threads/visibility.rs
tags:
  - annotations
  - architecture
  - configuration
  - decision
  - git
  - input
  - review-points
  - sessions
---

# 0093 Version-scoped viewer membership and durable landing

Status: accepted (2026-09-20)

Amends [0087](0087-global-comparisons-and-board-history.md) and
[0092](0092-per-call-commit-sources.md). Comparisons remain checkout-wide,
explicit history remains repository-wide, and MCP source selection remains an
immutable-origin query.

## Context

ADR 0087 deliberately made the board independent of Git ancestry. That
preserves history, but normal viewer surfaces then show discussions whose
creation version is absent from the content being displayed. It also treated
the selected `HEAD` endpoint as a pinned commit even when the user intended to
keep reviewing `HEAD` against the working tree.

Working-tree, index, and review-point origins are not commits. After their
exact file bytes are committed, however, the viewer needs a durable way to
recognize the first observed matching commit without rewriting the immutable
creation evidence. Review-point identity must likewise remain distinct from
its baseline commit and from another point with the same baseline.

## Decision

### Accepted presentation defines normal membership

One predicate gates inline marks and stubs, the Thread list, the normal Reviews
board and its counts, proposed totals, File-list paths and circles, directory
counts, and open-thread traversal. It uses the last successfully installed
presentation, including the `HEAD` captured with that result, rather than
request-time endpoint selections.

A commit origin matches an equal displayed Target. A Base-side origin may
also match an equal Source in an active diff; Target and unspecified origins
have no Source exception. WorkingTree and Index endpoints represent the
accepted checkout `HEAD`. Diff Off has only a Target route. A pending or
failed replacement retains a successful active presentation, while an
unavailable Off Target has no commit context.

Unlanded WorkingTree and Index origins, EmptyTree, and Unknown retain their
legacy placement behavior. A ReviewPoint origin instead matches only its exact
accepted Source point. A landed WorkingTree or Index origin matches its landed
commit on either active endpoint; a landed ReviewPoint matches either that
commit or its exact point. Point deletion removes only the point route.
Exact scoped membership overrides ancestry and foreign-worktree placement.

Active or parked reply/edit drafts temporarily retain their parent in the
normal board and marks, but not unrelated circles, directory counts,
workspace traversal, or notifications. Normal sidebar and file indicators do
not inherit Recently resolved or Archived view state. Explicit history,
archive/restore, Clear board, and direct maintenance access remain
repository-wide.

### Immutable origins gain first-write landing

Annotation format **6** requires WorkingTree, Index, and ReviewPoint origins
to carry their owning checkout identity and a SHA-256 digest plus byte length
of the complete source file. Review-point provenance uses the point's owner,
not the checkout from which it was opened.

An append-only landing event records `landed_commit` separately from origin,
placement, and lifecycle. A bounded ordered worker compares only the recorded
path with a regular blob from the recorded checkout and repository. Git reads
occur outside the annotation lock; persistence rechecks immutable evidence
under lock. The first successfully persisted exact match wins. Duplicate
matches are idempotent, conflicts are ignored, and landing changes neither
activity, modification time, lifecycle, placement, board revision, nor
auto-resolve authority. Archived candidates can land; deletion wins.

Reconciliation runs at startup, store reload, local creation, and active
checkout HEAD observation. It has one running request, retains the oldest
queued request, and coalesces further triggers to the newest captured retry.
Candidate paths and aggregate content bytes are bounded before untrusted work
where possible, failures are explicit, and repository and checkout bindings
are revalidated. Landing jobs retain their captured checkout ownership and
commit across viewer worktree switches and later worker execution.

### HEAD and Index transitions preserve intent and evidence

The viewer observes typed symbolic, detached, unborn, and unavailable HEAD
states before refreshing comparison state. Only same-reference symbolic
movement affects a `FollowHead` Source with WorkingTree Target. Persistent
`diff.head-transition` policy is `ask-pin`, `ask-follow`, `pin`, or `follow`.
Every policy immediately displays new `HEAD -> WorkingTree`; pinning drops the
HEAD alias and following retains it.

Ask policies create a persistent prompt independent of the timed toast queue.
It opens from the prompt, Diff menu, or `Space d d`, and remains when
`watch.toast` is zero until acted on, dismissed, or superseded. Actions verify
the checkout, transition identity, HEAD state, expected endpoints, and Source
intent. Branch switches and detached, unborn, unavailable, or linked-worktree
movement do not mutate symbolic intent.

An accepted Index Source uses one immutable manifest for path enumeration,
blob reads, diffing, fingerprinting, and whole-tree equality. The manifest
covers raw paths, modes, object IDs, and index flags; ambiguous or unsupported
forms fail closed. If that pre-transition complete tree equals the new commit,
the persistent choice is **Pin new commit** or **Keep Index**. Per-file landing
is independent of this whole-tree proof. Diff Off never reads Source to
manufacture the choice.

### Explicit history and MCP stay repository-wide

Unselected MCP `threads` reads remain repository-wide and independent of the
viewer. A selected MCP source continues to match only exact immutable
`OriginVersion::Commit` creation origins and `origin.path`. A landed mutable
or review-point origin therefore does not enter `threads { source: ... }`.
Landing changes presentation membership, never MCP source identity.

## Clean format boundary

There is no reader, writer, upgrader, backfill, alias inference, or
dual-format path for format 5 annotations or the previous comparison
preference shape. They fail explicitly. Before manually deleting incompatible
app-owned state, stop every affected viewer and MCP process; then start only
the new build. Build, install, and startup never delete the state
automatically.

## Consequences

- Normal review context is precise without deleting repository history.
- Immutable origins remain suitable for audit and MCP source filtering.
- Exact landing can recover after restart but claims only the first observed
  persisted match, not the first historical commit that ever contained bytes.
- Accepted-state and generation checks prevent pending or stale selections
  from granting visibility or changing persistent Source intent.
- Operators accept a deliberate clean reset before this pre-release format
  boundary; compatibility behavior is intentionally rejected.
