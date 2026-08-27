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
`README`, though not dotfiles like `.gitignore`) open rendered; every other file opens as syntax-highlighted
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
| `h` at column 0 | focus the tree (a selection wraps to the line above instead) |
| `/` `?`, `n` `N`, `:noh` | search, next match, clear highlight |
| `:N` | go to source line N |
| `gs` | toggle raw source view |
| `gd` / `:diff`, `gD` / `:diff seen` | toggle the diff against `HEAD`; against last seen |
| `]g` `[g`, `]G` `[G` | next / previous hunk, crossing into the next uncommitted file; next / previous uncommitted file |
| `]f` `[f`, `Space j` | next / previous changed file; follow menu: jump, auto, clear |
| `:follow`, `:status` | toggle auto-jump; session and path overlay |
| `v` / `V` or mouse drag, then `y` / `c` | select text / lines, then copy or comment |
| `Space a`, `Space A`, `]c` `[c` | thread at cursor, pick a thread, next/previous thread |
| thread pane `r` `x` `n` `p` `j` `k`, `Esc` | reply, resolve or reopen, switch, scroll; close |
| comment box `Enter`, `Ctrl-Enter` / `Alt-Enter`, `Esc` | newline, submit, cancel (twice on a draft) |
| comment box arrows, `Home` `End` `Ctrl-a`, `Alt-b` `Alt-f` | move by character or line, line start / end, word |
| comment box `Ctrl-w` `Ctrl-u` `Ctrl-k`, `Delete` | delete word back, to line start, to line end, forward |
| comment box paste, click, `PgUp` `PgDn` / `Alt-Up` `Alt-Down` | insert at the cursor, place the cursor, scroll the thread |
| comment box `Ctrl-e` | edit the draft in `$VISUAL` / `$EDITOR` |
| `Space e`, `Space E` | tree: open and focus or return focus; hide |
| tree `j` `k` `h` `l` `Enter`, `R`, `I` | move, collapse, expand or open; re-read; show ignored |
| `Space f` / `Space F`, `Space o` | file picker (ignored files too), recent files; `Ctrl-n` `Ctrl-p` move |
| `[o` `]o` | previous / next opened file |
| `Esc`, `:q` | close or clear; quit |

Copy uses OSC 52, so it lands in the system clipboard through most
terminals and multiplexers.

The mouse works on whichever pane it is over: the wheel scrolls the tree,
the text, or the thread pane under the pointer, and a click focuses it.
Drag the tree's divider or the thread pane's top rule to resize them.
Starting on a directory opens the tree; starting on a file opens the file.

## 4. Annotations

Select with `v`, `V`, or the mouse and press `c`. The comment becomes a
thread anchored to the content, so it follows the lines when text above
them changes, moves onto the rewritten lines and shows as *edited* when an
agent changes the lines themselves (until you reply or resolve), and shows
as *detached* when the lines are gone. Edits made while Fathomable was not
running are followed too, on the next start, through the file's last-seen
snapshot (section 5); commenting snapshots the file so there is always
one. Threads live
outside the repository at
`$XDG_STATE_HOME/fathomable/workspaces/<hash>/threads.jsonl`
(`~/.local/state/...` by default), one append-only JSON line per event.

## 5. Changes against git

Inside a git work tree the bar between the line numbers and the text shows
what differs from `HEAD`: a green bar for added lines, orange for changed
ones, and a thin red rule along the top of the line that follows a removal
(the removed text itself is only shown in the diff view). Annotation marks
sit at the far left of the gutter.
The bar is thin (`▎`) for a change not yet in the index and thick (`▌`)
for one that is staged; a new untracked file is all thin green. `gd` swaps
the pane for a unified diff of the file against `HEAD` (`DIFF`), and the
status line counts `+added -removed` lines.

`]g` and `[g` walk the hunks, and when a file's hunks run out they carry
on into the next uncommitted file in path order, wrapping at the end, so
holding `]g` from the top of the tree visits every uncommitted change.
`]G` and `[G` step by file instead, landing on the first hunk. The tree
shows every uncommitted file with a letter in its gutter column,
`M`odified, `A`dded, `D`eleted, or `?` untracked, in one colour when the
change is staged and another when it is not, and its `+added -removed`
counts after the name; a collapsed folder shows the most advanced letter
and the summed counts of everything beneath it, and the root header shows
the repo's totals (`demo +12 -3`). A save, `git add`, or commit updates all
of this within a beat.

There is a second base. **Last seen** is the file as it was when you last
looked at it: Fathomable snapshots a file when you switch away, quit,
comment on it, or leave it alone for five seconds. It never drives the bar
or `]g`; `gD` (or `:diff seen`) shows it as `DIFF seen`, and `gD` again
returns to the rendered view. Snapshots live under
`~/.local/state/fathomable/workspaces/<hash>/seen/` and can be deleted at
any time; they expire after thirty days unless the file has an open
thread.

## 6. Following an agent

Any file written under the workspace (ignoring what git ignores) becomes a
*change*: a `●` next to it in the tree (and on collapsed folders above it),
a toast in the bottom-right corner for a few seconds, and a hint in the
status line, `→ src/foo.rs +12 -3 (3)`, naming the newest change and how
many are pending. `]f` and `[f` step through the changed files, newest
first; `Space j j` jumps straight to the newest. A jump lands on the first
hunk against `HEAD` (or the range an agent passed to `open`; outside git,
the first hunk against the last-seen snapshot), and a change is forgotten
once its target is on screen.

`Space j a` (or `:follow`) turns on **auto-jump**: the pill reads `AUTO`
and the viewer opens the newest change by itself once writes have been
quiet for a second. It never jumps while you are selecting, writing a
comment, reading a thread or diff, have a popup open, or have touched the
keyboard or mouse in the last three seconds; `[o` takes you back.

## 7. Configuration and themes

Configuration is optional KDL at `$XDG_CONFIG_HOME/fathomable/config.kdl`
(`~/.config/fathomable/config.kdl`):

```kdl
theme "default-light"

follow {
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
| `follow` | tell the viewer which files the agent is editing (shown as `follow N` in the status line and listed in `:status`) |
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
`:status` inside the app shows the open document, the terminal size, the
session id, the socket, and every state path. Set
`FATHOMABLE_LOG=debug` for more. A session killed without a clean quit is
swept away by the next start.

If Fathomable dies, it hands the terminal back and prints one block between
two rules: what it was showing, where its state lives, and a backtrace with
the runtime plumbing dropped. Paste that block at your agent. A copy is
written to `$XDG_STATE_HOME/fathomable/log/<session-id>.crash`, so a report
that has scrolled away is still there; `--doctor` counts what is waiting
there ([0022](decisions/0022-crash-reports.md)).
