---
type: Decision
title: Provenance-scoped viewer membership and durable landing
description: Immutable origin provenance and the accepted presentation determine normal membership; landing remains evidence only.
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

# 0093 Provenance-scoped viewer membership and durable landing

Status: accepted (2026-09-20)

Amends [0087](0087-global-comparisons-and-board-history.md) and
[0092](0092-per-call-commit-sources.md). Comparisons remain checkout-wide,
explicit history remains repository-wide, and MCP source selection remains an
immutable-origin query.

## Context

Normal viewer surfaces need a deterministic answer to whether a stored thread
belongs to the content that actually reached the screen. Origin versions are
typed: WorkingTree, Index, Commit, ReviewPoint, EmptyTree, and Unknown are
different identities. In particular, a clean WorkingTree is not its observed
`HEAD`.

Working-tree, index, and review-point origins can later have bytes identical to
a commit. The store records the first commit where that exact full-file match
is observed, without rewriting the creation evidence. This landing fact must
not silently change which review the thread belongs to.

## Decision

### Provenance and accepted presentation determine membership

Normal membership uses immutable origin provenance and the last successfully
installed presentation. Pending selections and failed refreshes do not replace
that accepted presentation.

- A Commit(C) origin whose side is Target or Unspecified belongs when the
  actual displayed Target is Commit(C). This includes `C^ -> C`, Diff Off with
  Target C, and any other Source with Target C. A Commit(C) Source does not
  admit it when Target is WorkingTree or Index.
- A Base origin with recorded `ComparisonFacts` belongs only when the accepted
  active ordered Source and Target match both recorded facts. When either
  recorded endpoint is WorkingTree or Index, the recorded comparison checkout
  must also match the accepted presentation checkout. This retains deletion
  discussions for their comparison without admitting them to every comparison
  or linked worktree sharing the same Source and observed `HEAD`. Comparisons
  with wholly immutable endpoints remain repository-wide.
- A Base origin without `ComparisonFacts` belongs only when its exact typed
  origin matches Source in an active diff.
- WorkingTree Target or Unspecified origins match only a WorkingTree Target in
  their recorded checkout. Index follows the same rule. ReviewPoint matches
  only the exact point identity.
- Diff Off has no Source membership and admits only matching Target-side or
  Unspecified origins. Unknown or insufficient provenance grants no normal
  membership.

Normal membership does not use repository ancestry, mutable `HEAD` aliases,
landing, or repository-wide reachability. Explicit history views and direct
thread-ID operations remain available independently.

Membership and inline placement are separate. A normal member receives a mark
only through its eligible endpoint and side route and a successful anchor or
context placement. A detached member can remain listed and can still be
replied to, edited, resolved, reopened, archived, or restored.

Human starts in an active diff record `OriginVersion`, `OriginSide`, and the
ordered `ComparisonFacts`. Diff Off starts are Target-content annotations and
do not record comparison facts. The commit shortcuts select a commit against
its first parent; root commits use `EmptyTree`, merge commits use their first
parent, and an unavailable parent object fails explicitly.

There is no retained-review focus, separate review association, or mutable
identifier. Endpoint selection alone establishes the installed presentation.

### Immutable origins gain first-write landing

WorkingTree, Index, and ReviewPoint provenance carries its owning checkout
identity and a SHA-256 digest plus byte length of the complete source file.
Review-point provenance uses the point's owner, not the checkout from which it
was opened.

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

Landing is evidence only. It never adds, removes, or changes normal membership,
including Diff Off and commit routes. There is no comparison between
`landed_commit` and `observed_head` for membership.

### HEAD and Index transitions preserve intent and evidence

The viewer observes typed symbolic, detached, unborn, and unavailable HEAD
states before refreshing comparison state. Only same-reference symbolic
movement affects a `FollowHead` Source with WorkingTree Target. Persistent
`diff.head-transition` policy is `ask-pin`, `ask-follow`, `pin`, or `follow`.
Every policy immediately displays new `HEAD -> WorkingTree`; pinning drops the
HEAD alias and following retains it.

Ask policies create a persistent prompt independent of the timed toast queue.
Actions verify the checkout, transition identity, HEAD state, expected
endpoints, and Source intent. Branch switches and detached, unborn,
unavailable, or linked-worktree movement do not mutate symbolic intent.

An accepted Index Source uses one immutable manifest for path enumeration,
blob reads, diffing, fingerprinting, and whole-tree equality. If that complete
tree equals the new commit, the persistent choice is **Pin new commit** or
**Keep Index**. Per-file landing is independent of this whole-tree proof.

### MCP source remains explicit

`thread_start` with `source: Commit(C)` records immutable Commit(C) provenance
with side Unspecified. Omitting `source` records WorkingTree provenance.
Observed `HEAD` and later landing never convert that WorkingTree origin into a
commit origin. An agent reviewing a commit must pass `source`.

Likewise, `threads {source: C}` filters only exact immutable Commit(C) creation
origins and origin paths. It never includes landed WorkingTree, Index, or
ReviewPoint origins.

## Clean format boundary

Annotation format **8** removes the short-lived serialized review-association
field while retaining typed origin, checkout, digest, comparison, and landing
facts. Its ordered comparison facts qualify WorkingTree and Index endpoints
with their checkout; wholly immutable comparisons have no checkout qualifier.
Comparison preference format **3** removes retained focus while
preserving endpoint, alias, whitespace, and Source-intent state. There is no
reader, writer, upgrader, backfill, or dual-format path for the incompatible
format-7 annotation or format-2 preference shapes.

Before resetting incompatible state, stop every affected viewer and MCP
process. Delete only
`workspaces/<repository-hash>/threads.jsonl` for annotation state and/or
`workspaces/<checkout-hash>/comparison/comparison.json` for that checkout's
comparison preference, then start only the new build. Do not delete
`review-points/`, configuration, logs, or unrelated workspace state.
Fathomable never performs this reset automatically.

## Consequences

- The membership rule is local, typed, and explainable from persisted origin
  plus accepted presentation.
- WorkingTree and Index never become Commit origins because they are clean,
  landed, or observed at that commit.
- Deletion context remains available only for its recorded ordered comparison.
- Landing remains durable audit evidence without presentation authority.
- Operators accept one explicit scoped reset at this pre-release format
  boundary.
