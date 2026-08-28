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
fathomable --name review # a second window on the same workspace, named for agents
```

A file argument opens the workspace *and* shows that file. Several
Fathomables may show one workspace; they share its threads, and an agent
can drive all of them or one by name (`--name`, or `:name` later). The viewer
re-reads a file when it changes on disk and keeps your position, so leave
it open next to an editor or an agent.

The workspace is live too. A file the agent creates, deletes, or renames
shows up in, leaves, or moves within the tree on its own (within
`follow.hint-debounce`), so `R` is only for a listing you suspect is
stale. If the file you are reading is deleted, the text stays put under a
`deleted` banner and the pill reads `DELETED`: you can still scroll,
search, and read its threads, but `c`, `C`, and replies are refused until
the file comes back, at which point it reloads and the banner goes. If it
is renamed, the view follows with your position and threads intact, the
threads move to the new path in the store, and the status line says
`renamed to NEW`.

Markdown files (`.md`, `.markdown`, `.mdx`, and well-known extensionless
prose such as `README` and `LICENSE`) open rendered; every other file,
`justfile` and `Makefile` included, opens as syntax-highlighted source,
coloured by its extension or, without one, its file name. Fenced code blocks inside Markdown are
coloured by their info string (` ```rust `). A language syntect does not
bundle (TOML, KDL, Dockerfile among them) shows plain. `gs` still flips any
file between the two views.

A binary file — one git would diff as binary: `-diff` or `binary` in
`.gitattributes`, else a `NUL` in its first 8000 bytes — opens as a
**file-info pane** instead: the format its magic number gives away
(WebAssembly, PNG, ELF, gzip, …), its size, mode, and modification time,
and its git state with the `HEAD` size beside the size on disk. A text
file over `viewer.max-file-size-mib` (64 MiB by default) gets the same
pane with a notice saying which config line raises the limit. Neither can
be annotated: `c` says so.

## 3. Keys

Vim-style movement in the view; `Space` opens a Helix-style menu. `Space ?`
shows the full list inside the app.

| Keys | Action |
| --- | --- |
| `j` `k` `h` `l`, `gg`, `ge` / `G`, `Ctrl-d` `Ctrl-u` | move, top, bottom, half page |
| `h` at column 0 | focus the tree (a selection wraps to the line above instead) |
| `zl` `zh`, `zL` `zH`, horizontal wheel | scroll long lines sideways by a column (a count applies: `10zl`), by half the pane, by four columns |
| `/` `?`, `n` `N`, `:noh` | search, next match (scrolling sideways to show it on a long line), clear highlight |
| `:N` | go to source line N |
| `gs` | toggle raw source view |
| `gd` / `:diff`, `gD` / `:diff seen` | toggle the diff against `HEAD`; against last seen |
| `]g` `[g`, `]G` `[G` | next / previous hunk, crossing into the next uncommitted file; next / previous uncommitted file |
| `]f` `[f`, `Space j` | next / previous changed file; follow menu: jump, auto, clear |
| `:follow`, `:status` | toggle auto-jump; viewer and path overlay |
| `:name NAME` | name this viewer so an agent can target it; `:name` alone clears it |
| `v` / `V` / `x` or mouse drag, then `y` / `c` | select text / lines (`x` grows a line per press), then copy or comment |
| `c` with nothing selected | open the thread on the cursor line, or comment on it when there is none |
| `C` | always start a new thread, on the selection or the cursor line |
| `Space a`, `]c` `[c` | thread at cursor, next/previous thread |
| `]r` `[r` | next / previous thread waiting on you, crossing into the next file and opening its pane |
| thread pane `r` `x` `n` `p` `j` `k`, `Esc` | reply, resolve or reopen, next / previous thread in the file (the cursor follows), scroll; close |
| `Space t` | focus the file-threads pane under the tree: this file's threads, open and resolved, the one under the cursor highlighted |
| file threads `j` `k` `Enter` `r` `x`, `Esc` | next / previous thread (the cursor follows), open its pane, reply, resolve or reopen; back to the text |
| `Space A` | the thread list: every thread on this work in place of the document, open then resolved, grouped by file |
| list `j` `k` `gg` `ge` `G` `Ctrl-d` `Ctrl-u`, `Enter` | move between threads; open the file at the thread and its pane |
| list `r` `x` `z` `Z` `f`, `Esc` | reply, resolve or reopen, fold the entry, fold resolved, only this file; back to the document |
| comment box `Enter`, `Ctrl-Enter` / `Alt-Enter`, `Esc`, `Ctrl-c` | newline, submit, cancel (twice on a draft), clear the draft (empty closes) |
| comment box arrows, `Home` `End` `Ctrl-a`, `Alt-b` `Alt-f` | move by character or line, line start / end, word |
| comment box `Ctrl-w` `Ctrl-u` `Ctrl-k`, `Delete` | delete word back, to line start, to line end, forward |
| comment box paste, click, `PgUp` `PgDn` / `Alt-Up` `Alt-Down` | insert at the cursor, place the cursor, scroll the thread |
| comment box `Ctrl-e` | edit the draft in `$VISUAL` / `$EDITOR` |
| `Space e`, `Space E` | tree: open and focus or return focus; hide |
| tree `j` `k` `h` `l` `Enter`, `R`, `I` | move (the highlighted file is shown), collapse, expand or open and focus; re-read (new, deleted, and renamed files already show on their own); show ignored |
| `Space f` / `Space F`, `Space o` | file picker (ignored files too), recent files; `Ctrl-n` `Ctrl-p` move |
| `[o` `]o` | previous / next opened file |
| `Esc`, `:q` | close or clear; quit |

Copy uses OSC 52, so it lands in the system clipboard through most
terminals and multiplexers.

Prose wraps to the pane; code block lines, source lines, and diff lines
never do. A line cut off at the right edge ends in a dim `›`; `zl` scrolls
every such line sideways together (the gutter, wrapped prose, and a diff
sign stay put), and a line cut on the left then starts with a dim `‹`.
The offset is per file, survives reloads and `gs`/`gd`, and `:status`
reports it as `col N`. Copying a selection always yields the whole
source line, not the visible slice.

The mouse works on whichever pane it is over: the wheel scrolls the text
or the thread pane under the pointer — over the tree it steps one row per
tick, showing each file it lands on — and a click focuses the pane. A
click in the tree stays in the tree: it expands a directory or shows a
file like the wheel does, and only `Enter` moves focus to the view.
Drag the tree's divider, the thread pane's top rule, or the file-threads
pane's top rule to resize them.
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

While the tree is shown, a file with threads gets a **file-threads pane**
under it: one row per thread in line order — range, `●` open or `✓`
resolved in the gutter colour, the first line of the comment, and the
reply count and age at the edge. The highlighted row is the thread under
the cursor, so reading the file walks the pane; `Space t` or a click on
its header focuses it, a click on a row opens that thread, and `j`/`k`,
`Enter`, `r`, and `x` act on the highlight.

`Space A` shows the whole review at once: every thread on the current
work (the ones whose commit `HEAD` can reach), open ones first and then
resolved ones dimmed, grouped by file, each with its comment and replies
in full. It takes the text column the way a document does; the tree
stays beside it. `Enter` opens the file at the selected thread with its
pane, `r` and `x` reply and resolve in place, `f` narrows the list to the
file you were reading, and `Esc` goes back to it.

A thread is **waiting** on you when it is open and an agent wrote its
newest message; your reply, resolve, or reopen ends the wait. Waiting
threads have their own colour (`annotation.waiting`) in the gutter
bracket, the file-threads pane, and the thread list, the status line
counts them (`2 waiting`), the tree tags their files `↩`, and a reply
landing while you read raises a toast (`reply on src/lib.rs:42`). `]r`
and `[r` step through them — this file first, then the others in path
order, wrapping — and open each one's pane, so holding `]r` reads every
reply that needs an answer.

## 5. Changes against git

Inside a git work tree the bar between the line numbers and the text shows
what differs from `HEAD`: a green bar for added lines, orange for changed
ones, and a thin red rule along the top of the line that follows a removal
(the removed text itself is only shown in the diff view). Annotation marks
sit at the far left of the gutter: a thread's rows are bracketed `╭`, `│`,
`╰`, a thread on one row is `•`, and a thread nested inside another
re-draws the corners on the outer one's line.
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
counts after the name (`bin` for a binary file, which has no lines to
count); a collapsed folder shows the most advanced letter
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
once its target is on screen. A file the agent says it is editing
(`follow`) is revealed in the tree, its folders expanded, without moving
your highlight unless the tree has focus.

`Space j a` (or `:follow`) turns on **auto-jump**: the pill reads `AUTO`
and the viewer opens the newest change by itself once writes have been
quiet for a second, preferring a file the agent said it is editing
(`follow`). It is a monitor, not a leash: it waits while you are
selecting, writing a comment, reading a thread or diff, have a popup
open, or have touched the keyboard or mouse in the last three seconds,
and it switches itself off — toast `auto-jump off`, pill cleared — the
moment you go somewhere else: another file, a thread, the thread list,
the diff view, a selection. Scrolling and searching in the file it
landed on keep it on. When the change is in the file you are reading it
only scrolls if the hunk is off screen. `[o` takes you back; `Space j a`
turns it on again.

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
    names "README" "LICENSE" "LICENCE" "COPYING" "CHANGELOG" \
          "CONTRIBUTING" "AUTHORS" "NOTICE"   // extensionless prose
}

viewer {
    max-file-size-mib 64    // larger text files show the file-info pane
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

A *session* is a workspace's annotation state; every running Fathomable is
a *viewer* of one. A viewer writes a record under
`$XDG_STATE_HOME/fathomable/sessions/`, marks its workspace in
`$XDG_STATE_HOME/fathomable/workspaces/<hash>/workspace.json`, and listens
on `$XDG_RUNTIME_DIR/fathomable/<hash>/<pid>.sock`. `fathomable --mcp` is a
stdio MCP server that, on every call, picks the known workspace containing
the current directory and drives its viewers; reading and answering threads
also works with no viewer running, straight from the store.

Register it with your agent host once. For Claude Code:

```sh
claude mcp add fathomable -- fathomable --mcp
```

For any other host, add a stdio server whose command is
`fathomable --mcp`. Then start Fathomable in the repository, start the agent
in the same repository, and the tools are:

| Tool | Use |
| --- | --- |
| `session_list`, `session_switch` | see known workspaces and their viewers; pin one when the cwd heuristic is wrong |
| `open` | show a file in every viewer, or in the one named by `viewer`, optionally at a line or line range |
| `follow` | tell the viewer(s) which files the agent is editing (shown as `follow N` in the status line and listed in `:status`) |
| `annotations_list` | read the threads on the current work, optionally `since` a Unix time or on one `path`; works without a viewer |
| `thread_reply` | answer a thread, optionally resolving it; a `persona` name is recorded next to the client name; works without a viewer |

Every tool accepts an optional `session`: a workspace root, or a viewer name
or id. A thread belongs to the commit it was written against and is shown
(here and in the viewer) only while that commit is `HEAD` or one of its
ancestors, so switching to unrelated work hides it and merging brings it
along ([0024](decisions/0024-workspace-sessions.md)). Details and the wire
protocol are in [0014](decisions/0014-mcp-server-and-socket-v1.md).

## 9. When something is off

```sh
fathomable --doctor        # terminal, directories, config, log locations
fathomable --sessions      # known workspaces and their viewer records
fathomable --config-show   # effective configuration
```

Each run logs JSON lines to `$XDG_STATE_HOME/fathomable/log/<session-id>.log`;
`:status` inside the app shows the open document, the terminal size, the
viewer name and id, the socket, and every state path. Set
`FATHOMABLE_LOG=debug` for more. A viewer killed without a clean quit is
swept away by the next start.

If Fathomable dies, it hands the terminal back and prints one block between
two rules: what it was showing, where its state lives, and a backtrace with
the runtime plumbing dropped. Paste that block at your agent. A copy is
written to `$XDG_STATE_HOME/fathomable/log/<session-id>.crash`, so a report
that has scrolled away is still there; `--doctor` counts what is waiting
there ([0022](decisions/0022-crash-reports.md)).
