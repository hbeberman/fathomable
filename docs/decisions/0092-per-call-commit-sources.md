---
type: Decision
title: Per-call commit sources
description: MCP starts and reads can select one exact local commit without changing checkout or viewer state.
resource: crates/fathomable/src/mcp/source.rs
tags:
  - annotations
  - decision
  - git
  - security
  - sessions
---

# 0092 Per-call commit sources

Status: accepted (2026-09-19)

Amends [0061](0061-agents-start-threads.md),
[0082](0082-three-tool-review-core.md),
[0084](0084-explicit-mcp-contracts.md),
[0087](0087-global-comparisons-and-board-history.md), and
[0089](0089-store-only-mcp.md). The three-tool surface, repository binding,
working-tree confinement, current placement projection, lifecycle authority,
and store-only transport remain.

## Context

Review callers need to start discussions against files from a known commit
and retrieve discussions created from that same immutable source. Reusing the
viewer comparison would add process-global state, make concurrent calls
interfere, and confuse immutable creation origin with where a thread projects
in the current checkout. Treating an observed working-tree `HEAD` as commit
origin would make the same mistake and could include dirty, index, or
review-point content.

The existing annotation format already separates immutable origin evidence
from mutable placement, and the workspace can read local Git objects without
an external command. The missing contract is a narrow request-local selector,
an unambiguous resolved identity, bounded historical text capture, and retry
and pagination identities that cannot drift with `HEAD`.

## Decision

### One optional selector on two tools

`threads` and `thread_start` accept one optional top-level `source`:

```json
{
  "source": {
    "kind": "commit",
    "revision": "HEAD"
  }
}
```

`revision` is exactly uppercase `HEAD` or a full 40-hex SHA-1 commit ID.
Hex input is case-insensitive and canonicalizes to lowercase. Abbreviations,
branches, tags, revspecs, ranges, paths, URLs, whitespace-padded values, and
other source kinds are rejected. The selector applies to the whole call.
Omission or `null` preserves the existing request and response behavior.

`thread_reply` does not accept `source`. There is no fourth tool, global
loaded commit, server mutation, viewer mutation, comparison selection,
checkout switch, index/review-point selector, or cross-worktree route.

Every successful selected response includes top-level `resolved_commit` as
the canonical full ID, including empty and zero-limit reads and all-replay
starts. The text and structured MCP result channels contain the same complete
value. `checkout` continues to identify the startup-bound checkout.

### Starts capture one resulting tree

A fresh selected start validates its paths and ranges against regular text
blobs in the selected commit's resulting tree. Root commits and merge commits
are ordinary tree snapshots; Fathomable does not infer a parent, merge base,
or diff. Each new thread records that exact commit as
`OriginVersion::Commit`, an unspecified comparison side, the immutable origin
path and range, bounded snippet/context, and full-content identity.

Current placement remains a separate projection in the bound checkout. Its
path or range can differ from the origin, and absent or changed current
content can produce detached or edited placement without changing historical
evidence.

One call caches each distinct selected path, including failures, and admits at
most 64 MiB of raw blob bytes across those paths. Loaded bytes count even when
binary or UTF-8 validation rejects them. Missing files fail rather than becoming empty
content. Directories, symlinks, submodules, unsupported Git modes, and entries
that do not name blobs fail explicitly. Binary blobs and invalid UTF-8 fail as
text sources. Whole-batch validation still precedes fresh writes; existing
partial-write reporting remains truthful.

Selected commit identity is part of a keyed start's durable intent. A matching
replay returns the existing thread without requiring the source object to
remain available; a changed source conflicts. The final locked store operation
also verifies that a replayed thread has the selected immutable origin.
Resolution, reopening, or archival does not invalidate a matching retry, even
when the mutable thread-level `commit` now names a different resolution
commit. A deleted target still fails rather than being recreated.
Legacy unselected intent serialization is unchanged, so annotation format
remains **5** with no migration, reset, or backfill.

### Reads filter immutable origin

A selected `threads` call returns only non-archived board entries admitted by
the ordinary status and time filters whose immutable creation
`OriginVersion::Commit` exactly equals `resolved_commit`. It excludes
working-tree and index origins with a matching observed `HEAD`, review-point
origins with the same baseline, and later resolution or placement commits.
`path` applies to immutable `origin.path`, not current projected placement.
Exact-ID lookup remains a separate mode and cannot combine with `source`.

Ordering and limit behavior are unchanged. A selected `next_after` carries
`updated`, `id`, and `resolved_commit`; continuation requires the same full-ID
source and matching cursor ID. `HEAD` is resolved once per call and cannot be
combined with `after`; callers continue with the preceding response's full
`resolved_commit`. Pages are pinned to one origin filter, not to one
snapshot-isolated view of the changing board.

The input schema rejects cross-mode cursors and `HEAD` continuation; equality
between the cursor and source IDs is checked at runtime. Legacy cursors keep
their exact `updated`/`id` JSON shape.

### Local immutable objects are a separate read boundary

Before selected access, the server reopens the workspace and confirms that
its checkout root and repository key still match the startup binding. `HEAD`
is then pinned to one commit; a full ID must name that exact commit object.
Neither form follows Git replacement objects. Intermediate path entries must
have directory mode and name actual tree objects before their data is loaded;
non-directory entries are never traversed to look up a descendant.
The resulting tree and regular blobs are read through the in-process Git
object store. No fetch, remote access, Git subprocess, checkout, ref update,
or working-tree fallback occurs.

This boundary differs from checkout-confined working-tree reads: immutable
object lookup does not follow filesystem paths or symlinks from the commit,
while current placement still uses the capability-confined checkout reader.
Historical and deleted source can therefore be disclosed when its object is
locally available. Repository contents and same-UID mutation remain trusted
inputs; object loss after pruning fails explicitly rather than fetching or
substituting current content.

## Consequences

- `mcp/source.rs` owns selector validation, repository-binding checks,
  request-local commit capture, the 64 MiB budget, and selected cursors.
- Existing workspace APIs expose validated full commit IDs, exact commit
  lookup, and bounded regular-blob reads without leaking `gix` types.
- Existing annotation APIs bind selected provenance and keyed intent while
  preserving format 5.
- Origin filtering and current placement remain visibly separate, so a caller
  can review historical findings without mistaking checkout projection for
  the selected commit's coordinates.
