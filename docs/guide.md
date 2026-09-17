---
type: Guide
title: Setup guide
description: Install Fathomable, compare workspace versions, review discussions, configure the viewer, and connect MCP.
tags:
  - onboarding
---

# Setup guide

Fathomable is a read-only terminal viewer and repository discussion side-car.
It reads the checkout and Git object database, but writes product state only
under XDG state directories.

## 1. Install

Fathomable currently supports Linux and requires Rust 1.97 or newer. The
repository pins the toolchain.

```sh
git clone <this repository> fathomable
cd fathomable
make install
fathomable --version
```

Contributors should also install the tools in
[CONTRIBUTING.md](../CONTRIBUTING.md) and run `scripts/gates.sh` before a
commit.

## 2. Launch and state compatibility

```sh
fathomable README.md
fathomable
fathomable path/to/checkout
fathomable --name review
```

A file argument opens its enclosing workspace and displays the file. A
directory opens the workspace. `--name` labels one viewer; it does not route
MCP or share a comparison with another viewer.

A Git workspace is the repository, including linked worktrees. Threads and
review points are shared through the Git common-directory identity. Each
running viewer owns a comparison for its active checkout. `]w` and `[w`
switch worktrees; the newly active checkout restores its own comparison
preference or starts at its pinned `HEAD`.

Viewers on the same checkout do not control one another while running. They
share the checkout's last-used preference; the last successful preference
write is what a later viewer restores.

The current annotation format is **5** and the internal viewer/MCP socket
protocol is **8**. After installing a new build, restart existing viewers,
MCP children, and the agent host connection so both socket ends match.

An older `threads.jsonl` is refused with its path and version. Fathomable
does not migrate, delete, or reset it automatically. Old `seen/` and
`checkpoints/` directories are inert and may be reviewed or removed manually
later; installing or starting this version never deletes them.

## 3. Keys

`Space ?` opens the complete keymap. Prefixes show their available
continuations. These tables contain the main bindings.

### Text and navigation

| Keys | Action |
| --- | --- |
| `j` `Down` | move down |
| `k` `Up` | move up |
| `h` `Left` | move left, wrapping to the prior row |
| `l` `Right` | move right, wrapping to the next row |
| `0` `Home` | rendered-row start |
| `$` `End` | rendered-row end |
| `gg` | top |
| `ge` `G` | bottom |
| `gh` | logical line start |
| `gl` | logical line end |
| `Ctrl-d` | half page down |
| `Ctrl-u` | half page up |
| `/` | search forward |
| `?` | search backward |
| `n` | next match |
| `N` | previous match |
| `v` | character selection |
| `V` | line selection |
| `x` | select this line, then extend downward |
| `y` | copy selection or current line |
| `gf` | open a local file reference or external URL |
| `Alt-Left` | previous jumplist position |
| `Alt-Right` | next jumplist position |
| `]g` `[g` | next or previous comparison hunk across files |
| `]G` `[G` | next or previous file in the comparison |
| `]f` `[f` | next or previous queued live change |
| `]w` `[w` | next or previous worktree |
| `Space f` | visible file picker |
| `Space F i` | picker including ignored files |
| `Space F r` | recently opened files |
| `Space F c` | only changed files, or all files |
| `Space F u` | hide or show untracked files |
| `Space F g` | show or hide ignored files |
| `Space j j` | newest queued live change |
| `f` | open or focus the File view |
| `Space v s` | source or rendered presentation |
| `Space v t` | show or hide inline thread summaries |
| `Space v x` | show or hide resolved inline summaries |
| `:` | command line |
| `Esc` | clear transient input/selection, then leave a unified diff |

### Comparisons

| Keys | Action |
| --- | --- |
| `Space d d` | open **Comparison controls...** |
| `Space d b` | choose the global base |
| `Space d t` | choose the global target |
| `Space d c` | **Save review point**; type an optional name or accept unnamed |
| `Space d w` | compare or ignore whitespace |
| `w` | compare or ignore whitespace while text has focus |
| `:diff` | open the unified diff for the current global comparison |

### Discussions

| Keys | Action |
| --- | --- |
| `c` | comment on selected/current lines; reply from thread rows |
| `Space c c` | start a line comment |
| `Space c f` | start a file comment |
| `Space c r` | reply to the cursor thread |
| `Space c e` | edit your newest message |
| `Space c d` | delete the cursor thread |
| `t` | open or focus Reviews |
| `r` | resolve or reopen the cursor thread |
| `R` | toggle one-shot auto-resolve |
| `e` | edit your selected message |
| `dd` | delete the cursor thread |
| `z` | fold or expand the cursor thread |
| `Z` | fold or expand all threads |
| `Enter` | open/fold the selected thread or file row |
| `]c` `[c` | next or previous thread in this file |
| `]C` `[C` | next or previous thread across the board |
| `Space c R` | open **Recently resolved** |
| `Space c h` | open **Archived threads** |
| `Space c a` | **Archive resolved threads** |
| `Space c A` | **Clear board...** |
| `a` | archive the selected resolved review entry |
| `u` | restore the selected archived entry |
| `s` | current-file or workspace scope in the normal board |

### Panes, drafts, and pickers

| Keys | Action |
| --- | --- |
| `Space p s` | hide or restore the sidebar |
| `Space p m` | hide or show the menu bar |
| `Space p f` | hide or show the Files pane |
| `Space p t` | hide or show the Threads pane |
| `Space w h` | focus the pane to the left |
| `Space w j` | focus the pane below |
| `Space w k` | focus the pane above |
| `Space w l` | focus the text |
| `Space w w` | next pane |
| `Space w f` | focus Files |
| `Space w t` | focus Threads |
| `y` `Y` | copy the relative or full path from Files |
| `Enter` `Ctrl-Enter` | submit a draft; submit and enable auto-resolve |
| `Alt-Enter` | newline in a draft |
| `Alt-k` `Alt-Up` | scroll text above a draft upward |
| `Alt-j` `Alt-Down` | scroll text above a draft downward |
| `Ctrl-e` | edit a draft with `$VISUAL` or `$EDITOR` |
| `Left` `Right` `Up` `Down` | move inside a draft or picker |
| `Home` `End` | draft line start or end |
| `Alt-b` `Alt-f` | prior or next draft word |
| `Backspace` `Delete` | delete backward or forward |
| `Ctrl-w` | delete the prior word |
| `Ctrl-k` | delete to line end |
| `Ctrl-c` | clear a draft; close it when already empty |
| `Esc` | return to the parent comparison picker, then close the picker |
| `Space ?` | open **View keymap** |
| `Space a w` | report that Wake agent is not implemented |

Picker motion lets the cursor move freely between three-entry top and bottom
margins. Crossing a margin scrolls the list while keeping the cursor there.
At the beginning or end of the list, the cursor can move closer to that edge.
Hovering over a picker row highlights it, clicking chooses it, and the mouse
wheel moves the selection and list.

The Files header reads `Files` at the left. That word takes the shared hover
background under the pointer; clicking it opens **Only changed**, **Show
untracked**, and **Show ignored**. A checkmark means the setting is on.
The popup aligns with the sidebar and unfolds on the row below the pane
header, rather than opening at the pointer. Right-clicking the header does
nothing.
Active filter words appear passively before the `+n -m` comparison totals.
Right-clicking a file offers **Open**, **File comment**, **Copy path** (`y`),
and **Copy full path** (`Y`). Right-clicking a directory offers
**Expand/Collapse** and both path-copy actions.

The Threads header follows the same title-menu model. `Threads` is the only
hovered and clickable title region; its menu unfolds below the header with
checked **Only current file** and **Show resolved** settings. Subdued
`file` or `workspace` scope and lifecycle counts are passive and
right-aligned. On narrow panes scope shortens to `f` or `w`, then disappears
before the counts do. Clicking passive header cells only focuses the pane.
Right-clicking a file group offers **Fold/Unfold**, **Fold/Unfold all**, and
**Open file**; the pane-wide resolved setting stays in the title menu.
Right-clicking a thread offers **Go to**, **Reply**, auto-resolve,
**Resolve/Reopen**, conditional **Archive**, edit, and **Delete** actions,
plus **Fold file** in workspace scope.

The File view has its own header above the document. `File` is the only
hovered and clickable title region; the current filename follows passively,
and lifecycle counts for that file sit at the right.
Clicking `File` opens **Open Reviews** (`t`), then checked **Rendered view**,
**Show inline threads**, and **Show resolved threads** settings below a
separator. The presentation state stays in this menu rather than becoming
another header label. Bare `f` always opens and focuses File view.

The normal full-column review view reads `Reviews` at the left. Clicking
that title opens **Open File** (`f`), then checked **Only current file** (`s`)
and **Show resolved** settings below a separator, aligned with the text
column. Subdued `workspace` or `file` scope and lifecycle counts are passive
and right-aligned; scope shortens to `w` or `f` before disappearing. Bare
`t` always opens and focuses Reviews rather than toggling it closed; bare
`f` or `Esc` returns to the File view.

Paging or clicking file rows in the Files pane updates the current file
without closing Reviews. This lets file-scoped Reviews follow the sidebar.
An explicit **Open**, **Go to**, or `Enter` still enters File view.

Context and menu-bar popups put action labels at the left and their subdued
shortcuts at the right, with at least one cell between them. Those menus use
`Sp` as the compact spelling of `Space`.
The wheel steps one row and previews it, while dragging the sidebar divider
resizes the sidebar.

Commands are `:q`, `:source`, `:noh`, `:N`, `:diff`, `:status`,
`:name NAME`, `:help`, `:doctor`, and `:about`.

## 4. Global comparisons and review points

The status/header label is the source of truth:

```text
Compare: a1b2c3d -> working tree
```

The selected comparison stays active while opening different files. Its
changed paths drive the files pane, counts, gutters, `]g`/`[g`, and
`]G`/`[G`. A deleted or historical-only path opens from its selected
endpoint even when no matching file exists on disk. With a commit, index, or
empty-tree target, the Files pane and visible file picker are confined to
that target snapshot plus base-only deletions; files from newer checkout
history are not listed. A working-tree target lists the live checkout.

The base and target pickers begin with:

```text
Working tree
Index
HEAD
Tags...
Branches...
Review points...  (base only)
Advanced...
<short ID> <subject, ellipsized to fit> <YYYY-MM-DD>
```

The working tree is the current content on disk. The index is the staged
snapshot the next commit would record; it is not `HEAD`. `HEAD` is the
currently checked-out commit and resolves immediately to a pinned ID.
**Advanced...** contains the empty-tree endpoint.

The remaining rows are at most 500 commits reachable from `HEAD`, newest
first. The date is UTC and remains right-aligned while long subjects are
ellipsized. **Tags...** opens a searchable tag list and selecting one pins
its commit. **Branches...** searches both local and remote-tracking branches;
selecting one opens its commits from newest to oldest. These menus use only
local Git data and never fetch.

Rows that denote the selected endpoints show muted-blue **[current base]** and
**[current target]** hints at the right. On commit rows the hints appear just
before the date. Working tree, Index, HEAD, tag, and commit rows all use the
same endpoint identity, so a selected endpoint remains recognizable in nested
menus.

Typing four or more hexadecimal characters in the top-level picker searches
all matching commit IDs reachable from local branches, remote-tracking
branches, and tags, including commits older than the displayed 500. The first
four-character search walks IDs without decoding every old subject; extending
the prefix filters that result in memory. In a selected branch's commit menu,
the same search covers that branch's complete reachable history. Typed local
revisions and object IDs remain accepted. Press `Esc` to return from commits
to branches, or from another submenu to the main endpoint picker.

Selecting `HEAD` resolves it immediately. If it names commit `B`, later
commits do not move that endpoint. Open **Comparison controls...** with
`Space d d` and choose **Start comparison at current HEAD** only when you
deliberately want the base to become the checkout's current `HEAD`.

To compare a contiguous committed batch, open the base picker and type:

```text
first-revision..last-revision
```

The result is the net tree delta from the parent before the first commit to
the last commit. A root starts at the empty tree. A merge boundary is refused
until its parent is explicit.

The working-tree endpoint is the final on-disk state. Staged and unstaged
edits that cancel are absent from that net comparison. Select `index` when
you specifically want staged content.

`Space d c` opens the optional-name picker. Type a name and press Enter, or
press Enter on **save without a name**. The point records the observed Git
baseline plus content-addressed blobs for dirty or added text and tombstones
for deletions. It does not write Git or the checkout. Binary, oversized,
unreadable, raced, and ignored paths are reported honestly; an incomplete
point is not selectable.

To compare from a saved point, use `Space d b`, open **Review points...**,
and choose it as the base. The resulting point-to-working-tree delta shows a
reversal even when that reversal disappears from a commit-to-working-tree
comparison. Saving another point never selects it automatically.

Review points depend on their recorded Git objects for unchanged committed
files. If rewriting and garbage collection remove those objects, Fathomable
reports the point as unavailable instead of substituting current content.

## 5. Discussions and board history

A line or file comment stores immutable origin evidence:

- original path, range, snippet, and bounded surrounding context;
- exact source version and comparison side;
- the displayed comparison;
- working-tree, index, or review-point facts and content identity.

Removed diff lines originate on the base; added/context lines originate on
the target. A selection spanning both sides is refused. Committing,
rebasing, relocating, resolving, or switching comparisons never rewrites the
origin.

Current placement is separate. The viewer projects against the version it
displays; MCP projects against its bound checkout. Exact anchors and bounded
context are used when trustworthy. Otherwise the thread is detached and the
original evidence remains in the review entry. Fathomable stores no
automatic full-file reader snapshots.

Lifecycle is:

| Glyph | Meaning |
| --- | --- |
| `●` | active |
| `◐` | resolution proposed |
| `○` | resolved |

The user controls one-shot auto-resolve with `R`. The next agent reply
consumes it whether or not that reply asks to resolve. Without permission,
`resolve: true` records a successful resolution proposal for review in
Fathomable.

Bare `t` opens the normal board. Open and proposed threads remain board
members across commits, branches, and worktrees. Resolved threads are hidden
from the normal view until `x`, but remain available.

`Space c R` opens **Recently resolved**, ordered by the latest actual
resolution event. A later metadata edit does not reorder it. Reopening an
entry removes it; resolving again returns it at the new resolution time.

`Space c a` archives every thread that is still resolved under the store
lock. `a` archives the selected resolved review entry. Archive is neither
resolution nor deletion.

`Space c A` opens **Clear board...**. The confirmation names
active/proposed and resolved counts and warns:

```text
Archive the shared board across this repository and all worktrees?
```

It archives only the IDs and states you acknowledged. A discussion arriving
later is not included. If an acknowledged thread changes, Fathomable shows
updated counts for confirmation again. Escape cancels without changing
state.

`Space c h` opens **Archived threads**. Each entry retains messages,
lifecycle, resolution history, and original evidence. Press `u` to restore
it. A restored resolved thread is still resolved, and auto-resolve remains
disabled.

Clear board affects the repository's shared board across all linked
worktrees. It does not alter files, Git, comparisons, review points, or
thread history. Nothing archives automatically on startup, save point,
commit, branch change, idle, or quit.

## 6. Menu bar and configuration

The persistent first row is:

```text
☰  Layout  Go  Review  Diff          fathomable    HEAD to WorkingTree
```

The repository directory is centered in subdued, dim text. With multiple
worktrees, the active branch or short detached commit follows it; hovering
and clicking that repository/worktree segment opens the worktree picker.
The repository identity shortens or disappears on narrow terminals rather
than colliding with the menus or comparison. The current filename lives in
the File header instead. The compact comparison pair remains at the right;
full paths and source/view labels stay out of the bar. Immutable commits use
short IDs; the other labels are `HEAD`, `WorkingTree`, `Index`, `EmptyTree`,
`Point name`, or `Tag name`.
Selecting HEAD or a tag still pins its resolved commit ID. If HEAD advances
or the tag no longer resolves to that ID, the label falls back to the short
commit ID rather than pretending the endpoint moved.
Both labels use the menu's muted-blue accent and hover background. Clicking
the base or target label opens that endpoint's picker.

**Review** contains the unchecked **Reviews** command, Recently resolved,
Archived threads, Archive resolved threads, Clear board, filters, and
contextual thread actions. **Reviews** opens and focuses the normal view; it
is not a shown/hidden toggle. **Diff**
contains Comparison controls, base/target pickers, Save review point, and
whitespace. **Start comparison at current HEAD** remains inside Comparison
controls.

**Layout** is a persistent top-level menu immediately before **Go**. Its
first section switches between the mutually exclusive **File view** and
**Reviews view**; the active row carries a bold `▌` marker, and one is always
active. A separator precedes the independent checked **Sidebar**, **Files
pane**, and **Threads pane** toggles. On a narrow terminal, Layout, Go,
Review, Diff, and Help collapse under `☰`.

Configuration is KDL at
`$XDG_CONFIG_HOME/fathomable/config.kdl` (normally
`~/.config/fathomable/config.kdl`). Print the complete effective file with:

```sh
fathomable --config-show
```

The current shape is:

```kdl
theme "default-dark"

jump {
    toast 4000
}

watch {
    ignore
    debounce 300
}

markdown {
    extensions "md" "markdown" "mdx"
    names "readme" "license" "licence" "copying" "changelog" "contributing" "authors" "notice"
}

viewer {
    max-file-size-mib 64
}

layout {
    menu-bar #true
    sidebar {
        visible #true
        files #true
        threads #true
        width 32
        split 8
    }
}

threads {
    stubs #true
    stubs-resolved #false
}

diff {
    context 3
    ignore-whitespace #false
}

user {
    name "User"
}
```

Theme overrides live in `$XDG_CONFIG_HOME/fathomable/themes/`. Use
`--theme NAME` for one run.

## 7. Connect an agent

Start a repository-bound stdio MCP server:

```sh
fathomable --mcp
fathomable --mcp /path/to/checkout
```

The binding never follows the human viewer to another worktree. Viewer and
MCP processes are independent; both may run, or MCP may operate headlessly.

Register the command in the agent host. Examples:

```sh
claude mcp add --scope user fathomable -- fathomable --mcp
codex mcp add fathomable -- fathomable --mcp
```

Copilot CLI configuration:

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

The complete MCP surface is:

| Tool | Use |
| --- | --- |
| `threads` | read non-archived board discussions; filter with `status`, `path`, `since`, `after`, and `limit`, or use `ids` alone for exact archived/history reads |
| `thread_start` | start one or more discussions with `comments: [{path, line?, end_line?, body, idempotency_key?}]`; each fresh body is at most 1024 UTF-8 bytes |
| `thread_reply` | continue discussions with `replies: [{thread, body, resolve?, line?, end_line?, idempotency_key?}]`; each fresh body is at most 1024 UTF-8 bytes |

Normal `status: "open"`, `"resolved"`, and `"all"` reads are not gated by
the bound checkout's ancestry. They exclude archived history. Exact `ids`
can retrieve archived records and mark `archived: true`.

Results separate immutable `origin` from `placement_evidence`, current
`placement`/`location`, `resolution_history`, `archive_history`, and
`restore_history`. Agent starts capture the bound checkout's working-tree
facts; callers do not send viewer, comparison, review-point, task, or
membership IDs.

Write batches are prevalidated, including the 1024-byte fresh-body limit.
Per-item `idempotency_key` values provide durable caller-scoped retry safety.
A matching historical keyed write may replay a larger body; a fresh write
with that body still fails. A fresh reply to an archived thread fails
explicitly. A matching successful keyed reply replay returns its original
resolution outcome and current archived thread without duplicating or
restoring it.

Agents are instructed to treat these discussions like pull-request review
threads, not chat responses. Each thread should hold one local, actionable
issue with only essential evidence and a concise requested action or outcome.
Whole files, whole sections, long replacements, broad reviews, plans, and
status reports belong in the worktree. Independent issues get separate
threads; splitting one message merely to evade the body limit is not allowed.

Supported write identity comes from the host's native channel:

| Host | Channel |
| --- | --- |
| Copilot CLI | `COPILOT_AGENT_SESSION_ID` at MCP launch |
| Claude Code | `CLAUDE_CODE_SESSION_ID` at MCP launch |
| VS Code | `params._meta["vscode.conversationId"]` |
| Codex | `params._meta.sessionId` and `params._meta.threadId` |

Reads do not require identity. Writes fail rather than inventing identity.

## 8. State and diagnostics

Useful commands:

```sh
fathomable --doctor
fathomable --viewers
fathomable --config-show
```

State defaults under `$XDG_STATE_HOME/fathomable`:

```text
workspaces/<repository-hash>/threads.jsonl
workspaces/<repository-hash>/review-points/
workspaces/<checkout-hash>/comparison/comparison.json
viewers/
log/
```

The repository hash is based on the Git common directory, so linked
worktrees share threads and review points. Comparison preferences use the
checkout root identity. Plain directories use their root identity. A
working-file rename is only a projection in the active checkout and is
discarded when the viewer switches worktrees or restarts.

Use `scripts/demo-repo.sh` to create an isolated disposable repository and
XDG state directory for experimentation:

```sh
FATHOMABLE=target/debug/fathomable scripts/demo-repo.sh
```

The script prints the repository path and required XDG exports.
