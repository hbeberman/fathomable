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

Thread discovery amended 2026-09-21 by
[0094](0094-diff-range-thread-discovery.md): normal thread viewers follow the
accepted comparison's commit range, with a shared **All threads** override.
Inline placement remains subject to exact endpoint and side eligibility.

Normal viewer membership and transition behavior amended 2026-09-20 by
[0093](0093-version-scoped-viewer-membership.md): normal surfaces now follow
the last accepted version presentation, while explicit history remains
repository-wide. Symbolic HEAD intent, exact mutable-origin landing,
review-point identity, persistent HEAD/Index choices, and the clean format-6
boundary are defined there.

MCP origin selection amended 2026-09-19 by
[0092](0092-per-call-commit-sources.md): an agent start may explicitly capture
from one immutable local commit tree without inheriting or changing a viewer
comparison. Selected reads match only exact `OriginVersion::Commit` creation
origins and filter `origin.path`; observed-`HEAD` working-tree/index origins
remain distinct. Returned current placement is still projected and qualified
by the bound checkout.

Terminology amended 2026-09-19: **Normal diff** and `diff.mode = "normal"`
replace Standard and `"standard"` without a compatibility alias. `Space d n`
selects Normal, while `Space d s` opens **Pick source...**; `Space d b` is
retired. User-facing comparison endpoints are Source and Target, while
persisted fields and annotation-format enum names remain unchanged. Picker
titles preserve the workflow and nesting path: **diff source**, **diff
target**, and **diff commit**, with qualifiers such as **(tags)**,
**(branches)**, and **(branches / name / commits)**. Their metadata names the
current candidate type and shows matched-to-total counts only while filtering.
Commit subjects remain prominent while short IDs and UTC dates are dim.

Resource bounds amended 2026-09-19: fresh non-Git workspaces start in Off
without enumerating or comparing the working tree. Saved endpoint choices
remain available; explicitly selecting Source or an active mode starts a
comparison. Comparison and immutable Target enumeration run on cancellable
workers, with generation checks on delivery. One running and one replaceable
pending request prevent repeated refreshes from spawning unbounded workers.
The last successful comparison may remain visibly stale after an error;
pending, limited, and failed work never means a complete clean comparison.
Path and aggregate content-read budgets are finite, including saved
review-point content. Mutable reads use bounded readers, not only a size
check before an unbounded read. Changed-path line counts are computed on the
worker rather than rereading every changed file on the event-loop thread.
Immutable Git endpoints with equal object identity and mode prove a path
unchanged without reading or charging its blob. The aggregate content ceiling
is deliberately generous and remains a last-resort bound on pathological
changed-content work rather than a repository-size target.
While a requested active mode is waiting for its comparison, filesystem
events replace the scan without cancelling that presentation intent. An
explicit Off selection cancels it.
Annotation creation and submission require matching displayed provenance,
not merely a newly selected endpoint. Active and parked new-annotation
drafts cancel pending projection work and defer further comparison refreshes
until the final draft closes. A failed or pending endpoint selection cannot
associate retained text with a different immutable commit.

Navigation amended 2026-09-18 by
[0090](0090-direct-workspace-navigation.md): Shift-Up/Down and `K`/`J`
traverse the selected comparison in Normal or Unified mode, including
hunkless changed paths; Shift-Left/Right and `H`/`L` traverse changed files
at their first diff. Off still gates traversal and its footer hint.

Transport amended 2026-09-18 by [0089](0089-store-only-mcp.md):
annotation format **5** remains, but socket protocol **8** and the transport
are removed. MCP line relocation consistently uses the stored path in its
bound checkout; ephemeral viewer-local rename projection remains a display
concern, not an alternate write interpretation.

Presentation amended 2026-09-18: one session-global, configuration-defaulted
mode presents the selected endpoints as Normal, Unified, or Off. Off is
Target-only source browsing, not history redaction. This amendment removes
Comparison controls, Start comparison at current HEAD, typed commit batches,
`Space d d`, and `:diff` without compatibility aliases.

Presentation amended later 2026-09-18: `Space d d` returns as a direct
current-`HEAD`-to-working-tree selector, not the removed presentation toggle.
The Diff menu exposes it as **Head to WorkingTree** below Source and Target;
**Save review point** begins a separate section.

Presentation amended later 2026-09-18: queued live-change badges, hints,
status counts, and navigation are removed rather than retained behind Off.
Transient counted file-edit toasts remain; Off hides only those toasts while
their timers continue, and plain notifications remain visible.

Presentation amended 2026-09-19: `Space d l` selects the resolved current
`HEAD` and its first parent; `Space d c` picks one commit and selects that
commit with its first parent. Root commits are rejected without changing the
pair. `Space d p` captures a review point and immediately selects that exact
point as Source with Working tree as Target. `Space t` replaces `Space c` for
repository-board workflows.

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
3. explicit saved workspace states used as temporal sources.

## Decision

### One comparison per checkout

A running viewer owns one comparison for its active checkout. The selected
Source, Target, whitespace rule, and session-global presentation mode apply to
every file. Opening another file does not choose another pair or mode.
`diff { mode "normal" }` chooses the startup mode; `normal`, `unified`, and
`off` are the only accepted values. Runtime mode changes do not persist.
Source, Target, and whitespace keep their existing persistence and session
rules.

The primary endpoints are:

- an empty tree;
- an immutable commit, stored as its resolved object ID;
- the current index;
- the live working tree;
- an explicit review point, resolved through its owning store.

A fresh Git checkout pins the current `HEAD` commit once as its source and uses
the working tree as target. An unborn repository uses the empty tree.
Persisted checkout-local selection takes precedence. A later commit never
advances an already selected commit endpoint. To advance Source, select the new
commit explicitly.

`A -> B` means the direct net delta between those endpoint trees. It never
silently substitutes a merge base or three-dot comparison.

The endpoint picker is hierarchical. Its first rows distinguish the working
tree (files on disk), index (the staged next-commit snapshot), and `HEAD`
(the checked-out commit), followed by **Tags...**, **Branches...**, source-only
**Review points...**, and **Advanced...** for the empty tree. Up to 500
commits reachable from `HEAD` follow those choices, newest first.

Commit rows render as dim short ID, prominent subject, and dim right-aligned
UTC `YYYY-MM-DD`; the subject is ellipsized before the date is displaced.
Endpoint-bearing rows show muted-blue **[current source]** and **[current
target]** hints using the shared popup-key accent; commit-row hints sit
immediately before the date.
Equivalent working-tree, index, `HEAD`, tag, and commit rows therefore expose
the active pair without changing what selection means.

**Tags...** is a searchable list whose selection pins the tagged commit.
**Branches...** searches local and remote-tracking branches, then opens up to
500 commits reachable from the selected branch. No picker fetches or checks
out.

Typing four or more hexadecimal characters searches older commit IDs without
eagerly loading every old subject: the top-level picker walks commits reachable
from local branches, remote-tracking branches, and tags, while a selected
branch's commit picker remains within that branch. Longer prefixes refine the
first result set in memory. Other typed local Git revisions remain available;
typed `first..last` batches do not. Escape returns from a nested picker to its
parent before closing the endpoint picker.

Root cards are titled **diff source**, **diff target**, or **diff commit**.
Nested titles append **(tags)**, **(branches)**, **(review points)**, or
**(advanced)** as applicable; branch histories append **(branches / name /
commits)**. Unfiltered metadata names the candidate type, such as **24
branches**. A non-empty filter reports **8 of 397 choices**, or **no matches**;
the optional review-point name card has no count. Branch names ellipsize before
they can hide the query, and narrow cards drop count metadata before query
text.

Picker motion lets the cursor move freely between three-row top and bottom
margins. Crossing a margin scrolls the list while keeping the cursor at that
margin. Once the list reaches its beginning or end, the cursor can move closer
to that edge. Pointing at a row gives it the shared hover treatment, a left
click chooses it, and the wheel moves through the visible choices.

### One presentation mode per session

The **Diff** menu begins with three mutually exclusive `▌` choices:

- **Normal diff** (`Space d n`) shows Target content with comparison
  gutters, counts, Files filtering, and hunk navigation.
- **Unified diff** (`Space d u`) shows the selected Source-to-Target patch. It
  follows file switches and remains selected when `Esc` closes transient
  input or returns to File.
- **Diff off** (`Space d o`) loads and enumerates Target without evaluating
  Source. The File surface never falls back to Source bytes, and Files/pickers
  omit Source-only paths. An already open Source-only path retains its label,
  displays `not present in Target`, and has no Source body.

**Pick source...** (`Space d s`), **Pick target...** (`Space d t`), **HEAD to
Working tree**, **HEAD~1 to HEAD**, and **Commit~1 to Commit...** follow those
mode rows. The direct HEAD action pins the current
`HEAD` as Source and selects the working tree as Target. The parent actions
first resolve one immutable commit identity, derive its first parent, and
select the parent-to-commit pair atomically; a root commit leaves both
endpoints unchanged. Save review point and Ignore whitespace each begin a
separate section.
Off retains Source, whitespace, the only-changed Files filter, and the last
Normal/Unified mode. Changed-only and Ignore whitespace are dormant while
Off: their marks are hidden, rows are disabled, and direct keys report `diff
mode is off`. Selecting Target alone keeps Off active. Selecting Source attempts
to restore the last active mode; if the pair cannot be read, both endpoints
remain selected, the failure is reported, and mode remains Off. Entering Off
clears retained Unified and deletion-backed source content before any fallible
Target read.

Off also gates comparison gutters, counts and hunk navigation; current Git
`XY` and `[G`/`]G`; and transient file-edit toasts, including their counts.
Those toasts remain timed while hidden, and plain notifications remain
visible. There is no live-change queue, badge, hint, status row, jump action,
or internal acknowledgement state. `:status` and file/binary information
report mode Off and Target facts only.

Rendered/Source remains available in Normal and Off only for files accepted
by the configured [Markdown classifier](0016-syntax-highlighting.md#markdown-versus-source-files).
Other files stay in source view. Unified retains each file's source/rendered
choice but disables it through `Space v s` and menus. A pending
new-line or new-file annotation draft, including a parked draft on a removed
Unified row, blocks mode, Source, and Target changes until submit or cancel.
Replies and message edits do not.

The comparison owns:

- changed-path enumeration, including historical-only paths;
- the complete target path set used by the files pane and file picker;
- added, deleted, content, mode, type, binary, unsupported, and unavailable
  facts;
- file and hunk navigation;
- line and workspace counts;
- gutter changes;
- unified diff content;
- historical source loading and labels.

The files pane and visible file picker follow the selected Target. A working
tree Target lists the live checkout. A commit, index, or empty-tree Target
lists only that snapshot. Normal and Unified additionally include Source-only
paths deleted by the comparison so their removal remains navigable. Off never
includes those paths. Files added to the checkout after an immutable Target do
not leak into that historical view. Opening a listed path displays Target
content; Normal and Unified may display Source content for a comparison
deletion, while Off never does.

Current Git index/worktree status remains a separately labelled fact. It
does not replace the selected comparison's changed set.

Working-tree endpoints mean final on-disk content. Staged and unstaged
changes that cancel therefore produce no net change against the selected
source, while the index remains an explicit endpoint.

The endpoint and whitespace selection is persisted outside the checkout under
a checkout-derived comparison directory. Worktree navigation restores that
checkout's choice or creates its pinned default; the session-global mode
continues across the switch. Each viewer owns its active selection and mode;
neither is a shared board control. The initial pinned default is persisted
after its first successful comparison, before a later commit or restart can
redefine it. Two viewers on the same checkout do not live-control one another;
their last-used endpoint preference has explicit last-successful-writer
behavior.

The menu bar right-aligns compact `source to target` controls in Normal and
Unified. Off hides Source and shows only the bare clickable Target. Commit
endpoints use short IDs. Working tree, index, empty tree, and `HEAD` use those
names; an explicitly selected tag uses `Tag name`. `HEAD` and tag names are
presentation aliases beside the pinned commit ID, not mutable endpoints. They
persist only while the name still resolves to that same ID, otherwise the menu
falls back to the short commit ID. Every displayed label uses the shared
popup-key accent and menu hover background and opens its endpoint picker.

The File header and every Reviews/history header end with a dim
`Diff: normal`, `Diff: unified`, or `Diff: off` control. It outranks passive
counts at narrow widths and opens an anchored, non-searchable, three-row
choice popup. Normal `CMP ...` and unified-diff provenance is absent from the
bottom line whenever endpoint controls actually render. Stale/error status
remains. If the menu is hidden or too narrow to render the controls, the
bottom line provides the mode-aware endpoint fallback.

Mutable endpoints refresh after relevant Git and filesystem events.
Immutable commit pairs retain their content. A failed refresh keeps the last
successful result, labels it stale, and reports the error rather than
relabelling old content as current. Changing branches under a pinned
commit-to-working-tree pair keeps the commit and names the moving checkout.

### Explicit workspace review points

`Space d p` saves a review point for the workspace. Capture is deliberate;
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
the cooperating store lock. Read errors, races, unsupported content, missing
required objects, and failures before the manifest append prevent publication.
An append, flush, or sync failure reports an uncertain outcome and tells the
reader to reload before retrying rather than claiming the point was not saved.
Known ignore-policy exclusions are recorded on an otherwise selectable point.
Capture does not write the checkout, index, refs, or Git object database.

Review-point directories and all manifest/blob files follow the
[private-state contract](0009-cli-and-diagnostics.md#persistent-state-privacy),
including reused blobs and temporary replacements. Comparison preferences
use the same 0700/0600 hierarchy, validate existing files before replacement,
and refuse pre-existing temporary paths. Unsafe state is reported without
repairing permissions, migrating, or discarding it.

Workspace traversal is fallible for capture: an unreadable directory aborts
the point rather than turning every unseen child into a deletion tombstone.

Git objects are not mirrored or retained. If history rewriting or garbage
collection removes a required commit, the point reports unavailable content
and never substitutes the current `HEAD` or working file.

A review point selected as the source compares directly to the working tree,
including a reversal that disappears from a commit-to-working-tree net
diff. Review points are not valid targets. A successful `Space d p` capture
selects the exact returned point as Source and Working tree as Target without
re-querying by name or time. Capture, comparison, and preference-persistence
outcomes are reported separately: a point remains selected for the running
viewer if only preference persistence fails, while comparison failure leaves
the saved point available without claiming selection.

Lifecycle amended 2026-09-19: `Space d r` and **Manage review points...** open
a searchable repository-wide point manager. Enter or a row click opens a
point card with bounded identity, capture-time, baseline, and file-count facts.
The card's `r` action edits the name and `d` opens a separate destructive
confirmation; `y` confirms deletion and Esc returns without crossing either
boundary. The comparison Review points picker also exposes `Ctrl-r` and a
clickable hint for direct rename; the manager keeps rename on the selected
point's card. A pending new line or file annotation blocks management so
displayed source and provenance cannot be silently rebound.

Names are trimmed on new writes; blank or whitespace-only input means unnamed.
A nonblank name is one line, contains no control character, U+2028, or U+2029,
and is at most 128 Unicode scalar values. Exact, case-sensitive names are
unique across active points in the repository-wide store. Legacy capture
records remain readable even when their names predate these rules, and the
viewer bounds and sanitizes their display.

Capture records remain immutable. Rename appends a durable
`review-point-rename` metadata record under the manifest lock and increments a
per-point name revision. The viewer supplies the snapshot it displayed as a
compare-and-swap token; a concurrent rename is rejected as stale rather than
overwritten. After success or a recoverable conflict the originating picker
reloads and reselects the stable point ID when it still exists. Names are
labels only: selection, comparison preferences, deletion, and thread origin
evidence continue to use the immutable point ID.

Deletion remains separately guarded and appends a durable
`review-point-delete` record under the same exclusive manifest lock as capture
and rename. Replay repairs only an interrupted final JSONL suffix and otherwise
fails closed on malformed complete records. The lock remains held while blobs
referenced only by the deleted point are validated and removed; shared blobs
remain. Deletion is committed before cleanup, so cleanup failures are reported
without resurrecting the point. Logical deletion and best-effort reclamation
are not secure erasure:
historical manifests, Git objects, copied excerpts, caches, backups, shared
blobs, and failed-cleanup blobs may remain.

Viewers reload point metadata at point-dependent actions. A selected deleted
Source falls back to pinned `HEAD`, or EmptyTree without one, while preserving
Target, mode, whitespace, and dormant Off state; preference persistence is
best-effort and startup reconciliation remains authoritative. Threads never
pin point blobs or change on deletion: their copied point ID, baseline,
content identity, excerpt, messages, lifecycle, and archive state remain
available to the viewer and MCP.

### Immutable origin and qualified placement

Every thread stores immutable origin evidence:

- original path and optional range;
- exact bounded snippet and surrounding context;
- the version and side that supplied those lines;
- the human's selected comparison when present;
- working-tree, index, or review-point facts when applicable;
- a content identity, which identifies bytes but is not called a snapshot.

A historical target comment originates at the target. A removed line
originates at the source. Selections spanning both sides are refused. Unified
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
Truncated stored context is display evidence only, never a fallback placement
authority; exact full anchors remain usable
([context mapping](0038-reanchoring-without-a-snapshot.md#mapping-through-the-window)).

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
their complete messages and original evidence. This labelled immutable origin
evidence remains visible while diff mode is Off: Target-only applies to source
browsing, not the repository's retained review record.

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

- `Space t a` archives all threads that are still resolved when the store
  lock is held.
- `Space t A` opens **Clear board...**. The confirmation names
  active/proposed and resolved counts and states that the board is shared
  across the repository and all worktrees. It archives exactly the
  acknowledged slate. A new thread is excluded; a changed acknowledged
  entry causes updated counts to be shown for confirmation again.
- `Space t R` opens **Recently resolved**, newest actual resolution first.
  Generic metadata edits do not reorder it. Reopen removes an entry;
  resolving again returns it at its new resolution time.
- `Space t h` opens **Archived threads**. `u` restores the selected record
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
