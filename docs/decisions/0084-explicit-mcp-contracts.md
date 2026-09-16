---
type: Decision
title: Explicit MCP contracts
description: Review tools advertise precise inputs and outputs, distinguish placement from content edits, and protect explicitly keyed writes against retries.
tags:
  - annotations
  - decision
  - sessions
---

# 0084 Explicit MCP contracts

Status: accepted (2026-09-16)

Reply contract amended 2026-09-16 by
[0085](0085-thread-lifecycle-and-auto-resolve.md). The reply-specific
`propose_resolve` input, returned `proposed_resolved` flag, and plain thread
array are historical. `thread_reply` now accepts `resolve`; each successful
request-order `results` item contains the complete current `thread`, its
original `resolution.outcome` (`not_requested`, `resolution_proposed`, or
`resolved`), and `replayed`. A proposal is successful and adds
`reason: pending_fathomable_user_review` plus guidance not to ask in chat or
retry. Execution-time failure returns `PARTIAL_BATCH` with indexed
`completed`, one `failed` item and error, and indexed `unattempted` items;
whole-batch prevalidation still returns `INVALID_BATCH`. Reads and returned
threads use one uniform `messages` array with `author`, `body`, `created`,
`modified`, and `resolution_proposed`. Idempotency, location, author
objects, output-schema, and text/structured parity below remain.

Amends [0082](0082-three-tool-review-core.md) without changing the
three-tool surface, automatic authorship, or repository binding. Its
human-only resolution boundary was later replaced by [0085](0085-thread-lifecycle-and-auto-resolve.md).
The MCP author projection is distinct from the stored and internal socket representation described in
[0083](0083-single-user-alpha-clean-slate.md).

## Context

An agent should discover valid calls and complete results from the tool
schemas, rather than infer a contract from examples or repair avoidable
argument errors. A response lost after a successful write should not require
guessing whether a similar discussion is the same operation.

## Decision

### Inputs describe the accepted calls

`threads.status` is an enum: `open`, `resolved`, or `all`. Omission or null
means `open`. A non-empty `ids` list is an exact lookup and excludes
non-null filters and pagination arguments. An empty `ids` list behaves as
omission. The input schema expresses these alternatives, and the server
still validates them.

`threads.limit` is an upper bound. Zero returns an empty `threads` array,
with `more` reporting every matching discussion and no continuation cursor.

Both write tools advertise non-empty item arrays and 1-based line numbers.
`end_line` requires `line`, defaults to it when omitted or null, and cannot
precede it. Reversed ranges fail at the MCP boundary; the viewer's
direction-independent selection range is unchanged. Comparisons between
the two numeric fields and checks against current file contents remain
runtime validation.

The reply argument is `propose_resolve`, not `resolve`. There is no alias.
It records a proposal on the reply and never closes the thread. The
returned reply field remains `proposed_resolved`.

### Complete, typed results

Each tool publishes an output schema. Success results carry the same
complete JSON value in `structuredContent` and a serialized JSON text
block, following the MCP text-fallback convention. There is no separate
abbreviated prose rendering that could hide replies from a text-only host.
Failures retain the MCP error indication and actionable explanations.
Batch prevalidation failures also return `error_code: "INVALID_BATCH"` and
an `issues` array with each failing item's zero-based `item_index`; the text
fallback names the same item as `comments[index]` or `replies[index]`.

Every MCP author, on the opening comment and on every reply, is an object
with `kind: "user"` or `kind: "agent"` and a `name`. Agent `client` and
`id` fields are present when recorded. The human's MCP name is the stable
label `user`, not an invented chat identity. This presentation does not
change stored authorship or the viewer's configured human display name.

### Location and content are separate

Discussion results retain `placement: anchored | edited | detached | file`
and add:

- `anchor_range`: the last stored anchor range, absent for a file-wide
  discussion.
- `location: unchanged | moved | detached | file`: whether the currently
  projected range is at that reference, elsewhere, unavailable, or
  file-wide.

`range` is the projected range when placement is usable. For a detached
thread it remains the last-known range, not a valid current location.
`anchor_range` is not immutable creation history: an explicit or persisted
re-anchor updates the reference. The existing `edited` state describes
changed content; a move alone does not determine whether a finding is
still relevant. Reads calculate this projection without persisting it.
When a reply supplies the unchanged stored range and the discussion still
anchors there, the reply does not persist a redundant re-anchor or change
`placement` to `edited`. A range that differs from the stored reference still
updates that reference, even when the old anchor projects onto those lines.

### Retry identity is explicit

Each start or reply item may carry an `idempotency_key`. Without a key,
repeated calls remain independent writes. There is no fuzzy comparison of
comment text and no automatic suppression of similar findings.
Keys must contain non-whitespace text and occupy at most 256 UTF-8 bytes.

A key belongs to the repository's shared store, the automatically
identified caller, and the operation kind. Reusing it for the same
effective request returns the existing discussion without another comment,
reply, relocation, or update timestamp. Reusing it for a different request
fails explicitly. Generated times, current file contents, and `HEAD` are
not part of the request identity.

Replay returns the discussion's current state, not a frozen historical
response. It is recognized before checks that depend on mutable file or
thread state, so a completed operation does not become a new write after a
file edit or thread resolution. If the discussion was subsequently deleted,
replay fails rather than recreating it.

The receipt and its write are persisted together under a cooperating store
lock. This protects concurrent calls and retries after a server restart;
an in-memory cache or a separate receipt written after the mutation would
not provide the guarantee.

All new items in a batch are prevalidated before writing. Duplicate keys
within one operation's batch fail before any write. The batch is still not
an all-or-nothing I/O transaction: a later failure may leave earlier items
completed. Per-item keys let a caller retry those items without duplicating
their effects. Both the viewer and headless paths enforce this contract.

### Current-only persistence boundary

The annotation event format advances from **2 to 3**: start and reply events
can carry their receipt, and a keyed reply carries any relocation in that
same event. The internal socket advances from **5 to 6** for keyed write
requests carrying the authenticated caller scope. Successful replies keep
the existing internal response shape. The exact-version policy of
[0062](0062-one-version-no-compatibility.md) remains: no old-format reader,
migration, or automatic state reset is introduced.

Matching viewer and MCP builds must be restarted together. An existing
format-2 annotation store is refused, not rewritten or silently deleted.
Any operator decision to archive or reset old annotation state is separate
from installing this change; a socket mismatch only requires matching
processes, not deletion of annotation data.

## Consequences

- Schemas expose the supported argument modes and returned history, but
  runtime validation remains authoritative.
- Changing the input name and MCP author shape is a current-only alpha
  contract change; clients reconnect to discover the new schemas.
- Placement metadata explains where the server can locate a discussion,
  not whether an agent should act on it or whether the user accepts it.
- Idempotency is opt-in retry protection, not cross-agent finding
  deduplication, task ownership, or discussion resolution.
