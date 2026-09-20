---
type: Decision
title: Explicit review membership, retained focus, and durable landing
description: Threads keep immutable review association while checkout-local focus and accepted presentation independently control lists and inline projection.
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

# 0093 Explicit review membership, retained focus, and durable landing

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

### Association, focus, presentation, and placement are separate

Every thread has four independent concepts:

1. immutable origin evidence records bytes, version, side, and observed
   comparison;
2. immutable `ReviewAssociation` records deliberate `ReviewScope` membership
   or an exact typed content endpoint;
3. accepted presentation records what Source and Target reached the screen;
   and
4. placement records where the thread can currently be projected and acted on.

`ReviewScope` is a typed commit review, an explicit immutable comparison, or a
checkout-qualified mutable task with its own stable identifier. Endpoint
identity distinguishes Commit, WorkingTree, Index, ReviewPoint, EmptyTree, and
Unknown. It never equates WorkingTree or Index with their observed `HEAD`, and
ReviewPoint identity never collapses to its baseline commit.

The normal review list and open-thread work queue include the native scope of
the accepted presentation plus the active retained review focus. A commit
review enters ordinary surfaces only through its matching focus; merely using
that commit as Source or as the accepted mutable `HEAD` grants no membership.
An exact explicit comparison can be native when the accepted endpoint pair
matches. A mutable review is task-bounded and remains a member only while its
explicit focus is retained. Content-only annotations match their exact typed
displayed endpoint.

Inline projection is a separate decision. Normal and Unified may project any
normal member whose anchor or context locates in displayed content; detached
members remain in the list and remain reply/resolution-actionable. Diff Off
keeps retained members in the list but renders marks only on exact Target
content. Base-side origins do not become Target content through landing. Clean
versus dirty state and Normal versus Unified never change review membership.

Active or parked reply/edit drafts temporarily retain their parent in the
normal board and eligible marks, but not unrelated circles, directory counts,
workspace traversal, or notifications. Normal sidebar and file indicators do
not inherit Recently resolved or Archived view state. Explicit history,
archive/restore, Clear board, and direct maintenance access remain
repository-wide.

### Review entry and retained focus are explicit

`Commit~1 to Commit…` and `HEAD~1 to HEAD` start a commit review and retain that
commit as active focus. A real root uses `EmptyTree -> C`; merge commits use
their first parent. A missing parent object fails instead of being treated as a
root. The first-parent route is navigation, not fabricated origin evidence, so
contextless MCP starts at Commit(C) have unspecified side and associate
directly with Review C.

`Focus current comparison` starts an explicitly comparison-specific review, or
a checkout-qualified mutable task when either endpoint is WorkingTree or
Index. `Clear review focus` is the explicit lifetime boundary. Ordinary
endpoint and diff-mode changes never replace or clear focus. Human starts copy
the active scope; without a focus they are content-only annotations on their
exact typed origin endpoint. `ComparisonFacts` remain observed evidence and do
not imply review intent.

The focus is stored in the checkout-specific comparison preference and is
validated against its owning checkout when loaded. It survives endpoint
changes, HEAD and branch movement, clean/dirty transitions, and restart. A
linked-worktree switch loads that worktree's separate focus, so a mutable scope
cannot leak across checkout boundaries. The status badge and Review menu show
the active focus.

### Immutable origins gain first-write landing

Annotation format **7** requires WorkingTree, Index, and ReviewPoint origins
to carry their owning checkout identity and a SHA-256 digest plus byte length
of the complete source file. Review-point provenance uses the point's owner,
not the checkout from which it was opened.

An append-only landing event records `landed_commit` separately from origin,
association, placement, and lifecycle. A bounded ordered worker compares only
the recorded path with a regular blob from the recorded checkout and
repository. Git reads occur outside the annotation lock; persistence rechecks
immutable evidence under lock. The first successfully persisted exact match
wins. Duplicate matches are idempotent, conflicts are ignored, and landing
changes neither activity, modification time, lifecycle, placement, board
revision, nor auto-resolve authority. Archived candidates can land; deletion
wins.

Reconciliation runs at startup, store reload, local creation, and active
checkout HEAD observation. It has one running request, retains the oldest
queued request, and coalesces further triggers to the newest captured retry.
Candidate paths and aggregate content bytes are bounded before untrusted work
where possible, failures are explicit, and repository and checkout bindings
are revalidated. Landing jobs retain their captured checkout ownership and
commit across viewer worktree switches and later worker execution. Landing
never replaces association or makes a thread disappear. It may provide an
exact Diff Off Commit(C) content route or a related active Review-C route, but
never grants membership merely because C is a mutable presentation baseline.

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

Annotation format **7** requires every origin provenance record to contain its
typed review association. Comparison preference format **2** requires an
explicit optional retained focus. There is no reader, writer, upgrader,
backfill, association inference, or dual-format path for older annotation or
comparison preference shapes. They fail explicitly. Before manually deleting
incompatible app-owned state, stop every affected viewer and MCP process; then
start only the new build. Build, install, and startup never delete the state
automatically.

## Consequences

- Review membership survives presentation changes without conflating commits
  with mutable endpoint aliases.
- Immutable origins remain suitable for audit and MCP source filtering.
- Mutable review tasks have an explicit start/clear boundary and stable
  checkout-qualified identity across edits and staging changes.
- Exact landing can recover after restart but claims only the first observed
  persisted match, not the first historical commit that ever contained bytes.
- Accepted-state and generation checks prevent pending or stale selections
  from granting visibility or changing persistent Source intent.
- Operators accept a deliberate clean reset before this pre-release format
  boundary; compatibility behavior is intentionally rejected.
