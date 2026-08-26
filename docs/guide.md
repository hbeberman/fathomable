---
type: Guide
title: Setup guide
description: Install Fathomable, open a workspace, learn the keys, configure themes, and connect an agent over MCP.
tags:
  - onboarding
---

# Setup guide

The human-facing walkthrough: from a fresh checkout to a session that an
agent can drive. Keep it current whenever a flag, key, file path, or tool
name changes; the [decisions](decisions/index.md) hold the reasoning, this
page holds what to type. Linux only for now (see the
[charter](charter.md)).

## 1. Install

Requires a stable Rust toolchain (1.88 or newer).

```sh
git clone <this repository> fathomable
cd fathomable
make install          # cargo install --path crates/fathomable --locked
fathomable --version
```

Contributors also want `scripts/setup-build-deps.sh` and
`just install-commit-hooks`; see the [README](../README.md) for the gate
workflow.

## 2. Open something

```sh
fathomable README.md     # single file
fathomable               # workspace rooted at the enclosing git root, or cwd
fathomable path/to/dir   # workspace rooted there
```

A file argument opens the workspace *and* shows that file. The viewer
re-reads a file when it changes on disk and keeps your position, so leave
it open next to an editor or an agent.

## 3. Keys

Vim-style movement in the view; `Space` opens a Helix-style menu. `Space ?`
shows the full list inside the app.

| Keys | Action |
| --- | --- |
| `j` `k` `h` `l`, `gg`, `G`, `Ctrl-d` `Ctrl-u` | move, top, bottom, half page |
| `/` `?`, `n` `N`, `:noh` | search, next match, clear highlight |
| `:N` | go to source line N |
| `gs` | toggle raw source view |
| `gd` / `:diff`, `]c` `[c` | toggle unified diff against `HEAD`; next / previous change |
| `V` or mouse drag, then `y` / `c` | select lines, then copy or comment |
| `Space a`, `Space A`, `]a` `[a` | thread at cursor, pick a thread, next/previous thread |
| thread panel `r` `x` `n` `p` `j` `k` | reply, resolve or reopen, switch, scroll |
| comment box `Enter`, `Ctrl-Enter` / `Alt-Enter` | newline, submit |
| `Space e` / `Ctrl-b`, `Space E` | tree: open and focus or return focus; hide |
| tree `j` `k` `h` `l` `Enter`, `R`, `I` | move, collapse, expand or open; re-read; show ignored |
| `Space f` / `Space F`, `Space o` | file picker (ignored files too), recent files |
| `[o` `]o` | previous / next opened file |
| `Esc`, `:q` | close or clear; quit |

Copy uses OSC 52, so it lands in the system clipboard through most
terminals and multiplexers.

## 4. Annotations

Select lines with `V` or the mouse and press `c`. The comment becomes a
thread anchored to the content, so it follows the lines when text above
them changes and shows as *detached* when the lines are gone. Threads live
outside the repository at
`$XDG_STATE_HOME/fathomable/workspaces/<hash>/threads.jsonl`
(`~/.local/state/...` by default), one append-only JSON line per event.

## 5. Changes against git

Inside a git work tree the gutter bar shows what differs from `HEAD`: green
for added lines, orange for changed ones, red on the line after a removal.
`]c` and `[c` walk the changes, `gd` swaps the pane for a unified diff of
the file (`gd` again returns), and the status line counts `+added -removed`
lines. The base is re-read when you open, switch to, or the agent rewrites
a file, so a fresh commit shows at the next change.

## 6. Configuration and themes

Configuration is optional KDL at `$XDG_CONFIG_HOME/fathomable/config.kdl`
(`~/.config/fathomable/config.kdl`):

```kdl
theme "default-light"
```

Built-in themes are `default-dark` and `default-light`. Drop your own at
`~/.config/fathomable/themes/<name>.kdl`; it can `inherits` a built-in and
override only the keys it wants. `--theme NAME` selects one for a single
run; `--config PATH` points at another config file. The theme file shape
and the key vocabulary are in
[0011](decisions/0011-theme-schema.md).

## 7. Connect an agent

Every running Fathomable is a *session*: it writes a record under
`$XDG_STATE_HOME/fathomable/sessions/` and listens on a Unix socket in
`$XDG_RUNTIME_DIR/fathomable/`. `fathomable --mcp` is a stdio MCP server
that binds to the session whose workspace contains the current directory
and forwards tool calls to it.

Register it with your agent host once. For Claude Code:

```sh
claude mcp add fathomable -- fathomable --mcp
```

For any other host, add a stdio server whose command is
`fathomable --mcp`. Then start Fathomable in the repository, start the agent
in the same repository, and the tools are:

| Tool | Use |
| --- | --- |
| `session_list`, `session_switch` | see running sessions; pick one when the cwd heuristic is wrong |
| `open` | show a file, optionally at a line or line range |
| `follow` | tell the viewer which files the agent is editing (shown as `follow N` in the status line) |
| `annotations_list` | read threads, optionally `since` a Unix time or on one `path` |
| `thread_reply` | answer a thread, optionally resolving it; a `persona` name is recorded next to the client name |

Every tool accepts an optional `session` id. Details and the wire protocol
are in [0014](decisions/0014-mcp-server-and-socket-v1.md).

## 8. When something is off

```sh
fathomable --doctor        # terminal, directories, config, log locations
fathomable --sessions      # live and dead session records
fathomable --config-show   # effective configuration
```

Each run logs JSON lines to `$XDG_STATE_HOME/fathomable/log/<session-id>.log`;
the session id is at the right end of the status line. Set
`FATHOMABLE_LOG=debug` for more. A session killed without a clean quit is
swept away by the next start.
