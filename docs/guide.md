---
type: Guide
title: Setup guide
description: Everyday UX, KDL configuration, agent setup, and where Fathomable saves state.
related_resources:
  - scripts/perf-record.sh
  - scripts/test-workspace-performance.py
tags:
  - onboarding
---

# Setup guide

Fathomable is a read-only workspace viewer for reviewing diffs and interactive
comment threads with agents via MCP. It never edits your checkout or Git data.
Install it on Linux using the [README](../README.md#install); contributor
tooling is separate in [CONTRIBUTING.md](../.github/CONTRIBUTING.md).

Source builds support Rust 1.95 or newer and default to stable Rust. The
minimum supported compiler and the compiler used for attributed release
builds are separate; see the
[toolchain policy](decisions/0001-dependency-policy.md#rust-toolchain-roles).

**Enthusiast alpha:** features may appear, change, or disappear at any time.
There is no cross-upgrade or cross-downgrade persistence guarantee for
Fathomable stores, including annotations and review points. Treat this state
as disposable between versions and keep important conclusions outside the app.
See the
[alpha contract](decisions/0083-single-user-alpha-clean-slate.md#enthusiast-alpha-contract).

## Start

```sh
fathomable                   # Current workspace
fathomable path/to/checkout  # Another workspace
fathomable README.md         # Open a file in its workspace
```

In Git, the workspace is the enclosing repository or linked worktree.
Outside Git, a directory is its own workspace.

A fresh non-Git workspace starts with **Diff Off**: browsing a directory does
not implicitly read every file against an empty tree. Explicit comparisons
remain available, including a saved review point against the working tree.
Saved endpoint choices are retained.

Recursive discovery runs in the background with finite budgets. File pickers
show a scanning or incomplete-coverage explanation while their results are
partial. Comparisons show scanning, limited, or stale/error status rather than
reporting an incomplete walk as clean. A limit does not prove that an omitted
path is absent. Loaded-file reads still obey `viewer.max-file-size-mib`.
New annotations require a ready projection matching the selected endpoints.
An active or parked new-annotation draft freezes comparison refreshes so a
background result cannot replace its evidence; deferred refreshes resume
after the last such draft is submitted or discarded.

Broad live watching prioritizes the root, loaded-file ancestors, and
materialized directories, then stops at its finite budget. The status badge
retains `watch scanning`, `watch limited`, or `watch error`; `:status` keeps
the exact coverage state and reason visible. Limited or errored broad coverage is
not retried by a periodic whole-tree walk; `R` requests a fresh generation.
Thread-state observation retains its separate narrow recovery path.
Git refs and worktree-registry discovery share the broad watch budget;
immediate state/Git control watches have a separate fixed ceiling of 16.

The `limits` config block below sets discovery entries, workspace watches,
retained paths, comparison paths, comparison content bytes, and pending
events. Positive invocation-only overrides are `--discovery-entries`,
`--workspace-watches`, `--retained-paths`, `--comparison-paths`,
`--comparison-bytes`, and `--pending-events`; zero is rejected, not unlimited.
Content bytes count reads, including line-count passes, rather than unique
file sizes. Git status uses the same scan/path/content budgets and remains
stale on exhaustion. These are operation budgets, not an exact process-RSS
ceiling: loaded documents, Git metadata decoding, and allocator overhead are
separate. Cancellation does not interrupt a blocked filesystem syscall, but
quitting does not join discovery workers.

## UX

**File list** browses the workspace; **Thread list** lists discussions beside
the document. The main column switches between **File** (`f`) and **Threads**
(`t`). `F` and `T` show and focus the corresponding sidebar list. Click pane
titles for their existing filters and view options, right-click rows for
actions, or use the top **Layout**, **Go**, **Review**, and **Diff** menus.
Plain header space and content can focus a pane without changing the title's
menu target. `Alt-Space` opens and focuses the top-left Fathomable menu,
revealing the app bar first if it is hidden. Hover never focuses.

The startup welcome and **Help > Getting started** keep the complete quick
reference visible by first tightening optional blank rows, then placing the
Pane, Diff, and Comment sections in readable columns on short, wide terminals.
Narrow layouts wrap the introduction and retain one control column when height
allows; genuinely undersized layouts use the normal terminal-too-small
treatment instead of clipping a section. `Alt-Space` has its own line below
the `Space` keymap hint. The welcome's brief `c` description applies to adding
a comment in File; elsewhere `c` remains contextual, including **only
changed** in File list and reply in the thread surfaces.

The **File list** title menu has four session filters: **only changed**,
**only reviews**, **hide untracked**, and **show ignored**, plus
**auto-unfold**. `Space F c`,
`Space F o`, `Space F u`, and `Space F i` toggle them from any pane. The same
`c`, `o`, `u`, and `i` keys work while File list has focus. Only
reviews means files with an active or resolution-proposed, non-archived thread
in the current workspace. Filters combine, and the File list header names active
filters compactly as `c`, `r`, `u`, and `i`.

`Space f` contains file-opening workflows: `f` opens the ordinary picker, `i`
includes ignored paths, and `r` lists files opened during this viewer session.
`Space F` contains File-list controls: the four filters plus `Z` to toggle
auto-unfold without moving focus from another pane. `Space t`
contains thread workflows, while `Space T` contains Thread-list controls:
`s` and `x` change scope and resolved visibility, and `Z` folds or unfolds all
file groups without moving focus. Section rules in mixed leader menus separate
modes, endpoint presets, durable actions, and settings.

The focused pane marks its name with a purple `▎`; filenames, counts, filters,
and controls remain neutral. The selected list row keeps its blue active or
remembered treatment independently. Menus, popups, prefixes, drafts, and
command/search input suspend the pane marker.

Use `w` and `W` to cycle focus forward and backward through the current main
surface, File list, and Thread list, skipping hidden panes. Layout show/hide
actions do not take focus. Hiding a focused sidebar pane returns to the
current main surface. In a list, movement and clicks preview content without
leaving the list; `Enter` explicitly opens and focuses File source or the
Threads evidence fallback. The mouse wheel scrolls only the pointed viewport:
it never changes selection, preview, focus, or the main surface. Dragging the
sidebar divider resizes it.

`Tab` and `Shift-Tab` follow the pane that owns navigation. In File and File
list they cycle workspace-wide open threads and land in File. In main Threads
they cycle only open threads admitted by its current view and filters and
stay in Threads. In Thread list they use that pane's scope and filter, retain
pane focus, and preview the current main surface. If a pane target is outside
the current main Threads view, main Threads stays unchanged and the status
says to press Enter to open it.
Thread actions, menus, and history use the logical thread and message retained
by the surface that owns focus. Returning from a rejected Thread-list preview
therefore acts on the still-visible main Threads entry, while the sidebar keeps
its own selection for the next focus return.

In main Threads, `j`/`Down` and `k`/`Up` walk the visible review vertically:
each message in an expanded conversation, then the adjacent thread or file
row. Folded threads and file groups are one stop, and movement stops at the
top and bottom. On a selected thread or file-group header, `h`/Left collapses
it and `l`/Right expands it. `Tab` and `Shift-Tab` remain the direct
next/previous open-thread loop. File keeps ordinary character movement for
`h`/`l` and passes vertically over inline thread headers as before.

A thread jump temporarily reveals a folded destination without changing the
remembered fold. File centers the visible source, detached marker, or
file-wide anchor through the newest reply when it fits, otherwise the newest
reply start; main Threads does the same from stored origin context. The
thread and text cursors sit on that newest reply. Failed navigation keeps the
current reveal. Changing the thread scope or filter, or switching worktrees,
ends it. An explicit `z`, `Z`, Enter, chevron, double-click, or menu fold
takes over, so a later jump cannot undo that explicit choice.

`Alt-Left` and `Alt-Right` walk logical navigation history. They restore the
exact message in a temporarily revealed main Threads conversation even when
Thread list previews another entry. In File they restore projected Normal
deletion rows by Base line and within-line position rather than substituting a
nearby Target line; comments started there therefore retain Base evidence.
Viewport scroll offsets are recomputed from the restored logical position.

In File list, `z` folds or unfolds the selected directory, or the immediate
parent when a file is selected. Folding a file's parent leaves the cursor on
that directory, so another `z` unfolds it. A root-level file has no foldable
parent row. `Z` turns on auto-unfold: every directory admitted by the current
filters is recursively unfolded, including directories added or revealed
later, and the File-list status shows `Z`. Press `Z` again to fold everything
and leave the mode. A manual keyboard or mouse fold/unfold also leaves the
mode; navigation and selection do not. Recursive unfolding skips directory
symlinks. Previewing a file keeps its draft parked; explicitly opening File
resumes it.

The configured split remains intact on a narrow terminal. When it cannot fit,
Fathomable replaces pane content with a size warning showing the required and
current dimensions. Resize, use **Layout** to hide a pane, or quit; recovery
preserves focus, selections, previews, and scroll positions. With the app bar
shown, minimum terminal sizes are `20×5` for the main surface, `28×5` with
File list, `28×6` with Thread list, and `28×8` with both; hiding the app bar
subtracts one required row.
Hidden drafts, pickers, and confirmations cannot accept input under the size
warning; `Esc` dismisses them without submitting. The Thread-list action
footer appears only while that pane owns navigation. At minimum height, the
File footer yields to a deletion banner so a content row remains visible.

### Essential keys

`Space ?` opens the complete keymap. Key prefixes show their continuations;
the table below is a quick reference, not the full list.

| Keys | Action |
| --- | --- |
| `h` `j` `k` `l` | move left, down, up, right in File (arrow keys also work) |
| `j` `k` | next / previous visible review item in Threads (`Down` / `Up` also work) |
| `h` `l` | collapse / expand the selected thread or file group in Threads (`Left` / `Right` also work) |
| `gg` `G` | top / bottom |
| `Ctrl-d` `Ctrl-u` | half page down / up |
| `/` `n` `N` | search, next match, previous match |
| `v` `V` `y` | select characters, select lines, copy |
| `gf` | follow a file reference or URL |
| `Space f f` `Space f i` `Space f r` | file picker / including ignored / recent files |
| `Space F c` `Space F o` | only changed / only reviews in File list |
| `Space F u` `Space F i` | hide untracked / show ignored in File list |
| `Space F Z` | toggle auto-unfold in File list |
| `Space T s` `Space T x` | Thread-list scope / show resolved |
| `Space T Z` | fold or unfold all file groups in Thread list |
| `Space d n` `Space d u` `Space d o` | Normal / Unified / Off diff mode |
| `Space d s` `Space d t` | pick the diff source / target |
| `Space d d` | show uncommitted changes (current `HEAD` to working tree) |
| `Space d l` `Space d c` | show the latest commit or a specific commit against its first parent |
| `Space d w` | ignore whitespace |
| `Space d p` `Space d r` | save-and-select / manage review points |
| `Space v s` | source / rendered view for configured Markdown files |
| `Space v t` `Space v r` | toggle thread stubs / resolved stubs |
| `w` `W` | focus the next / previous displayed pane |
| `f` `F` | show and focus File / File list |
| `t` `T` | show and focus Threads / Thread list |
| `Alt-Space` | open and focus the top-left Fathomable menu |
| `Shift-Down` `J` / `Shift-Up` `K` | next / previous comparison change, placed at File's top third |
| `Shift-Right` `L` / `Shift-Left` `H` | next / previous changed file, with its first change at the top third |
| `Tab` `Shift-Tab` | next / previous open thread in the focused surface's scope |
| `Alt-Left` `Alt-Right` | back / forward through logical navigation positions |
| `c` `Space t f` | line comment or reply / file comment |
| `r` `R` | resolve or reopen / toggle one-shot auto-resolve |
| `z` `Z` | fold/unfold the nearest directory / toggle File-list auto-unfold; threads elsewhere |
| `Enter` `Ctrl-Enter` | submit draft / submit with auto-resolve enabled |
| `Shift-Enter` `Alt-Enter` | draft newline / draft newline fallback |
| `Ctrl-e` | edit with `$VISUAL` or `$EDITOR` |
| `Esc` | cancel transient UI; from a sidebar return to the current main surface |
| `Space ?` | keymap |
| `q` | quit after confirmation |
| `:q` `:quit` `:q!` `:quit!` | quit immediately |

Typing `:` opens command discovery above the status line. The list is filtered
by a case-insensitive fuzzy subsequence match, with exact and prefix matches
first. `Tab` selects the first match and cycles the matches from the original
query; `Shift-Tab` cycles backward. Typing or erasing starts a new query. The
selected command shows a brief description and its accepted arguments.

| Command | Description |
| --- | --- |
| `:about` | show version, license, and repository information |
| `:doctor` | inspect configuration, storage, workspace, and terminal diagnostics |
| `:help` | open or close the getting-started guide |
| `:mcp` | show setup steps for connecting an agent over MCP |
| `:quit` (`:q`, `:q!`, `:quit!`) | quit immediately without confirmation |
| `:status` | show live viewer, workspace, and storage status |
| `:<line>` | jump to a source line; numeric jumps are not completion candidates |

All current named commands take no arguments.

`:status` uses the same scrollable report popup as Doctor and MCP Setup.
Long values wrap, and each worktree has its own row. Scroll with `j`/`k`,
Down/Up, or the mouse wheel; the popup shows when more rows are available.
`Esc` or a click outside closes it. Clicking inside keeps it open, and
scrolling does not move the document underneath.

`Ctrl-e` uses a fresh owner-only temporary directory containing an owner-only
draft file. Editor replacements and backups placed beside the draft stay
private, and Fathomable removes this scratch directory on return, including
error returns. A forced termination may leave private scratch files behind.
Unsafe temporary-directory ancestors are refused before the draft is written.
Files the editor is configured to write elsewhere are outside this protection.

### Switch worktrees

Choose **Go > Worktrees...**, or click the repository/worktree identity in
the menu bar when another checkout is available. `Alt-Space` exposes the
menus even when the bar is hidden. Worktree navigation is menu-only; the
old bracket shortcuts have no aliases.

The repository name stays the same across checkouts. The active branch or
short detached commit identifies the checkout; the picker and `:status`
also show Git's lock state and reason. A lock prevents Git pruning, not
viewing or selecting that worktree.

Switching preserves the open relative path and line when present in the
destination. A missing file closes rather than showing the previous
checkout's source as current. If Git removes the active worktree, it is
shown as unavailable without switching automatically. Use **Go >
Worktrees...** to select a surviving checkout, even when only one remains.
Discovery failures explicitly label the last-known list rather than claiming
the workspace is not Git.

### Compare versions

One session-global diff mode applies to every file and review/history view.
The rightmost control in each File or Threads header reads **Diff: normal**,
**Diff: unified**, or **Diff: off**. Click it for the three mode choices, or
use `Space d n`, `Space d u`, and `Space d o`:

- **Normal** shows Target content with comparison gutters, counts, File list
  filtering, and hunk navigation.
- **Unified** shows the selected Source-to-Target patch. It follows file
  switches and remains active when you press `Esc`.
- **Off** is Target-only source browsing. It shows no diff Source content or
  Source-only paths and suppresses comparison, Git-status, and file-edit toasts
  and counts. Plain activity notifications remain visible.

Off retains the selected Source, the whitespace setting, the **only changed**
File list filter, and the last active Normal/Unified mode so they return when
diffs are enabled. Their controls are dormant while Off. Changing Target keeps
Off active; choosing Source attempts to restore the last active mode. If that
pair cannot be read, both endpoint choices remain selected and the viewer stays
Off with an error. A Source-only file that was already open says **not present
in Target** and shows no diff Source body. Threads still shows clearly labelled immutable
origin excerpts and stored discussion history because those are review
evidence, not source browsing.

In Normal and Unified, the Source and Target at the right of the menu bar apply
to every file. Off shows only the clickable Target. Click an endpoint, or use
`Space d s` / `Space d t`, to choose a commit, tag, branch commit, index
(staged content), or working tree (files on disk). Review points are Source-only.
These choices never fetch, check out, or modify Git. A pending new-line or
new-file comment must be submitted or cancelled before changing mode, Source,
or Target.

A fresh Git workspace compares symbolic `HEAD` to the working tree and
remembers that following intent separately from the resolved commit. When the
same branch advances, every policy immediately displays new
`HEAD -> WorkingTree`; `diff.head-transition` decides whether future movement
continues to follow or pins the new commit. `ask-pin` (the default) follows and
offers **Pin here**; `ask-follow` pins and offers **Follow HEAD**; `pin` and
`follow` apply those outcomes without asking. Branch switches and detached,
unborn, unavailable, or linked-worktree HEAD movement do not change symbolic
intent.

The persistent choice appears even when `watch.toast` is zero. Click it, choose
the transition row in the Diff menu, or press `Space d d` to open the same
two-choice card. `Esc` or clicking outside dismisses the choice and keeps the
policy's immediate outcome. A newer transition supersedes it. The `Space d`
card begins with **normal diff**, **unified diff**, and **diff off**, then
**pick source…**, **pick target…**, **show uncommitted changes**, **show latest
commit**, and **show a specific commit…**. **show uncommitted changes**, or
`Space d d`, normally selects symbolic `HEAD` as Source and the working tree
as Target; while a HEAD or Index transition choice is pending, it opens that
choice instead. **show latest commit**, or `Space d l`, selects the immutable `HEAD~1`
to `HEAD` pair. **show a specific commit…**, or `Space d c`, opens a
commit-only picker, compares the chosen commit with its first parent, and
retains that commit as the active review. A root commit uses
`EmptyTree -> commit`; merges use the first parent, and an unavailable parent
fails. **save review point** and **manage review
points…** follow a separator, and **ignore whitespace** follows another. The
active Normal, Unified, or Off row carries the same `▌` marker as the top
Diff menu. There is no comparison-control popup, **Start comparison at current
HEAD**, or `:diff`.

The endpoint cards are titled **diff source**, **diff target**, and **diff
commit**. Nested cards retain that role and add their path in parentheses, such
as **diff source (tags)**, **diff target (branches)**, or **diff commit
(branches)**. Card metadata names what it counts, such as **24 branches** or,
while filtering, **8 of 397 choices**; the optional review-point name card has
no counter. Long branch names are shortened before they can hide the typed
query, and narrow cards drop count metadata before the query. Commit rows keep
the subject prominent while the short hash and UTC date are dim.

Rendered/Source is available in Normal and Off only for files matched by
`markdown.extensions` or `markdown.names`, including custom extensions and
extensionless names. Other files stay in source view: the menu action is
greyed out, and `Space v s` explains why it is unavailable without changing
the view. Each Markdown file retains its own source/rendered choice
when switching files. Unified retains that choice but disables the toggle
until another mode is selected. `:status` and file information report Target
only while Off.
Normal comparison provenance moves out of the bottom status line when the
menu-bar endpoint controls actually render; stale/error status remains, and
provenance returns there when the controls are hidden or too narrow.

`Space d p` captures the active checkout's working tree as an optionally named,
immutable **review point** without changing Git, then selects that exact point
as Source against the working tree. The resulting comparison is normally empty;
later edits show what changed since the save. Capture ignores File-list filters,
and reported exclusions remain part of the point. A manifest write failure can
leave publication uncertain; reload before retrying when the notice says so.
Choose any saved point under the Source picker's **Review points...**. See
[comparison and review-point details](decisions/0087-global-comparisons-and-board-history.md).

`Space d r` or **Diff > Manage review points...** opens the searchable
repository-wide point list. Enter or click a row to open its action card:
`r` renames, `d` enters the separately guarded deletion confirmation, and
`Esc` returns to the list. In the comparison **Review points...** picker,
`Ctrl-r` renames the highlighted point directly; the visible rename hint is
clickable too.

The one-line rename editor starts with the current name. Enter submits and Esc
returns without changing it; blank or whitespace-only input clears the name.
Names are trimmed, case-sensitive, unique among active points, limited to 128
Unicode scalar values, and cannot contain controls or Unicode line/paragraph
separators. The point list reloads after a rename and keeps the same immutable
point ID selected when possible. A concurrent external rename is never
overwritten: the viewer reloads the list and reports the stale conflict.

Deletion still requires the separately rendered confirmation and `y`; Esc
returns to the point card. Deletion removes the point from comparison selection
and reclaims only content blobs no other point uses. Threads keep their
immutable point ID, baseline, content identity, excerpt, and messages. This is
logical deletion with best-effort reclamation, not secure erasure. If the
deleted point is the selected Source, Fathomable replaces Source with the current
pinned `HEAD` (or EmptyTree) while preserving Target, diff mode, and
whitespace. A pending new annotation must be submitted or cancelled before
management.

Threads created from a review point belong to that exact point, not merely to
its Git baseline. Fathomable can record the first commit whose regular blob at
the recorded path exactly matches the complete captured point-side bytes, but
that landing is evidence only and does not make the thread a member of a
commit presentation. Deleting a point removes the point view while preserving
immutable thread and landing evidence.

Use `Shift-Down` or `J` and `Shift-Up` or `K` to cycle every comparison
change across the workspace. Text hunks are individual stops; a changed path
with no text hunk, such as a binary or mode-only change, is one stop. Use
`Shift-Right` or `L` and `Shift-Left` or `H` to cycle changed files in either
direction; both land on the first diff in the destination file. Both cycles
follow the selected comparison and are unavailable in Off mode. File list
expands and centers a listed destination without taking keyboard focus; a
hidden File list catches up when shown.

### Review discussions

Select lines and press `c`, or use `Space t f` for a file-wide comment.
Threads retain their original excerpt even if edits move or detach them.
Keep comments focused; each new message is limited to 1024 UTF-8 bytes.
When placing a thread from stored evidence, the viewer and MCP try an exact
full-anchor match first. If that fails and its context was truncated at
capture, the thread stays detached rather than guessing from partial text;
the stored excerpt remains readable.

An expanded entry in the wide main **Threads** surface places its stored
immutable origin context before the conversation. Source uses the origin
path's syntax, wraps without line numbers, and marks the originally selected
rows with the File view's highlighted anchor cell: `●` for one rendered row,
or `╭│╰` for a wrapped or multi-line range. The surrounding source keeps one
background and the four-cell indent. Fathomable prepares at most 16 KiB and
256 logical source rows; an omission marker preserves the beginning and end of
a larger selection, and a truncation row says when capture or preparation
omitted evidence. File-wide threads and compact **Thread list** cards do not
show this block. Moved, detached, historical, or unavailable entries instead
add one short original location warning; unchanged placement adds none.
Preparing this block uses only stored evidence, never a fresh checkout or Git
read.

Full File and main Threads thread headers place a neutral `@` plus seven
commit characters after the location when the origin captured a commit,
observed `HEAD`, or review-point baseline. For mutable origins this is the
captured reference, not a claim that the retained bytes equal that commit.
Compact Thread-list cards omit it. Narrow full headers keep the reference
ahead of age, reply count, optional context, and textual lifecycle status.

The lifecycle marks are **● active**, **◐ resolution proposed**, and
**○ resolved**. You resolve or reopen with `r`. Enabling `R` gives the
agent one chance to resolve: its next reply consumes that permission.
Without permission, an agent's completion request is a proposal for you to
review.

Use `Tab` and `Shift-Tab` from any normal pane to cycle active and
resolution-proposed threads across the workspace. Resolved and archived
threads are skipped. Source opens when it can be displayed; otherwise
Fathomable selects the expanded Threads entry and its stored evidence.
File list follows the destination path without taking focus from File or
Threads. Active File list filters still apply: an excluded destination is not
fabricated, and its reveal waits until the listing admits it.
This navigation never switches worktrees: choose **Go > Worktrees...**
explicitly.

The File footer keeps the core loop visible as
`comment c · diffs ⇧arrows/HJKL · threads (⇧)Tab`, omitting unavailable
actions. On a thread row, `comment c` becomes `reply c`; where both fold
actions apply, the footer uses `folding z/Z`.

The stored board is shared across the repository's worktrees and survives
commits and branch changes. Normal membership follows immutable origin
provenance and the Source/Target pair that last reached the screen. Working
tree, index, commit, review point, and empty tree remain distinct identities;
a clean WorkingTree or Index never aliases its observed `HEAD`.

A Commit(C) Target- or Unspecified-side thread belongs whenever Target is
Commit(C), including `C^ -> C`, another commit to C, and Diff Off Target C. It
does not belong in `C -> WorkingTree` or `C -> Index`. A Base-side thread with
recorded comparison facts belongs only to that exact accepted ordered pair,
which keeps deletion review context without admitting it to every comparison
sharing Source. If either recorded endpoint is WorkingTree or Index, its
comparison checkout must also be the accepted presentation checkout, so equal
`HEAD` values in linked worktrees do not alias. Wholly immutable comparisons
remain repository-wide. Without comparison facts, Base requires an exact typed
Source match in an active diff.

WorkingTree and Index Target- or Unspecified-side origins require the matching
typed Target in their recorded checkout. Review-point origins require the
exact point. Diff Off is Target-only and never admits Base-side origins.
Unknown or incomplete provenance is absent from normal surfaces, but explicit
history and direct thread-ID actions remain available.

Membership does not guarantee an inline mark: the current endpoint and side
must be eligible and anchor/context placement must succeed. Detached listed
threads remain reply-, edit-, and resolution-actionable. Landing records the
first observed exact full-file commit match, but never adds, removes, or
changes membership.

**Review** also offers repository-wide **Recently resolved**, **Archived
threads**, and **Clear board...**. Clearing archives the shared board after
confirmation; the confirmation presents **clear** before **cancel**, accepts
`Enter` or a click on **clear**, and cancels with `Esc`, a click on **cancel**,
or a click outside. It does not delete history. Restore an archived entry
with `u`. Nothing archives automatically.

## Configuration

Configuration uses **KDL** at `$XDG_CONFIG_HOME/fathomable/config.kdl`
(default `~/.config/fathomable/config.kdl`). No file is required: omitted
settings use defaults, and unknown settings are errors.

KDL uses named nodes, quoted strings, numbers, `#true` / `#false` booleans,
`{ ... }` child blocks, and `//` comments. Separate nodes with newlines or
semicolons. This is the complete default configuration:

```kdl
theme "default-dark" // Built-in theme or a custom theme name from themes/.

watch {
    toast 5000 // Toast duration in milliseconds; 0 disables toasts.
    ignore // Extra root-relative globs excluded from live-change notifications.
    debounce 300 // Quiet period in milliseconds for workspace and Git changes, not threads.
}

markdown {
    extensions "md" "markdown" "mdx" // Markdown extensions, case-insensitive and without dots.
    names "readme" "license" "licence" "copying" "changelog" "contributing" "authors" "notice" // Extensionless Markdown filenames, case-insensitive.
}

viewer {
    max-file-size-mib 64 // Largest text file to load, in MiB; larger files show file info.
}

limits {
    discovery-entries 100000 // Entry count; positive; no unlimited value.
    workspace-watches 8192 // Watch count; positive; no unlimited value.
    retained-paths 50000 // Path count; positive; no unlimited value.
    comparison-paths 10000 // Path count; positive; no unlimited value.
    comparison-bytes 67108864 // Byte count; positive; no unlimited value.
    pending-events 4096 // Event count; positive; no unlimited value.
}

layout {
    menu-bar #true // Show the menu bar at startup.
    sidebar {
        visible #true // Show the sidebar at startup.
        files #true // Include File list in the sidebar.
        threads #true // Include Thread list in the sidebar.
        width 32 // Sidebar columns, capped at one third of the terminal.
        split 8 // Thread list rows when both sidebar panes are shown.
    }
}

threads {
    stubs #true // Show inline thread summaries.
    stubs-resolved #false // Include resolved threads in inline summaries.
}

diff {
    mode "normal" // Startup presentation: "normal", "unified", or "off".
    context 3 // Unchanged lines shown around each diff hunk.
    ignore-whitespace #false // Default only; saved comparisons keep their whitespace rule.
    head-transition "ask-pin" // After HEAD moves: "ask-pin", "ask-follow", "pin", or "follow".
}

user {
    name "User" // Your non-empty display name for review comments.
}
```

`fathomable --config-show` prints the effective configuration and its path.
Custom themes live in `$XDG_CONFIG_HOME/fathomable/themes/`; `--theme NAME`
overrides the theme for one run. `ui.pane.focus` controls the focused pane
marker and name foreground; inherit it from a built-in or set it explicitly.

When upgrading from a configuration with `jump { toast ... }`, move `toast`
into the `watch` block and remove the obsolete `jump` block. `jump` is no
longer accepted as a top-level node.

## Connect an agent

Open `☰->Help->MCP Setup` or run `:mcp` in the viewer for MCP setup
steps.

Register `fathomable --mcp` as a stdio MCP server in your agent host.
It binds to the launch workspace; add `/path/to/checkout` to bind explicitly.
It can run without a viewer and never follows the viewer to another worktree.

```sh
copilot mcp add fathomable -- fathomable --mcp
claude mcp add --scope user fathomable -- fathomable --mcp
codex mcp add fathomable -- fathomable --mcp
```

For Copilot CLI, the command adds the server to
`~/.copilot/mcp-config.json`. Launch Copilot from the checkout you want to
review. To configure it manually, merge this entry into the existing
`mcpServers` object rather than replacing other servers:

```json
{
  "mcpServers": {
    "fathomable": {
      "type": "local",
      "command": "fathomable",
      "args": ["--mcp"],
      "tools": ["threads", "thread_start", "thread_reply"]
    }
  }
}
```

### VS Code

Register the server in your user profile:

```sh
code --add-mcp '{"name":"fathomable","type":"stdio","command":"fathomable","args":["--mcp","${workspaceFolder}"]}'
```

Open the checkout as a workspace and approve the server when prompted.
`${workspaceFolder}` explicitly binds Fathomable to that workspace. Use
**MCP: List Servers** to check its status. Fathomable must be on the server's
`PATH`; use the executable's full path if the editor cannot find it.

For workspace-scoped setup instead, merge this into `.vscode/mcp.json`:

```json
{
  "servers": {
    "fathomable": {
      "type": "stdio",
      "command": "fathomable",
      "args": ["--mcp", "${workspaceFolder}"]
    }
  }
}
```

For Remote SSH or containers, install Fathomable in that Linux environment
and configure it through **MCP: Open Remote User Configuration** or the
workspace's `.vscode/mcp.json`, rather than a local user-profile server.
Run the viewer in the same environment and OS user account so it shares the
server's state directory.

See the upstream [Copilot CLI setup](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers)
and [VS Code setup](https://code.visualstudio.com/docs/agent-customization/mcp-servers)
for host-specific options.

### Tools and identity

| Tool | Purpose |
| --- | --- |
| `threads` | Read discussions without changing state; exact IDs can retrieve archived history, and an optional commit source filters immutable origins. |
| `thread_start` | Start comments from WorkingTree by default, or from an explicitly selected immutable commit. |
| `thread_reply` | Reply to discussions, optionally requesting resolution. |

`thread_reply` identifies its target by thread ID. Omit `line` and `end_line`
to reply without relocating it, including when the source is detached in the
bound checkout; supplying coordinates explicitly requests relocation and
therefore requires readable source in that checkout.

#### Per-call commit sources

Only `threads` and `thread_start` accept the optional top-level selector:

```json
{
  "source": {
    "kind": "commit",
    "revision": "HEAD"
  }
}
```

`revision` is exactly uppercase `HEAD` or a full 40-hex SHA-1 commit ID;
abbreviations, branches, tags, parent expressions, ranges, paths, URLs,
whitespace-padded values, and lowercase `head` are rejected. Hex input may be
uppercase, but every successful selected response includes the canonical
lowercase full ID as top-level `resolved_commit`, including empty reads,
zero-limit reads, and keyed write replays. Omit `source` or pass `null` for a
repository-wide board read or a working-tree start. `thread_reply` has no
`source` field and is unchanged.

For example, a selected read with no matches still confirms the pinned source:

```json
{
  "checkout": "/path/to/checkout",
  "resolved_commit": "0123456789abcdef0123456789abcdef01234567",
  "threads": [],
  "more": 0,
  "next_after": null
}
```

To review a commit, pass `source`; without it `thread_start` records a
WorkingTree origin even when its `observed_head` names that commit. A selected
start captures every fresh comment's path, range, snippet, and content identity
from the resulting tree of that commit:

```json
{
  "source": {
    "kind": "commit",
    "revision": "0123456789abcdef0123456789abcdef01234567"
  },
  "comments": [{
    "path": "src/lib.rs",
    "line": 12,
    "end_line": 14,
    "body": "Handle the empty input before indexing.",
    "idempotency_key": "review-empty-input"
  }]
}
```

Root and merge commits mean their resulting trees; no parent, merge base, or
implicit diff is selected. The source applies to the whole call, is not saved
as a server or viewer selection, and does not change a checkout, index, ref,
comparison, or running viewer. A keyed start's durable intent includes the
selected full commit, so reuse against another commit conflicts. `HEAD` is
resolved once for a call; use the returned full ID for an exact retry if
`HEAD` may move.
Matching full-ID retries still work after resolution, reopening, or archival,
even if resolution records a later commit or the original Git object is gone.
Deleted discussions cannot be recreated by retrying their keys.

A selected read returns only discussions whose immutable
`origin.version.kind` is `commit` with that exact ID. Working-tree, index, and
review-point origins are excluded even when their observed `HEAD` matches.
They remain excluded after durable viewer landing on that commit: landing does
not rewrite immutable origin or broaden MCP source selection.
`path` filters the immutable `origin.path`. In each result, `origin` remains
the historical evidence; top-level `path`, `anchor_range`, and
`placement_evidence` are the stored current references; and top-level
`range`, `placement`, and `location` report projection in the MCP server's
bound checkout. Origin and current placement can therefore have different
paths or ranges, or the current placement can be detached.

Selected pagination pins the commit in the cursor:

```json
{
  "source": {
    "kind": "commit",
    "revision": "0123456789abcdef0123456789abcdef01234567"
  },
  "after": {
    "updated": 1789876800,
    "id": "1789876700-1234-1",
    "resolved_commit": "0123456789abcdef0123456789abcdef01234567"
  }
}
```

The cursor commit must match `source`. `HEAD` cannot be combined with
`after`; continue with the preceding response's `resolved_commit`. Selected
reads retain the ordinary status, update-time, ordering, limit, and archive
rules, but cannot be combined with exact `ids`.

Commit selection reads only objects already available in the repository
bound at MCP startup. It never fetches, runs Git or another external process,
checks out files, or mutates refs. Git replacement objects cannot substitute
another commit, tree, or blob for the selected immutable objects.
A fresh selected start has one 64 MiB raw blob budget across its distinct paths,
including loaded blobs rejected as binary or invalid UTF-8. Repeated paths
reuse the first result, including failures. Absent paths and Git directories,
symlinks, submodules, unsupported modes, or non-blob entries fail explicitly;
binary and invalid UTF-8 blobs are rejected as text sources. Commit and tree
metadata objects over 64 MiB are rejected before decoding, and exact object
loads use the same allocation ceiling. A selected read loads no historical
file bodies beyond the origin evidence already stored.

Writes need native chat identity: Copilot CLI's `COPILOT_AGENT_SESSION_ID`,
Claude Code's `CLAUDE_CODE_SESSION_ID`, VS Code's
`params._meta["vscode.conversationId"]`, or Codex's `params._meta.sessionId`
and `params._meta.threadId`. Reads need no identity; unsupported writes fail
explicitly. See [identity details](decisions/0080-automatic-chat-identity.md)
and [tool contracts](decisions/0084-explicit-mcp-contracts.md).

MCP always reads and writes the shared store directly, whether or not a viewer
is running. A successful write reports persisted state, not a rendered frame.
Viewers observe thread changes without waiting for `watch.debounce`; several
agent updates may be grouped into one counted activity toast. Known watch
failures or failed store refreshes report degraded thread updates and retry.
A previously loaded board stays visible if its store disappears or cannot be
read; refresh never recreates or overwrites the missing data.

MCP working-tree source and placement reads stay within the bound checkout.
Relative symlinks within that checkout work; absolute symlink targets and
links escaping it (including directory links) fail explicitly. This also
applies when an existing thread's file becomes an escaping symlink before a
read or relocation. Commit-selected origin reads instead address immutable
regular blobs in the bound local Git object database and never follow
checkout symlinks.

## What gets saved where

Product state lives under `$XDG_STATE_HOME/fathomable`
(default `~/.local/state/fathomable`), not in your checkout:

| Relative path | Contents |
| --- | --- |
| `workspaces/<repository-hash>/threads.jsonl` | Discussion messages, immutable origin excerpts and provenance, placement, landing evidence, and lifecycle history. |
| `workspaces/<repository-hash>/review-points/` | Explicitly saved manifests and content blobs. |
| `workspaces/<checkout-hash>/comparison/comparison.json` | Last-used endpoints, aliases, Source intent, and diff settings for that checkout. |
| `viewers/` | Viewer registrations. |
| `log/` | Diagnostic logs and crash reports. |

Application-owned state directories are created private (0700), and sensitive
files are created owner-only (0600), even with umask 000 or 022. Existing
directories and files must have these modes and belong to the running user's
effective UID. Symlinks, multiply linked or special files, unsafe ownership,
and loose permissions are refused with an error, not repaired, migrated, or
deleted. Fathomable does not chmod your HOME or XDG base directories.
Non-sticky world-writable ancestors are refused. Group-writable external
ancestors are allowed, but members of that group can rename entries and
interfere with state-path availability or integrity. Startup shows a status
warning, and `fathomable --doctor` or `:doctor` lists the affected paths
without reporting overall failure. Remove group write with `chmod g-w` on
each listed directory, or choose a private `XDG_STATE_HOME`; mode 0700 is a
more restrictive alternative. Read-only configuration is unaffected. Linux
`/proc/self/status` must be readable to obtain the effective UID.

These permissions protect against other local OS users, not root or programs
running as your UID. Source excerpts, messages, saved blobs, paths, and crash
details remain sensitive after the original checkout changes or disappears.
Review logs, reports, and backups before sharing them. See the
[private-state contract](decisions/0009-cli-and-diagnostics.md#persistent-state-privacy)
for the path and ownership checks.

There are no viewer IPC sockets or runtime-directory requirements.
`--viewers` lists viewer metadata without socket paths; `--doctor` checks
state and configuration without runtime socket checks. Old socket artifacts
are unused and are not automatically removed.

External-editor drafts use `fathomable-comment-<pid>.md` in the system temporary directory
and are removed after returning from the editor.

The Git common directory identifies the repository, so linked worktrees share
threads and review points but keep separate comparison preferences.
Plain directories use their root identity. Running viewers do not control
one another's comparison; the last successful preference write is restored
on a later launch.

Reading does not save full-file snapshots. Review points explicitly save
eligible changed content and rely on Git objects for unchanged committed
files; they are not standalone backups. Outside Git, points capture all
eligible files.

Annotation format 8 and comparison preference format 3 deliberately have no
compatibility reader, migration, backfill, or dual-format path. Before a
manual reset, stop every affected viewer and the agent host's MCP processes.
Format 8 comparison provenance includes a checkout qualifier whenever its
Source or Target is WorkingTree or Index.
Delete only `workspaces/<repository-hash>/threads.jsonl` for incompatible
annotation state and/or
`workspaces/<checkout-hash>/comparison/comparison.json` for that checkout's
incompatible comparison preference. Do not remove `review-points/`,
configuration, logs, or unrelated workspace state. Start only the new build
after the reset. Fathomable refuses incompatible state and never deletes it
during startup, build, or installation.
Review the actual XDG paths before backing up or resetting app-owned state;
never remove repository files as part of a reset.
A backup is recovery material for a build that understands that exact stored
format, not a migration format for another build. Configuration is preserved
by default, but obsolete settings may need manual changes.

For paths and connection diagnostics, run
`fathomable --doctor`; `fathomable --viewers` lists running viewers.

## Contributor performance profiles

`just perf path/to/file.md` runs the local Linux CPU profiler described in
[CONTRIBUTING.md](../.github/CONTRIBUTING.md#2-the-gate). It uses a separate optimized
frame-pointer build and keeps the exact sampled executable with owner-only
artifacts under Cargo's `target/perf/` directory. These artifacts can contain
source paths and terminal content; inspect them before sharing. Caller stacks
from an older capture cannot be repaired after recording.

For repeatable availability checks, build the release executable, then run
`python3 scripts/test-workspace-performance.py --bin target/release/fathomable`
(adjust the executable path for a target-specific build). This Linux,
Python-standard-library harness creates isolated synthetic wide/deep
workspaces and temporary state; it never scans the real home directory.
It checks first-frame overhead against a small fixture (250 ms), correlated
input-response p99 during discovery/churn (50 ms), quit latency (100 ms),
finite inotify coverage, visible incomplete status, and warm RSS stability.
It prints aggregate JSON and removes its named temporary fixture afterward.
Run it on a quiet host: these are measured regression thresholds, not
hard real-time guarantees for arbitrary filesystems.
