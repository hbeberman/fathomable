---
type: Decision
title: Diff-range thread discovery
description: Thread viewers follow the accepted comparison's commit range, with one shared All threads backstop and independent inline placement.
resource: crates/fathomable/src/app/threads/visibility.rs
tags:
  - annotations
  - architecture
  - decision
  - git
  - input
---

# 0094 Diff-range thread discovery

Status: accepted (2026-09-21)

Amends viewer membership in
[0093](0093-version-scoped-viewer-membership.md), thread viewers in
[0025](0025-thread-list.md) and [0027](0027-revisiting-threads.md), and
the settings menus in [0081](0081-the-menu-bar.md).
The immutable origin, strict inline-placement rules, lifecycle authority,
landing evidence, and exact-origin MCP selectors remain unchanged.

## Context

A comparison spanning several commits should expose findings from those
commits without requiring the reader to select each commit separately.
Keeping the creation commit immutable is necessary for accurate evidence;
restricting discovery to that one displayed Target is not.

Source and Target already express the reader's review scope. A second range
selector would duplicate that choice. Repository-wide discovery needs only a
backstop for stored discussions outside the selected comparison.

## Decision

### Thread discovery follows the accepted comparison

Normal thread membership is the union of the existing exact endpoint and
ordered-comparison membership rules and immutable Commit origins in the
accepted comparison's commit range. File and resolved-status filters still
apply. Archived threads remain outside the normal viewers.

The graph range is commits reachable from Target but not from Source:
Source-exclusive, Target-inclusive, traversing all parents rather than only
the first parent. This is not a timestamp interval or a list of changed
files. A finding can remain relevant to a review even when its path has no
net change between the two endpoint snapshots.

For a synthetic linear history `A -> B -> C`, selecting Source A and Target C
includes findings originating on B and C. A finding on A does not enter by
range membership, but a discussion attached to the displayed Source side can
still enter through its existing exact-comparison rule. Selecting Source B
removes B's findings from the range; exact endpoint rules remain additive.
Non-ancestor endpoint pairs use the same graph-set difference.

WorkingTree and Index use the comparison's captured HEAD as their graph
boundary, while their own discussions retain their typed, checkout-qualified
membership. A review-point Source uses its recorded baseline for the graph
boundary; an empty tree excludes no ancestors. None of these graph boundaries
converts a mutable or review-point origin into a Commit origin.

The accepted presentation owns the range. Compute it on the comparison worker
and install it with the successful comparison, not during rendering or by
walking Git for each thread. Pending selections and failed refreshes preserve
the last accepted presentation and its membership. Graph work must be bounded
and cancellable; unavailable history or exhausted limits must fail explicitly,
not silently install a partial range. Git reads stay local without fetching,
changing refs, or running an external process.

The graph walk admits at most the smaller of `limits.comparison-paths` and
`limits.retained-paths` distinct commit IDs. Its separate aggregate metadata
read budget is the larger of `limits.comparison-bytes` and 64 MiB, with the
existing 64 MiB per-commit metadata ceiling. It charges each unique inspected
commit's uncompressed object size once, separately from comparison blob
reads. These are work budgets, not an exact process-memory ceiling; decoded
parent vectors, collection overhead, and the commit-graph mapping are separate.
Equal endpoints validate the commit and yield
an empty range without walking its ancestry. Available commit-graph
generations allow the walk to stop at a common frontier; absent generation
coverage uses the same bounded exact object traversal.

Diff Off has no Source range: with the backstop disabled, only existing
Target-side membership applies. A non-Git or unborn workspace retains its
applicable local endpoint membership.

### One shared All threads backstop

**All threads** is a session-only toggle, initially off, shared by the main
**Threads** viewer and sidebar **Thread list**:

- Off follows the accepted comparison as described above.
- On bypasses comparison membership and includes stored non-archived threads
  from the repository, including off-branch, unknown-origin, and
  unavailable-source discussions.
- **Only current file** and **Show resolved** still apply independently.
- The override does not change Source, Target, focus, thread provenance,
  lifecycle, or inline eligibility.

Bare `A` toggles it in either thread viewer. `Space T A` toggles it from any
normal pane without moving focus. Lowercase `a`, `Space t a`, and `Space t A`
retain their existing archive and clear-board actions.

Both pane-title settings menus and the top **Review** menu expose the same
checked **All threads** item. Normal thread-view headers show **all history**
while enabled. Counts and navigation use the same admitted candidates as
their owning list. Dedicated **Recently resolved** and **Archived threads**
keep their existing repository-wide behavior rather than inheriting this
normal-view filter.

### Discovery is not inline placement

Range membership and All threads admit discussions to the lists, not to
arbitrary lines in the displayed code. Inline rendering still requires the
exact endpoint and side eligibility from
[0093](0093-version-scoped-viewer-membership.md) and trustworthy placement.
Historical entries keep their original evidence readable and their thread
actions available even when current source is absent or cannot be placed.
Navigation must not silently omit a listed finding or substitute unrelated
current code for its original context.

No origin, placement, landing, or lifecycle event is written merely because a
thread becomes discoverable. A graph relationship does not establish that a
defect remains present or that a later commit fixed it.

### Unchanged boundaries and deferred work

MCP `threads {source: C}` remains an exact immutable-origin query, not a viewer
range query. Unselected MCP reads remain repository-wide. No annotation
schema change, automatic retention policy, or garbage collection is required.

Rebase equivalence, automatic carry-forward, explicit fix-commit links, and
multi-commit associations are separate future work. All threads can reveal
stored discussions from rewritten history without claiming that an old commit
and its replacement are equivalent. Missing Git objects do not remove stored
messages or excerpts, and the viewer does not recover full source by fetching.
