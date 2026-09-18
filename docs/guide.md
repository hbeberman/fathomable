---
type: Guide
title: Setup guide
description: Everyday UX, KDL configuration, agent setup, and where Fathomable saves state.
tags:
  - onboarding
---

# Setup guide

Fathomable is a read-only terminal viewer for reviewing agent-driven work.
It never edits your checkout or Git data. Install it on Linux using the
[README](../README.md#install); contributor tooling is separate in
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Start

```sh
fathomable                   # Current workspace
fathomable path/to/checkout  # Another workspace
fathomable README.md         # Open a file in its workspace
```

In Git, the workspace is the enclosing repository or linked worktree.
Outside Git, a directory is its own workspace. `--name review` labels a
viewer; it does not change MCP routing.

## UX

**Files** browses the workspace; **Threads** lists discussions beside the
document. The main column switches between **File** (`f`) and **Reviews**
(`t`). Click pane titles for filters and view options, right-click rows for
actions, or use the top **Layout**, **Go**, **Review**, and **Diff** menus.
The mouse wheel scrolls; dragging the sidebar divider resizes it.

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
| `Space v s` | source / rendered view |
| `Space w w` | focus the next pane |
| `f` `t` | File / Reviews |
| `]f` `[f` | next / previous queued live change |
| `]g` `[g` | next / previous comparison hunk |
| `]w` `[w` | next / previous worktree |
| `c` `Space c f` | line comment or reply / file comment |
| `r` `R` | resolve or reopen / toggle one-shot auto-resolve |
| `z` `Z` | fold one thread / all threads |
| `Enter` `Ctrl-Enter` | submit draft / submit with auto-resolve enabled |
| `Alt-Enter` `Ctrl-e` | draft newline / edit with `$VISUAL` or `$EDITOR` |
| `Esc` | cancel transient input or return to File |
| `Space ?` `:q` | keymap / quit |

### Compare versions

The base and target at the right of the menu bar apply to every file.
Click either label, or use `Space d b` / `Space d t`, to choose a commit,
tag, branch commit, index (staged content), or working tree (files on disk).
These choices never fetch, check out, or modify Git.

A fresh Git workspace compares a pinned `HEAD` to the working tree.
Committing does **not** advance that base: use **Diff > Comparison
controls... > Start comparison at current HEAD** when ready. The Files pane,
gutters, counts, and `:diff` all follow the selected comparison.

`Space d c` saves an optionally named **review point** without changing Git.
Choose it under the base picker's **Review points...** to see changes since
that save. Saving a point does not select it automatically. See
[comparison and review-point details](decisions/0087-global-comparisons-and-board-history.md).

### Review discussions

Select lines and press `c`, or use `Space c f` for a file-wide comment.
Threads retain their original excerpt even if edits move or detach them.
Keep comments focused; each new message is limited to 1024 UTF-8 bytes.

The lifecycle marks are **● active**, **◐ resolution proposed**, and
**○ resolved**. You resolve or reopen with `r`. Enabling `R` gives the
agent one chance to resolve: its next reply consumes that permission.
Without permission, an agent's completion request is a proposal for you to
review.

The board is shared across the repository's worktrees and survives commits
and branch changes. **Review** offers **Recently resolved**, **Archived
threads**, and **Clear board...**. Clearing archives the shared board after
confirmation; it does not delete history. Restore an archived entry with
`u`. Nothing archives automatically.

## Configuration

Configuration uses **KDL** at `$XDG_CONFIG_HOME/fathomable/config.kdl`
(default `~/.config/fathomable/config.kdl`). No file is required: omitted
settings use defaults, and unknown settings are errors.

KDL uses named nodes, quoted strings, numbers, `#true` / `#false` booleans,
`{ ... }` child blocks, and `//` comments. Separate nodes with newlines or
semicolons. This is the complete default configuration:

```kdl
theme "default-dark" // Built-in theme or a custom theme name from themes/.

jump {
    toast 4000 // Toast duration in milliseconds; 0 disables toasts.
}

watch {
    ignore // Extra root-relative globs excluded from live-change notifications.
    debounce 300 // Quiet period in milliseconds before grouping filesystem changes.
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
        files #true // Include the Files pane in the sidebar.
        threads #true // Include the Threads pane in the sidebar.
        width 32 // Sidebar columns, capped at one third of the terminal.
        split 8 // Threads pane rows when both sidebar panes are shown.
    }
}

threads {
    stubs #true // Show inline thread summaries.
    stubs-resolved #false // Include resolved threads in inline summaries.
}

diff {
    context 3 // Unchanged lines shown around each diff hunk.
    ignore-whitespace #false // Default only; saved comparisons keep their whitespace rule.
}

user {
    name "User" // Your non-empty display name for review comments.
}
```

`fathomable --config-show` prints the effective configuration and its path.
Custom themes live in `$XDG_CONFIG_HOME/fathomable/themes/`; `--theme NAME`
overrides the theme for one run.

## Connect an agent

Register `fathomable --mcp` as a stdio MCP server in your agent host.
It binds to the launch workspace; add `/path/to/checkout` to bind explicitly.
It can run without a viewer and never follows the viewer to another worktree.

```sh
claude mcp add --scope user fathomable -- fathomable --mcp
codex mcp add fathomable -- fathomable --mcp
```

For Copilot CLI, add this server to its MCP configuration:

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

## What gets saved where

Product state lives under `$XDG_STATE_HOME/fathomable`
(default `~/.local/state/fathomable`), not in your checkout:

| Relative path | Contents |
| --- | --- |
| `workspaces/<repository-hash>/threads.jsonl` | Discussion messages, origin excerpts, placement, and lifecycle history. |
| `workspaces/<repository-hash>/review-points/` | Explicitly saved manifests and content blobs. |
| `workspaces/<checkout-hash>/comparison/comparison.json` | Last-used comparison preference for that checkout. |
| `viewers/` | Viewer registrations. |
| `log/` | Diagnostic logs. |

IPC sockets live separately under
`$XDG_RUNTIME_DIR/fathomable/<workspace-hash>/<pid>.sock`. External-editor
drafts use `fathomable-comment-<pid>.md` in the system temporary directory
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
migrated or deleted. For paths and connection diagnostics, run
`fathomable --doctor`; `fathomable --viewers` lists running viewers.
