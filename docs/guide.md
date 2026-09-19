---
type: Guide
title: Setup guide
description: Everyday UX, KDL configuration, agent setup, and where Fathomable saves state.
related_resources:
  - scripts/perf-record.sh
tags:
  - onboarding
---

# Setup guide

Fathomable is a read-only terminal viewer for reviewing agent-driven work.
It never edits your checkout or Git data. Install it on Linux using the
[README](../README.md#install); contributor tooling is separate in
[CONTRIBUTING.md](../CONTRIBUTING.md).

**Enthusiast alpha:** features may appear, change, or disappear at any time.
There is no cross-upgrade persistence guarantee for Fathomable stores,
including annotations and review points. Treat this state as disposable
between versions and keep important conclusions outside the app. See the
[alpha contract](decisions/0083-single-user-alpha-clean-slate.md#enthusiast-alpha-contract).

## Start

```sh
fathomable                   # Current workspace
fathomable path/to/checkout  # Another workspace
fathomable README.md         # Open a file in its workspace
```

In Git, the workspace is the enclosing repository or linked worktree.
Outside Git, a directory is its own workspace.

## UX

**File list** browses the workspace; **Thread list** lists discussions beside
the document. The main column switches between **File** (`f`) and **Threads**
(`t`). `F` and `T` show and focus the corresponding sidebar list. Click pane
titles for their existing filters and view options, right-click rows for
actions, or use the top **Layout**, **Go**, **Review**, and **Diff** menus.
Plain header space and content can focus a pane without changing the title's
menu target. `Alt-Space` opens and focuses the top-left Fathomable menu,
revealing the app bar first if it is hidden. Hover never focuses.

The **File list** title menu has four session filters: **only changed**,
**only reviews**, **hide untracked**, and **show ignored**. `Space F c`,
`Space F o`, `Space F u`, and `Space F g` toggle them from any pane. Only
reviews means files with an active or resolution-proposed, non-archived thread
in the current workspace. Filters combine, and the File list header names active
filters compactly as `c`, `r`, `u`, and `i`.

The focused pane marks its name with a purple `▏`; filenames, counts, filters,
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

In File list, `z` folds or unfolds the selected directory without opening a
file. `Z` unfolds every directory admitted by the current filters, or folds
them all when already expanded. Recursive unfolding skips directory symlinks.
Previewing a file keeps its draft parked; explicitly opening File resumes it.

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
| `h` `j` `k` `l` | move left, down, up, right (arrow keys also work) |
| `gg` `G` | top / bottom |
| `Ctrl-d` `Ctrl-u` | half page down / up |
| `/` `n` `N` | search, next match, previous match |
| `v` `V` `y` | select characters, select lines, copy |
| `gf` | follow a file reference or URL |
| `Space f` | file picker |
| `Space F c` `Space F o` | only changed / only reviews in File list |
| `Space F u` `Space F g` | hide untracked / show ignored in File list |
| `Space d s` `Space d u` `Space d o` | Standard / Unified / Off diff mode |
| `Space d d` | compare the current HEAD to the working tree |
| `Space d w` | ignore whitespace |
| `Space d c` `Space d x` | save / delete a review point |
| `Space v s` | source / rendered view for configured Markdown files |
| `Space v t` `Space v r` | toggle thread stubs / resolved stubs |
| `w` `W` | focus the next / previous displayed pane |
| `f` `F` | show and focus File / File list |
| `t` `T` | show and focus Threads / Thread list |
| `Alt-Space` | open and focus the top-left Fathomable menu |
| `J` `K` | next / previous comparison change across the workspace |
| `Tab` `Shift-Tab` | next / previous open review thread across the workspace |
| `]w` `[w` | next / previous worktree |
| `c` `Space c f` | line comment or reply / file comment |
| `r` `R` | resolve or reopen / toggle one-shot auto-resolve |
| `z` `Z` | fold/unfold a directory / all directories in File list; threads elsewhere |
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
| `:quit` (`:q`, `:q!`, `:quit!`) | quit immediately without confirmation |
| `:status` | show live viewer, workspace, and storage status |
| `:<line>` | jump to a source line; numeric jumps are not completion candidates |

All current named commands take no arguments.

`Ctrl-e` uses a fresh owner-only temporary directory containing an owner-only
draft file. Editor replacements and backups placed beside the draft stay
private, and Fathomable removes this scratch directory on return, including
error returns. A forced termination may leave private scratch files behind.
Unsafe temporary-directory ancestors are refused before the draft is written.
Files the editor is configured to write elsewhere are outside this protection.

### Compare versions

One session-global diff mode applies to every file and review/history view.
The rightmost control in each File or Threads header reads **Diff: standard**,
**Diff: unified**, or **Diff: off**. Click it for the three mode choices, or
use `Space d s`, `Space d u`, and `Space d o`:

- **Standard** shows Target content with comparison gutters, counts, File list
  filtering, and hunk navigation.
- **Unified** shows the selected Base-to-Target patch. It follows file
  switches and remains active when you press `Esc`.
- **Off** is Target-only source browsing. It shows no Base content or
  Base-only paths and suppresses comparison, Git-status, and file-edit toasts
  and counts. Plain activity notifications remain visible.

Off retains the selected Base, the whitespace setting, the **only changed**
File list filter, and the last active Standard/Unified mode so they return when
diffs are enabled. Their controls are dormant while Off. Changing Target keeps
Off active; choosing Base attempts to restore the last active mode. If that
pair cannot be read, both endpoint choices remain selected and the viewer stays
Off with an error. A Base-only file that was already open says **not present in
Target** and shows no Base body. Threads still shows clearly labelled immutable
origin excerpts and stored discussion history because those are review
evidence, not source browsing.

In Standard and Unified, the Base and Target at the right of the menu bar apply
to every file. Off shows only the clickable Target. Click an endpoint, or use
`Space d b` / `Space d t`, to choose a commit, tag, branch commit, index
(staged content), or working tree (files on disk). Review points are Base-only.
These choices never fetch, check out, or modify Git. A pending new-line or
new-file comment must be submitted or cancelled before changing mode, Base, or
Target.

A fresh Git workspace compares a pinned `HEAD` to the working tree. Committing
does **not** advance that Base; choose the new commit explicitly. The Diff menu
begins with Standard, Unified, and Off, then Base, Target, and **Head to
WorkingTree**. That action, or `Space d d`, pins the current `HEAD` as Base and
selects the working tree as Target. Save/Delete review point follow a
separator, and Ignore whitespace follows another. There is no comparison-control popup,
**Start comparison at current HEAD**, or `:diff`.

Rendered/Source is available in Standard and Off only for files matched by
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

`Space d c` saves an optionally named **review point** without changing Git.
Choose it under the base picker's **Review points...** to see changes since
that save. Saving a point does not select it automatically. See
[comparison and review-point details](decisions/0087-global-comparisons-and-board-history.md).

`Space d x` or **Diff > Delete review point...** opens the repository-wide
point list. Selecting a point opens a separate confirmation; press `y` to
delete or `Esc` to cancel. Deletion removes the point from comparison
selection and reclaims only content blobs no other point uses. Threads keep
their immutable point ID, baseline, content identity, excerpt, and messages.
This is logical deletion with best-effort reclamation, not secure erasure.
If the deleted point is the selected Base, Fathomable replaces Base with the
current pinned `HEAD` (or EmptyTree) while preserving Target, diff mode, and
whitespace. A pending new annotation must be submitted or cancelled first.

Use `J` and `K` to cycle every comparison change across the workspace.
Text hunks are individual stops; a changed path with no text hunk, such as a
binary or mode-only change, is one stop. The cycle follows the selected
comparison and is unavailable in Off mode. File list expands and centers a
listed destination without taking keyboard focus; a hidden File list catches
up when shown.

### Review discussions

Select lines and press `c`, or use `Space c f` for a file-wide comment.
Threads retain their original excerpt even if edits move or detach them.
Keep comments focused; each new message is limited to 1024 UTF-8 bytes.

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
This navigation never switches worktrees: use `]w` and `[w` explicitly.

The File footer keeps the core loop visible as
`comment c · diffs K/J · threads Shift-Tab/Tab`, omitting unavailable
actions. On a thread row, `comment c` becomes `reply c`; where both fold
actions apply, the footer uses `folding z/Z`.

The board is shared across the repository's worktrees and survives commits
and branch changes. **Review** offers **Recently resolved**, **Archived
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
    mode "standard" // Startup presentation: "standard", "unified", or "off".
    context 3 // Unchanged lines shown around each diff hunk.
    ignore-whitespace #false // Default only; saved comparisons keep their whitespace rule.
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
| `threads` | Read discussions without changing state; exact IDs can retrieve archived history. |
| `thread_start` | Start a batch of focused review comments. |
| `thread_reply` | Reply to discussions, optionally requesting resolution. |

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

MCP source reads stay within the bound checkout. Relative symlinks within
that checkout work; absolute symlink targets and links escaping it (including
directory links) fail
explicitly. This also applies when an existing thread's file becomes an
escaping symlink before a read or relocation.

## What gets saved where

Product state lives under `$XDG_STATE_HOME/fathomable`
(default `~/.local/state/fathomable`), not in your checkout:

| Relative path | Contents |
| --- | --- |
| `workspaces/<repository-hash>/threads.jsonl` | Discussion messages, origin excerpts, placement, and lifecycle history. |
| `workspaces/<repository-hash>/review-points/` | Explicitly saved manifests and content blobs. |
| `workspaces/<checkout-hash>/comparison/comparison.json` | Last-used comparison preference for that checkout. |
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

The Git common directory identifies the repository, so linked worktrees
share threads and review points but keep separate comparison preferences.
Plain directories use their root identity. Running viewers do not control
one another's comparison; the last successful preference write is restored
on a later launch.

Reading does not save full-file snapshots. Review points explicitly save
eligible changed content and rely on Git objects for unchanged committed
files; they are not standalone backups. Outside Git, points capture all
eligible files.

After upgrading, restart viewers and the agent host's MCP connection
together. Incompatible annotation stores are refused, not automatically
migrated or deleted. State persists across ordinary restarts of a compatible
build, but an upgrade may require an explicit operator reset. Stop affected
processes before backing up or resetting app-owned state and review the
actual XDG paths; never remove repository files as part of a reset. A backup
may need the older build to remain readable. Configuration is preserved by
default, but obsolete settings may need manual changes.

For paths and connection diagnostics, run
`fathomable --doctor`; `fathomable --viewers` lists running viewers.

## Contributor performance profiles

`just perf path/to/file.md` runs the local Linux CPU profiler described in
[CONTRIBUTING.md](../CONTRIBUTING.md#2-the-gate). It uses a separate optimized
frame-pointer build and keeps the exact sampled executable with owner-only
artifacts under Cargo's `target/perf/` directory. These artifacts can contain
source paths and terminal content; inspect them before sharing. Caller stacks
from an older capture cannot be repaired after recording.
