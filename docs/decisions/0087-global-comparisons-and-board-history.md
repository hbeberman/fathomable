---
type: Decision
title: Global comparisons and deliberate board history
description: One checkout-wide pinned comparison drives every diff surface; explicit review points replace reader snapshots, while immutable thread origins, repository-wide visibility, recent resolutions, and deliberate archives preserve discussion history.
resource: crates/fathomable/src/app/comparison.rs
related_resources:
  - crates/fathomable-core/src/review_points.rs
  - crates/fathomable/src/app/review_points.rs
  - crates/fathomable/src/app/threads/archive.rs
tags:
  - annotations
  - architecture
  - decision
  - git
  - input
  - review-points
  - sessions
---

# 0087 Global comparisons and deliberate board history

Status: accepted (2026-09-16)

Supersedes the last-seen and per-file checkpoint comparison model of
[0015](0015-follow-mode.md), [0020](0020-reanchoring-across-restarts.md),
[0049](0049-inline-threads-and-the-rail.md),
[0060](0060-one-diff-two-sides.md), and
[0069](0069-the-diffs-keys-on-the-bar.md). It supersedes automatic
follow-HEAD rescoping in [0035](0035-threads-follow-head.md), and replaces
the ancestry-based board visibility of
[0072](0072-a-resolved-thread-stays-at-its-commit.md).

The lifecycle, one-shot permission, uniform message, and summary contracts
of [0085](0085-thread-lifecycle-and-auto-resolve.md) and
[0086](0086-one-thread-summary-and-its-actions.md) remain.

## Context

The previous diff model stored a different pair on each open file. The file
tree and cross-file navigation still meant current Git status, even when the
text displayed a checkpoint, old commit, or last-seen snapshot. Selecting a
commit used the current file's history, and opening another file silently
returned to that file's local pair.

Persistent reader snapshots also had two unrelated jobs. They defined "what
changed since I looked" and supplied full-file content for offline thread
relocation. This made ordinary reading write state, tied anchoring quality to
an expiring store, and encouraged full-file capture when a thread itself only
needs bounded evidence.

Thread visibility had a similar split. The shared store held repository
discussion history, while `Reach` hid records based on the active worktrees'
ancestry and treated resolved-at-HEAD as a membership rule. A discussion
could therefore disappear from routine MCP and viewer reads merely because a
branch moved, even though no user archived or deleted it.

The desired model has three independent concepts:

1. the versions the viewer compares;
2. the repository's discussion board;
3. explicit saved workspace states used as temporal bases.

## Decision

### One comparison per checkout

A running viewer owns one comparison for its active checkout. The selected
base, target, whitespace rule, and optional temporal focus apply to every
file. Opening another file does not choose another pair.

The primary endpoints are:

- an empty tree;
- an immutable commit, stored as its resolved object ID;
- the current index;
- the live working tree;
- an explicit review point, resolved through its owning store.

A fresh Git checkout pins the current `HEAD` commit once as its base and uses
the working tree as target. An unborn repository uses the empty tree.
Persisted checkout-local selection takes precedence. A later commit never
advances an already selected commit endpoint. **Start comparison at current
HEAD** is the explicit action that advances the base.

`A -> B` means the direct net delta between those endpoint trees. It never
silently substitutes a merge base or three-dot comparison. A contiguous
commit batch means the parent before its first commit through its last
commit. A root begins at the empty tree; a merge boundary must be selected
explicitly.

The endpoint picker is hierarchical. Its first rows distinguish the working
tree (files on disk), index (the staged next-commit snapshot), and `HEAD`
(the checked-out commit), followed by **Tags...**, **Branches...**, base-only
**Review points...**, and **Advanced...** for the empty tree. Up to 500
commits reachable from `HEAD` follow those choices, newest first.

Commit rows render as short ID, subject, and right-aligned UTC `YYYY-MM-DD`;
the subject is ellipsized before the date is displaced. Endpoint-bearing rows
show colored **[current base]** and **[current target]** hints at the right;
commit-row hints sit immediately before the date. Equivalent working-tree,
index, `HEAD`, tag, and commit rows therefore expose the active pair without
changing what selection means.

**Tags...** is a searchable list whose selection pins the tagged commit.
**Branches...** searches local and remote-tracking branches, then opens up to
500 commits reachable from the selected branch. No picker fetches or checks
out.

Typing four or more hexadecimal characters searches older commit IDs without
eagerly loading every old subject: the top-level picker walks commits reachable
from local branches, remote-tracking branches, and tags, while a selected
branch's commit picker remains within that branch. Longer prefixes refine the
first result set in memory. Other typed local Git revisions and contiguous
`first..last` batches remain available. Escape returns from a nested picker to
its parent before closing the endpoint picker.

Picker motion lets the cursor move freely between three-row top and bottom
margins. Crossing a margin scrolls the list while keeping the cursor at that
margin. Once the list reaches its beginning or end, the cursor can move closer
to that edge.

The comparison owns:

- changed-path enumeration, including historical-only paths;
- added, deleted, content, mode, type, binary, unsupported, and unavailable
  facts;
- file and hunk navigation;
- line and workspace counts;
- gutter changes;
- unified diff content;
- historical source loading and labels.

Current Git index/worktree status remains a separately labelled fact. It
does not replace the selected comparison's changed set.

Working-tree endpoints mean final on-disk content. Staged and unstaged
changes that cancel therefore produce no net change against the selected
base, while the index remains an explicit endpoint.

The selection is persisted outside the checkout under a checkout-derived
comparison directory. Worktree navigation restores that checkout's choice
or creates its pinned default. Each viewer owns its active selection; it is
not a shared board control. The initial pinned default is persisted after its
first successful comparison, before a later commit or restart can redefine it.
Two viewers on the same checkout do not live-control one another; their
last-used preference has explicit last-successful-writer behavior.

The menu bar places a compact `base to target` label immediately before its
right-justified checkout/document status. Commit endpoints use short IDs.
Working tree, index, empty tree, and `HEAD` use those names; an explicitly
selected tag uses `Tag name`. `HEAD` and tag names are presentation aliases
beside the pinned commit ID, not mutable endpoints. They persist only while
the name still resolves to that same ID, otherwise the menu falls back to the
short commit ID.

Mutable endpoints refresh after relevant Git and filesystem events.
Immutable commit pairs retain their content. A failed refresh keeps the last
successful result, labels it stale, and reports the error rather than
relabelling old content as current. Changing branches under a pinned
commit-to-working-tree pair keeps the commit and names the moving checkout.

### Explicit workspace review points

`Space d c` saves a review point for the workspace. Capture is deliberate;
comments, file switches, idle time, startup, commits, and quit never save
one.

A Git review point records:

- a stable ID, optional name, time, checkout identity, and observed `HEAD`;
- content-addressed blobs for eligible added or modified working content;
- tombstones for deleted baseline paths;
- mode, type, size, and policy exclusions needed to interpret the manifest.

Unchanged committed content comes from the recorded commit. Equal bytes are
stored once across points. Outside Git the manifest stores every eligible
file because there is no committed baseline.

Capture reads each working file once, verifies observable file and `HEAD`
stability, writes required blobs durably, then publishes the manifest under
the cooperating store lock. Read errors, races, unsupported content, and
missing required objects prevent publication. Known ignore-policy exclusions
are recorded on an otherwise selectable point. Capture does not write the
checkout, index, refs, or Git object database.

Workspace traversal is fallible for capture: an unreadable directory aborts
the point rather than turning every unseen child into a deletion tombstone.

Git objects are not mirrored or retained. If history rewriting or garbage
collection removes a required commit, the point reports unavailable content
and never substitutes the current `HEAD` or working file.

`Space d r` selects **All changes** or **Since review point**. Since focus is
the actual point-to-working-tree delta, not a filter over the overall pair,
so it includes a reversal that disappears from the overall net diff. It is
available only with a working-tree target and changes paths, counts, gutters,
and navigation together. Saving another point does not select it.

### Immutable origin and qualified placement

Every thread stores immutable origin evidence:

- original path and optional range;
- exact bounded snippet and surrounding context;
- the version and side that supplied those lines;
- the human's overall comparison and temporal focus when present;
- working-tree, index, or review-point facts when applicable;
- a content identity, which identifies bytes but is not called a snapshot.

A historical target comment originates at the target. A removed line
originates at the base. Selections spanning both sides are refused. Unified
diff rows carry exact old/new line identities, so repeated removed text
cannot be matched to the wrong occurrence.

Agent starts capture the bound checkout's working content, observed `HEAD`,
and dirty/addition facts. They do not inherit a human viewer comparison and
MCP clients provide no comparison or review identifiers.

Current path, anchor, range, context, and version are placement evidence.
Relocation, rename, resolution, rebase, and projection never change origin.
Resolution records its own actor, time, checkout, version, and observed
`HEAD`; reopen preserves prior resolution history.

The viewer projects placement into the content it actually displays. MCP
projects into its bound checkout. These answers may differ without either
overwriting a global current line. Exact anchors are tried first, then the
latest bounded placement context and immutable origin context. Ambiguous or
missing evidence detaches the thread and retains its original excerpt.

Live working-tree reloads may persist a trustworthy local relocation.
Working-file events never relocate a thread while the viewer displays an
immutable target.

An exact working-tree rename is remembered by that viewer as a projection
local to the active checkout. Switching worktrees or restarting discards that
projection. It is not appended as one global board path that would overwrite
another worktree's answer. Without enough bounded evidence, the renamed path
may therefore detach rather than claim a shared current name.

Automatic last-seen snapshots, idle marks, mark-all-seen, snapshot pinning,
startup snapshot relocation, and automatic follow-HEAD rescoping are
removed. Existing old state directories are not deleted or migrated.

### One repository board

Every linked worktree shares one board. All non-archived threads remain
board members regardless of selected comparison, branch ancestry, or visible
hunks. Ancestry and worktrees can qualify where code is currently
projectable; they do not hide history.

Default board views still hide resolved threads until requested. File marks
claim only trustworthy placement in the displayed checkout/version.
Historical, off-branch, or detached entries remain in the full review with
their complete messages and original evidence.

MCP remains exactly three repository-bound tools:

- `threads`;
- `thread_start`;
- `thread_reply`.

Filtered and paged reads operate on the non-archived board. `resolved` and
`all` are not gated by current `HEAD`. Exact-ID reads can inspect archived
history and expose its archive state. A fresh write to an archived thread
fails; a matching durable idempotency retry is recognized first and returns
its original outcome without restoring or duplicating the thread.

The annotation format is **5** and the matching internal socket protocol is
**8**. Each build accepts only its exact version. A socket mismatch requires
restarting viewer and MCP processes. A store mismatch requires an explicit
operator decision about the named old state; startup never deletes it.

### Deliberate board cleanup

Archive is orthogonal to active, resolution-proposed, and resolved
lifecycle. It is not resolution, deletion, or a fourth lifecycle glyph.

- `Space c a` archives all threads that are still resolved when the store
  lock is held.
- `Space c A` opens **Clear board...**. The confirmation names
  active/proposed and resolved counts and states that the board is shared
  across the repository and all worktrees. It archives exactly the
  acknowledged slate. A new thread is excluded; a changed acknowledged
  entry causes updated counts to be shown for confirmation again.
- `Space c R` opens **Recently resolved**, newest actual resolution first.
  Generic metadata edits do not reorder it. Reopen removes an entry;
  resolving again returns it at its new resolution time.
- `Space c h` opens **Archived threads**. `u` restores the selected record
  with its lifecycle and history intact. A restored resolved thread remains
  resolved; no one-shot permission returns.

`a` archives a selected resolved entry in the review list. There is no
automatic archive timer, retention setting, startup sweep, commit hook, or
save-point sweep. Clear board does not alter comparisons, review points,
code, Git, or thread history.

## Consequences

- `app/comparison.rs` owns the checkout-wide selection and persistence.
- `fathomable_core::review_points` owns durable review-point manifests and
  blobs.
- `app/threads/archive.rs` owns deliberate human archive operations and
  clear-board confirmation.
- Existing line diff and `ThreadSummary` layouts are reused. There is no
  second renderer, thread store, review-session hierarchy, assignment
  system, or MCP workflow.
- Old last-seen files may remain in XDG state but are inert. The application
  never removes them automatically.
