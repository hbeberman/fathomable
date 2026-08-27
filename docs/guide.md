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

Markdown files (`.md`, `.markdown`, `.mdx`, and extensionless files such as
`README`) open rendered; every other file opens as syntax-highlighted
source, coloured by its extension. Fenced code blocks inside Markdown are
coloured by their info string (` ```rust `). A language syntect does not
bundle (TOML, KDL, Dockerfile among them) shows plain. `gs` still flips any
file between the two views.

## 3. Keys

Vim-style movement in the view; `Space` opens a Helix-style menu. `Space ?`
shows the full list inside the app.

| Keys | Action |
| --- | --- |
| `j` `k` `h` `l`, `gg`, `G`, `Ctrl-d` `Ctrl-u` | move, top, bottom, half page |
| `/` `?`, `n` `N`, `:noh` | search, next match, clear highlight |
| `:N` | go to source line N |
| `gs` | toggle raw source view |
| `gd` / `:diff`, `]c` `[c` | diff view: last-seen, then `HEAD`, then off; next / previous hunk |
| `]f` `[f`, `Space j` | next / previous changed file; follow menu: jump, auto, source, clear |
| `:follow`, `:follow source S` | toggle auto-jump; `workspace`, `followed`, or `open-only` |
| `v` / `V` or mouse drag, then `y` / `c` | select text / lines, then copy or comment |
| `x` | select the current line; repeat to extend down |
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

Select with `v`, `V`, or the mouse and press `c`. The comment becomes a
thread anchored to the content, so it follows the lines when text above
them changes and shows as *detached* when the lines are gone. Threads live
outside the repository at
`$XDG_STATE_HOME/fathomable/workspaces/<hash>/threads.jsonl`
(`~/.local/state/...` by default), one append-only JSON line per event.

## 5. Changes against git

Inside a git work tree the bar between the line numbers and the text shows
what differs from `HEAD`: a green bar for added lines, orange for changed
ones, and a thin red rule along the top of the line that follows a removal
(the removed text itself is only shown in the diff view). Annotation marks
sit at the far left of the gutter.
`]c` and `[c` walk the hunks, `gd` swaps the pane for a unified diff of
the file, and the status line counts `+added -removed` lines.

There are two bases. **Last seen** is the file as it was when you last
looked at it: Fathomable snapshots a file when you switch away, quit, or
leave it alone for five seconds, so the bar shows what changed since then.
**HEAD** is the last commit. `gd` shows the last-seen diff first (`DIFF
seen`), `gd` again the `HEAD` diff (`DIFF head`), and a third `gd` returns
to the rendered view; whichever base the diff view used last is the one the
bar and `]c` use. A file that has never been seen uses `HEAD`. Snapshots
live under `~/.local/state/fathomable/workspaces/<hash>/seen/` and can be
deleted at any time; a commit refreshes the `HEAD` base immediately.

## 6. Following an agent

Any file written under the workspace (ignoring what git ignores) becomes a
*change*: a `●` next to it in the tree (and on collapsed folders above it),
a toast in the bottom-right corner for a few seconds, and a hint in the
status line, `→ src/foo.rs +12 -3 (3)`, naming the newest change and how
many are pending. `]f` and `[f` step through the changed files, newest
first; `Space j j` jumps straight to the newest. A jump lands on the first
hunk against the last-seen base (or the range an agent passed to `open`),
and a change is forgotten once its target is on screen.

`Space j a` (or `:follow`) turns on **auto-jump**: the pill reads `AUTO`
and the viewer opens the newest change by itself once writes have been
quiet for a second. It never jumps while you are selecting, writing a
comment, reading a thread or diff, have a popup open, or have touched the
keyboard or mouse in the last three seconds; `[o` takes you back. `Space j
s` cycles what counts as a change: everything in the workspace, only the
files an agent named with `follow`, or only agent `open` calls.

## 7. Configuration and themes

Configuration is optional KDL at `$XDG_CONFIG_HOME/fathomable/config.kdl`
(`~/.config/fathomable/config.kdl`):

```kdl
theme "default-light"

follow {
    source "workspace"      // workspace | followed | open-only
    auto #false             // start with auto-jump on
    ignore "target/**"      // extra globs on top of .gitignore
    hint-debounce 300       // ms of quiet before a write becomes a change
    jump-debounce 1000      // ms of quiet before auto-jump moves
    seen-idle 5000          // ms alone with a file before it counts as seen
    toast 4000              // ms a toast stays; 0 disables toasts
}

markdown {
    extensions "md" "markdown" "mdx"   // files rendered as Markdown
    extensionless #true                // README, LICENSE, ... too
}
```

Every key is optional; the values above are the defaults and
`--config-show` prints the effective ones.

Built-in themes are `default-dark` and `default-light`. Drop your own at
`~/.config/fathomable/themes/<name>.kdl`; it can `inherits` a built-in and
override only the keys it wants. `--theme NAME` selects one for a single
run; `--config PATH` points at another config file. The theme file shape
and the key vocabulary are in
[0011](decisions/0011-theme-schema.md). A theme's `code.syntect` picks one
of syntect's bundled themes for code colours (`--doctor` lists them); only
their foreground colours are used, so a transparent background stays
transparent ([0016](decisions/0016-syntax-highlighting.md)).

## 8. Connect an agent

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
| `follow` | tell the viewer which files the agent is editing (shown as `follow N` in the status line; with `source "followed"` only these files raise change hints) |
| `annotations_list` | read threads, optionally `since` a Unix time or on one `path` |
| `thread_reply` | answer a thread, optionally resolving it; a `persona` name is recorded next to the client name |

Every tool accepts an optional `session` id. Details and the wire protocol
are in [0014](decisions/0014-mcp-server-and-socket-v1.md).

## 9. When something is off

```sh
fathomable --doctor        # terminal, directories, config, log locations
fathomable --sessions      # live and dead session records
fathomable --config-show   # effective configuration
```

Each run logs JSON lines to `$XDG_STATE_HOME/fathomable/log/<session-id>.log`;
the session id is at the right end of the status line. Set
`FATHOMABLE_LOG=debug` for more. A session killed without a clean quit is
swept away by the next start.
