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

Requires a stable Rust toolchain (1.88 or newer) and Linux: whether a
viewer is alive and which harness session a headless `--mcp` serves are
both read from `/proc`, so on another OS every viewer looks dead and
the tools ask for the session `id` every time.

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

The workspace is live too. A file or directory the agent creates,
deletes, or renames shows up in, leaves, or moves within the tree on its
own (within `watch.debounce`) — a whole new directory arrives
collapsed, and is listed when you expand it — so `R` is only for a
listing you suspect is stale. If the file you are reading is deleted,
the text stays put under a `deleted` banner: you can still scroll,
search, and read its threads, but `c`,
`C`, and replies are refused until the file comes back, at which point it
reloads and the banner goes. If it is renamed, the view follows with
your position and threads intact, the threads move to the new path in the
store, and the status line says `renamed to NEW`.

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

Vim-style movement in the text; `Space` opens a Helix-style menu, and any
prefix (`g`, `[`, `]`, `Space`, `d`) shows the keys that continue it under
a row naming the prefix (`Space c · threads`).
`Space ?` lists every binding inside the app. Every key below is checked
against the binding table by a test, so what is written here exists.

Text:

| Keys | Action |
| --- | --- |
| `j` `k` `h` `l`, arrows | move; `h` at the first column wraps onto the end of the row above and `l` at the last onto the start of the row below |
| `0` `$`, `Home` `End` | line start / end |
| `gg`, `ge` / `G` | top / bottom |
| `Ctrl-d` `Ctrl-u` | half page down / up |
| `/` `?`, `n` `N`, `:noh` | search, next / previous match, clear highlight |
| `:N` | go to source line N |
| `gs` / `:source` | toggle raw source view |
| `gd` / `:diff`, `gD` / `:diff seen` | the diff against `HEAD` (`HEAD · now`); against last seen; the same key again, or `Esc`, closes it |
| `h` `l`, `b` `t`, `w` in a diff | earlier / later pair along the file's checkpoint timeline; pick the base / the target from its checkpoints, the working file, last seen, `HEAD`, and the commits that touched it; ignore whitespace |
| `]g` `[g`, `]G` `[G` | next / previous hunk, crossing into the next uncommitted file; next / previous uncommitted file |
| `]f` `[f` | next / previous changed file |
| `Alt-Left` `Alt-Right` | back / forward through the jumplist: the positions far moves leave behind (another file by any route, `gf`, a search jump, `gg` / `G`, `:N`, `]c`, `]g`); `j` `k`, paging, and the mouse leave nothing |
| `v` / `V` / `x` or mouse drag, then `y` / `c` | select text / lines (`x` grows a line per press), then copy or comment; `y` with nothing selected copies the cursor line |
| `gy` `gx` | copy the link under the cursor; open it with `xdg-open` |
| `gf`, Ctrl-click | open the file named under the cursor in the viewer, at its line: a Markdown link to a path, or a bare `path:line`, `path:line:col`, or `path#L12`, read against the file's directory and then the root; `Alt-Left` returns |
| `c` with nothing selected | expand the thread at the cursor in place, or comment on the line when there is none; on an expanded thread, fold it and expand the next thread covering the lines, until none is |
| `r` `e` `o`, `dd` | reply to the thread here, edit the message here when yours, resolve or reopen, delete (on an expanded thread's rows, or the thread at the cursor) |
| `C` | always start a new thread, on the selection or the cursor line |
| `]c` `[c`, `]C` `[C` | next / previous thread in the file; across the workspace, opening its file |
| `]r` `[r`, `Tab` `Shift-Tab` | next / previous thread waiting on you, crossing into the next file, expanded where it lands |
| `:auto [on\|off]`, `:status`, `:name NAME` | toggle or set auto-jump; viewer and path popup; name this viewer so an agent can target it (`:name` alone clears it) |
| `Esc`, `:q` | clear the input, prefix, selection, or highlight, else leave the diff; quit |

The `Space` menu, from any pane:

| Keys | Action |
| --- | --- |
| `Space f` | file picker |
| `Space F i`, `Space F r` | files: the picker including ignored files; the recent files |
| `Space r` | the review list: every thread on the work in place of the document, newest agent reply first, resolved hidden; on the focused list, close it |
| `Space w h`, `Space w l` | window: the pane left of the text (the files pane, or the threads pane when the files pane is hidden; the files pane is shown when neither is); back to the text |
| `Space w j`, `Space w k` | window: from the files pane down to the threads pane, and back up, when both are shown |
| `Space w w`, `Space Space` | the next pane: text, files pane, threads pane, text, skipping a hidden pane |
| `Space w f`, `Space w t` | window: the files pane, the threads pane, from any pane; a hidden one is shown first |
| `Space p f`, `Space p t` | panes: hide the files pane or the threads pane, or show it again without taking the keys (the other pane keeps the sidebar) |
| `Space c c`, `Space c r`, `Space c o`, `Space c e`, `Space c d` | threads, on the thread at the cursor from any pane: new thread, reply, resolve or reopen, edit your newest message, delete |
| `Space v s`, `Space v t`, `Space v x` | view: toggle source view (as `gs`), thread stubs, stubs for resolved threads (hidden by default) |
| `Space d d`, `Space d D` | diff: the diff against `HEAD`, against last seen (as `gd` `gD`) |
| `Space d r`, `Space d g` | diff: the checkpoint diff, opened on the latest checkpoint against the working file; pick a commit to diff against the working file |
| `Space d b`, `Space d t` | diff: pick the base, the target (as `b` `t` in a diff; from outside one, against the working file) |
| `Space d c`, `Space d C` | diff: checkpoint this file; checkpoint the workspace, every non-ignored text file whose content moved since its last checkpoint (a toast counts them) |
| `Space d w` | diff: ignore whitespace (as `w` in a diff) |
| `Space j j`, `Space j a` | jump to the newest change, toggle auto-jump |
| `Space a w` | agent: wake a subscribed agent with its pending threads through `agents.wake` (a picker when several are subscribed) |
| `Space ?` | all keys |
| `:` | the command line, from any pane |

Threads pane (the sidebar's lower pane; its header reads `threads · file 3`
or `threads · workspace 12`):

| Keys | Action |
| --- | --- |
| `j` `k` | next / previous thread, wrapping; the text follows, and in workspace scope the file opens |
| `Enter` / `l` / `Right` | open the file with the thread expanded, the keys going to the text |
| `s`, `x` | list this file or the workspace; show or hide resolved threads (the review list shares the flag) |
| `r` `o`, `dd` | reply, resolve or reopen, delete |
| `Esc` | back to the text; the pane stays (`Space p t` hides and shows it) |

Review list (`Space r`; its header reads `review  4 open · 1 proposed
· resolved hidden` with the sort word at its right edge, the proposal
count only while there is one; the keys below sit on a bar along the
list's bottom row, and the header, the bar, and each entry's header
draw on the `ui.header` surface every pane header shares):

| Keys | Action |
| --- | --- |
| `j` `k` / `Down` `Up` | next / previous thread, its newest message highlighted |
| `l` `h` / `Right` `Left` | next / previous message in the thread |
| `gg` `ge` `G` | first / last thread |
| `Ctrl-d` `Ctrl-u` | half a page of rows |
| `Enter` | open the file with the thread expanded and the cursor on the highlighted message |
| `r` `e` `o`, `dd` | reply, edit your highlighted message, resolve or reopen, delete |
| `s`, `x`, `f`, `z` | sort by newest agent reply or by file and line; show or hide resolved threads (the threads pane shares the flag); only this file; fold the entry |
| `Esc` | close the list, back to the document (`Space r` does too) |

The draft, a comment, reply, or edit written in the thread's rows:

| Keys | Action |
| --- | --- |
| `Enter` | submit, or save an edit |
| `Alt-Enter` / `Ctrl-Enter` | newline |
| arrows, `Home` `End` `Ctrl-a`, `Alt-b` `Alt-f` | move by character or line, line start / end, word |
| `Ctrl-w` `Ctrl-u` `Ctrl-k`, `Delete` | delete word back, to line start, to line end, forward |
| paste, click, `Alt-j` `Alt-k` / `Alt-Down` `Alt-Up` | insert at the cursor, place the cursor, scroll the text around the draft |
| `Ctrl-e` | edit the draft in `$VISUAL` / `$EDITOR` |
| `Ctrl-c` | clear the draft (empty closes) |
| `Esc` | cancel (twice after a change) |

Files pane and picker:

| Keys | Action |
| --- | --- |
| `j` `k` `h` `l` `Enter` | move (the highlighted file is shown), collapse, expand or open and focus |
| `gg` `ge` `G` | top / bottom |
| `y` | copy the highlighted entry's path, relative to the root |
| `Esc` | back to the text; the pane stays |
| picker `Ctrl-j` `Ctrl-k` / arrows, `Enter`, `Esc` | move, open, close |

Copy uses OSC 52, so it lands in the system clipboard through most
terminals and multiplexers.

All long lines wrap to the pane. Prose wraps at words; code blocks,
source files, diffs, and very narrow tables wrap between displayed
characters. Wrapped diff continuations align under a blank sign cell.
Copying a selection still yields the original source text.

The mouse works on whichever pane it is over: the wheel scrolls the pane
under the pointer — over the tree it steps one row per tick, showing
each file it lands on — and a click focuses the pane. A click in the
tree stays in the tree: it expands a directory or shows a file like the
wheel does, and only `Enter` moves focus to the view. A click on a stub
expands its thread. Drag the sidebar's divider or the threads pane's rule
to resize them.

A **right-click** opens a menu of what the pointer is on, each entry
showing the key that does the same: on a selection, comment, new
thread, copy, and clear; on a line a thread covers, expand or fold,
reply, resolve or reopen, edit, and delete; on a link, copy or open it,
and on a link or a path that names a file, open it in the viewer;
on any line, comment, select, and copy. In the files pane it offers open,
checkpoint, and copy path; on a threads pane or review
list entry, go to, reply, resolve, edit, and delete. A right-click
outside the selection moves the cursor there first; inside it keeps the
selection. Hover highlights an entry; a click or its key runs it; `Esc`
or a click elsewhere closes the menu. The `delete thread` entry deletes
at once. The `Space` menu and `Space ?` take clicks too, as do the key
hints at the right edge of the threads pane, checkpoint, expanded
thread, and draft author rows and on the review list's key bar; a click
on the threads pane header toggles its reach, on the review list's sort
word switches the sort, and on the checkpoint header's base or target
name opens that picker.

Selecting with the mouse: drag over text for a character selection; a
press or drag in the gutter selects whole lines; a double-click selects
the word and a triple-click the line; Shift-click extends the
selection to the pointer where the terminal passes Shift through (most
keep it for their own selection). Every gesture ends in `SEL` mode, so
`y`, `c`, `C`, and the right-click menu apply. A Ctrl-click places the
cursor and runs `gf` there. The right button and the Ctrl modifier reach
the viewer only where the terminal forwards them under mouse capture
(Ghostty, kitty, foot, WezTerm, and Alacritty do).
Starting on a directory opens the tree; starting on a file opens the file.

## 4. Threads

Select with `v`, `V`, or the mouse and press `c`. The comment becomes a
thread anchored to the content (an agent can start one too, through
`thread_start` in section 8, and its comment then carries the agent's
name and waits on you), so it follows the lines when text above
them changes, moves onto the rewritten lines and shows as *edited* when an
agent changes the lines themselves (until you reply or resolve), and shows
as *detached* when the lines are gone: a blank row then appears where
the lines were, carrying the thread's mark, and the lines now at that
place are left alone. `c` on that row opens the thread; `C` is refused,
as the row is not text. The colour of a mark is the thread's status
alone: amber while open (`thread.open`), a bold cool colour when it
waits on you (`thread.waiting`), grey once resolved
(`thread.resolved`); *edited* and *detached* are words in an expanded
thread's header, not colours. The lines of the thread the cursor is on
are tinted in a cool colour (blue in the dark theme, teal in the light
one), distinct from the tint of other annotated lines (`thread.focus`).
Comment and reply bodies in an expanded thread render as Markdown:
lists, emphasis, `inline code`, and fenced blocks coloured by their
language, with a newline kept as a line break as in a GitHub comment;
the text in `threads.jsonl` is the source you typed. Where the lines
went and what state the thread is in are separate: an expanded thread's
header reads `detached · resolved` or `edited · waiting`, placement
first, and a thread at its own lines shows the state alone. A third
word, `proposed`, follows when an agent's newest reply proposes
resolving the thread (`waiting · proposed`): only you resolve, so the
thread stays open and waiting until your `o` accepts the proposal or
your reply keeps it going. Edits made while Fathomable was not
running are followed too, on the next start, through the file's last-seen
snapshot (section 5); commenting snapshots the file so there is always
one. A file too large to snapshot, or whose snapshot was deleted, is
followed through the thread itself: each thread keeps its lines and
three lines either side, and an edit that stays inside that window is
found. Threads live
outside the repository at
`$XDG_STATE_HOME/fathomable/workspaces/<hash>/threads.jsonl`
(`~/.local/state/...` by default), one append-only JSON line per event.
Each line carries the format version this build writes; a file of
another version is refused with the line to blame and the path to
delete, and there is no migration before the first tag
([0062](decisions/0062-one-version-no-compatibility.md)).

Every open thread shows a **stub** under the last of its lines: one row
per message for its newest two, each with the state glyph, the author
(`user`, or an agent as `name (type)`), the age, and the first line of
the message, on a tinted background (`thread.inline`; a theme that sets
it to `none` gets a `▎` at the left edge instead). Stubs are not lines:
`j`/`k`, paging, search, `:N`, and selection step over them, a click on
one lands on the row above, and they carry no line number. Stubs of
threads stacked on one row follow one another in line order. The stub
of the thread under the cursor reads in the text colour and, for the
thread the cursor is on, ends with `(c expand)`; the others are dimmed.
`Space v x` gives resolved threads a stub too, and `Space v t` hides
stubs altogether; `threads { stubs; stubs-resolved }` sets both
defaults.

`c` on a line a thread covers **expands** its stub in place, the view
staying still: a header row with the state, placement, watchers, and
the keys, then every message rendered as Markdown. Its message rows are
cursor rows: `j`/`k` walk the messages, `r` replies and puts the cursor
on the reply, `e` edits the message under the cursor when you wrote it,
`o` resolves or reopens, `dd` deletes the thread, and `c` on any of its
rows folds it. When several threads cover the line, `c` again folds the
expanded one and expands the next in line order, wrapping, and after the
last leaves none expanded. A click on a stub expands it.

Writing happens in the same rows: the **draft** is not a box along the
bottom but rows of the text. A reply is written at the end of its
thread's expanded rows, under a ` user  draft` row that carries the
draft keys at its right edge (or `Esc again to discard` once you have
pressed Esc on a changed draft); an edit replaces the message it edits,
seeded with its text; and a new comment (`c` on a line with no thread,
`C`, or `Space c c`) gets a block of its own under its lines, headed
`comment on L3-5`. The draft wraps at the text width and grows with
what you type, the view scrolling just enough to keep its cursor on
screen while the text cursor stays on the message; a click in the draft
places its cursor. Submit, cancel, or clear an empty draft and the rows
go: a reply becomes the newest message under the cursor, a new comment
becomes a stub.

The left column is the **sidebar**: the **files pane** above the **threads
pane**, each shown or hidden on its own (`Space p f`, `Space p t`),
the sidebar drawn while either is. The threads pane lists this file's
threads in line order or, after `s`, the whole workspace's by file and
line, resolved ones hidden until `x` shows them: `●` open or `✓`
resolved in the gutter colour, `L3-5` or `guide.md:3`, the first line of
the newest message, `↩n` when replied, and the age at the edge. Beside
the files pane it keeps `sidebar.split` rows (8 by default; drag its rule to
change that for the session), and alone it takes the whole column. The
highlighted row is the thread under the cursor, so reading the file
walks the pane; `j`/`k` step the cursor and the text follows, another
file opening in workspace scope, `Enter` opens the file with the thread
expanded, and `r` and `o` act on the highlight.

The text, the threads pane, and the review list show one **thread
cursor**: a thread and a message in it. Whichever surface you move it
from, the others follow, and `r`, `e`, `o`, and `dd` act on it wherever
the keys came from. While the list is open the cursor is what you last
stepped to or clicked; once it is closed and you move in the text, the
cursor rides the text cursor again: the thread starting on the cursor
line, else the first on its row, else the nearest above, at its newest
message, or the message under the cursor on an expanded thread's rows.
Thread motions follow one rule: lowercase steps within the file,
uppercase crosses files: `]c`/`[c` step to the previous or next thread
of this file, wrapping, and `]C`/`[C` across the workspace, files in
path order, opening the file they land in.

`Space r` shows the whole review at once as an inbox: every thread on
the current work (the ones whose commit `HEAD` can reach), the ones an
agent spoke in last at the top, newest first, then the rest by their
newest message; `s` sorts by file and line instead. Resolved threads
are hidden until `x` shows them dimmed (the threads pane shares the
flag). Every entry header carries the path, the lines, the state, and
the age, then the comment and replies in full. It takes the text column
the way a document does; the sidebar stays beside it. The newest message
in the selected thread starts highlighted; `j`/`k` move between
threads, `l`/`h` move between their messages, `Ctrl-d`/`Ctrl-u` move by
half a page of rows, `z` folds an entry, and `e` edits a highlighted
message you wrote. `Enter` opens the file with the thread expanded and
the cursor on that message, `o` resolves in place, `r` and `e` open the
file the way `Enter` does to write the reply or edit in the thread's
rows and bring the list back when the draft closes, `f` narrows the
list to the file you were reading, and `Esc` goes back to it.

A thread is **waiting** on you when it is open and an agent has the
last word on it; your reply, edit, resolve, or reopen ends the wait,
and until then no agent is woken for it
([0058](decisions/0058-the-user-has-the-last-word.md)). Waiting
threads have their own colour (`thread.waiting`) in the gutter
bracket, the threads pane, and the review list, the status line
counts them (`2 waiting`, and `1 proposed` before it while a thread on
the document carries a proposal; `:status` has both totals), the tree
tags their files `↩`, and a reply landing while you read raises a toast
(`reply on src/lib.rs:42`, or `reply on src/lib.rs:42, proposes
resolving` when the agent proposed closing it). `]r`
and `[r`, or `Tab` and `Shift-Tab`, step through them — this file
first, then the others in path order, wrapping — and expand each one,
so holding `Tab` reads every reply that needs an answer.

## 5. Changes against git

Inside a git work tree the bar between the line numbers and the text shows
what differs from `HEAD`: a green bar for added lines, orange for changed
ones, and a thin red rule along the top of the line that follows a removal
(the removed text itself is only shown in the diff view). Thread marks
sit at the far left of the gutter: a thread's rows are bracketed `╭`, `│`,
`╰`, a thread on one row is `•`, and a thread nested inside another
re-draws the corners on the outer one's line. The rows are the rendered
ones, so a thread on a long markdown paragraph is bracketed across the rows
it wraps to, and the blank rows between paragraphs inside a thread draw `│`.
The bar is thin (`▎`) for a change not yet in the index and thick (`▌`)
for one that is staged; a new untracked file is all thin green. `gd` swaps
the pane for the **diff view** with `HEAD` as its base: a unified diff of
the file, a header naming the two sides (`HEAD · now`) with the diff keys
at its right, the badge `DIFF HEAD` after the path, and `+added -removed`
counts in the status line. `gd` again, or `Esc` once there is nothing
else to clear, returns to the file.

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
or `]g`; `gD` (or `:diff seen`) shows it in the diff view as `last seen ·
now`, badge `DIFF seen`, and `gD` again returns to the file. Snapshots
live under
`~/.local/state/fathomable/workspaces/<hash>/seen/` and can be deleted at
any time; they expire after thirty days unless the file has an open
thread.

A **checkpoint** is a mark you make yourself. `Space d c` records the
current file's content on that file's timeline; `Space d C` records every
non-ignored text file whose content moved since its last checkpoint, in
one go, and a toast counts them (`checkpoint: 3 files`; a file the agent
has not touched gets no new entry). Checkpoints live beside the snapshots
under `~/.local/state/fathomable/workspaces/<hash>/checkpoints/`, one blob
per distinct content plus an `index.jsonl` of events; nothing expires, and
`--doctor` counts them.

`Space d r` opens the diff view on the file's newest checkpoint pair,
the latest checkpoint against the working file. Its header reads
`checkpoint 2/3  5m ago · now` and the badge `DIFF cp 2/3`; `h` and `l`
page to the earlier or later pair along the timeline, and, while the
file has a checkpoint, a strip along the bottom lists them, `◆` on the
workspace-wide ones. With no checkpoint the view says so and names
`Space d c`; `Space d r` again leaves it.

Every diff is the same view with two **sides**, so its keys work in all
of them. `b` and `t` pick the base or the target from the file's
checkpoints, the working file, last seen, `HEAD`, and the commits that
touched it (the fifty most recent reachable from `HEAD`); the header
names the pair (`a1b2c3d · HEAD`) and the badge the base (`DIFF
a1b2c3d`). From outside a diff they open one against the working file;
`Space d g` is the shortcut for a commit against the working file. `w`
(or `Space d w`) ignores whitespace, so lines that differ only in
spacing count as unchanged; the header says `· whitespace ignored`
while it does, and `diff { ignore-whitespace }` sets the start. The
gutter bar, `]g`, and the file counts always compare against `HEAD`
exactly: checkpoints and snapshots sit beside git, they never replace
it ([0049](decisions/0049-inline-threads-and-the-rail.md),
[0060](decisions/0060-one-diff-two-sides.md)).

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

`Space j a` (or `:auto`) turns on **auto-jump**: an `AUTO` badge follows the path in the status line
and the viewer opens the newest change by itself once writes have been
quiet for a second, preferring a file the agent said it is editing
(`follow`). It is a monitor, not a leash: it waits while you are
selecting, writing a comment, reading a thread or diff, have a popup
open, or have touched the keyboard or mouse in the last three seconds,
and it switches itself off — toast `auto-jump off`, badge gone — the
moment you go somewhere else: another file, a thread, the review list,
the diff view, a selection. Scrolling and searching in the file it
landed on keep it on. When the change is in the file you are reading it
only scrolls if the hunk is off screen. `Alt-Left` takes you back;
`Space j a` turns it on again.

## 7. Configuration and themes

Configuration is optional KDL at `$XDG_CONFIG_HOME/fathomable/config.kdl`
(`~/.config/fathomable/config.kdl`):

```kdl
theme "default-light"

jump {
    auto #false             // start with auto-jump off; `Space j a` toggles it
    debounce 1000           // ms of quiet before auto-jump moves
    toast 4000              // ms a toast stays; 0 disables toasts
}

watch {
    ignore "target/**"      // extra globs on top of .gitignore
    debounce 300            // ms of quiet before a write becomes a change
}

markdown {
    extensions "md" "markdown" "mdx"   // files rendered as Markdown
    names "README" "LICENSE" "LICENCE" "COPYING" "CHANGELOG" \
          "CONTRIBUTING" "AUTHORS" "NOTICE"   // extensionless prose
}

viewer {
    max-file-size-mib 64    // larger text files show the file-info pane
    seen-idle 5000          // ms alone with a file before it counts as seen
}

sidebar {
    width 32                // columns for the files and threads panes
    split 8                 // rows the threads pane keeps under the files pane
}

threads {
    stubs #true             // a stub under each thread's lines
    stubs-resolved #false   // resolved threads get one too
}

diff {
    context 3               // unchanged lines shown around each hunk
    ignore-whitespace #false // start with whitespace ignored (Space d w)
}

agents {
    types "coder" "reviewer" "planner"   // what an agent may subscribe as
    nag-after 5             // stop-hook checks between reminders; 0 never
    expire-after 24         // hours a silent subscription lives
    max-lines 40            // longest hook prompt before the rest is listed
    wake ""                 // command for Space a w, e.g. "claude -r {id} {prompt}"
}

user {
    name "User"             // how your messages are signed, to you and to agents
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

A workspace's threads are the workspace's; every running Fathomable is
a *viewer* of one. A viewer writes a record under
`$XDG_STATE_HOME/fathomable/viewers/`, marks its workspace in
`$XDG_STATE_HOME/fathomable/workspaces/<hash>/workspace.json`, and listens
on `$XDG_RUNTIME_DIR/fathomable/<hash>/<pid>.sock`. `fathomable --mcp` is a
stdio MCP server that, on every call, picks the known workspace containing
the current directory and drives its viewers; reading and answering threads
also works with no viewer running, straight from the store.

Register it with your agent host once. Every host runs the same stdio
command, `fathomable --mcp`; only the file it is written to differs.
Register it per user, not in the repository, so a clone does not opt
anyone in.

Claude Code:

```sh
claude mcp add fathomable -- fathomable --mcp
```

Codex CLI, which writes `~/.codex/config.toml`:

```sh
codex mcp add fathomable -- fathomable --mcp
```

The equivalent block, if you would rather edit the file:

```toml
[mcp_servers.fathomable]
command = "fathomable"
args = ["--mcp"]
```

Copilot CLI, in `~/.copilot/mcp-config.json` — `/mcp add` in a session
fills in the same file, and `COPILOT_HOME` moves the directory:

```json
{
  "mcpServers": {
    "fathomable": {
      "type": "local",
      "command": "fathomable",
      "args": ["--mcp"],
      "tools": ["*"]
    }
  }
}
```

`"stdio"` is accepted there as a synonym for `"local"`; `tools` takes
`"*"` or the names to expose.

VS Code, in the user `mcp.json` that the **MCP: Open User Configuration**
command opens, or in `.vscode/mcp.json` for a single workspace
(**MCP: Add Server** writes either):

```json
{
  "servers": {
    "fathomable": {
      "type": "stdio",
      "command": "fathomable",
      "args": ["--mcp"]
    }
  }
}
```

Any other host takes the same stdio command. Each host spawns
`fathomable` itself, so it has to be on the PATH that host sees: a VS
Code started from a desktop launcher may not have `~/.cargo/bin` on it,
and wants the absolute path instead.

Then start Fathomable in the repository, start the agent in the same
repository, and the tools are:

| Tool | Use |
| --- | --- |
| `workspaces` | see known workspaces and their viewers; `switch` (a root, or a viewer name or id) pins one for the connection when the cwd heuristic is wrong |
| `open` | show a file in every viewer, or in the one named by `viewer`, optionally at a line or line range; the range is scrolled into view with the cursor on its first line, not selected |
| `follow` | subscribe the session to the whole workspace with `type` (one of the configured `agents.types`, which the tool's schema lists as an enum) and `id` (the session id from the `hello` hook, optional when the session is known from the harness), so the hooks hand it every thread the user has the last word on, as its turns start and end; the session is named here, once: a `persona` when given, else its harness — Claude, Copilot, Codex — else the client string ([0058](decisions/0058-the-user-has-the-last-word.md)); `end` ends the subscription, forgetting its deliveries and watches; works without a viewer |
| `threads` | read the threads the checkout shows, oldest change first: `status` is `open` (the default), `pending` (open, and the user has the last word), `resolved`, or `all`; `path` a file, or a directory for the whole subtree (fails, naming same-named paths, when it is neither); `since` a Unix time and `limit` (50) page, with a note on how; each thread comes with its placement — *anchored*, *edited*, or *detached* — and its current range, and no anchor hashes; a resolved thread is only its head; an open thread says whose word is last: `pending` when it is the user's, `answered by name (type)` or `proposed by name (type)` when an agent's ([0058](decisions/0058-the-user-has-the-last-word.md)), and a message the user edited says so; when the session is subscribed, the pending threads count as shown to it, so the hooks do not repeat them; a fired watch is reported first with the `remind` threads in full; works without a viewer; not for polling — the hooks deliver |
| `thread_reply` | answer one thread (`thread`, `body`) or several (`replies`), and get each back as it now stands, with its placement; `resolve` on a reply proposes closing its thread and nothing more — the reply is badged *proposes resolving*, the thread stays open and waiting, and only the user resolves it ([0053](decisions/0053-resolution-is-the-users.md)); `line`/`end_line` say where the thread's lines are now after a rewrite, so it moves there and shows as *edited*, and a detached thread needs them; the batch is checked first, so an unknown id, a resolved thread, or a detached thread without a line refuses the whole call and nothing is written; signed with the session's id, type, and the name fixed at `follow` when the connection subscribed, the session is known from the harness, or `id` is passed, and with the harness's name when it is not, saying so; works without a viewer |
| `thread_start` | start a thread of the agent's own on lines of a file: one with `path`, `line`, `end_line`, and `body`, or several in `comments`, and get each back as it now stands; the comment is signed as a reply is and stamped with the checkout's commit as yours are, so the thread waits on you from birth, shows the agent's name on its comment, and reaches no agent until you reply, edit, or reopen it; the batch is checked first, so a path that is not a file, a range past the end of the file, a file that is not text, or an empty body refuses the whole call and nothing is written ([0061](decisions/0061-agents-start-threads.md)); works without a viewer |
| `thread_watch` | be woken when thread `on` gets a `message` or is `resolved`, reminded of the `remind` threads in full; one-shot; `cancel` removes the watch instead; fails when a named thread does not exist |

Every tool but `workspaces` accepts an optional `workspace`: a root, or
a viewer name or id. A subscription covers the whole workspace
([0055](decisions/0055-six-tools.md)); agent types are labels the viewer
shows next to a message (`name (type)`), and the only rule they carry is
that an agent is woken by the user's word alone: a thread reaches an
agent while the user's comment, reply, edit, or reopen is the newest
thing on it, and any agent's answer leaves it waiting on the user until
they speak again ([0058](decisions/0058-the-user-has-the-last-word.md)).
The user's own name on every surface comes from `user.name` in the
config. Every failure names the call that fixes it. A thread belongs to the commit it was written against and is shown
(here and in the viewer) only while that commit is `HEAD` or one of its
ancestors, so switching to unrelated work hides it and merging brings it
along ([0024](decisions/0024-workspace-sessions.md)). An amend, squash,
or rebase that drops that commit does not lose an open thread: while its
lines are still in the working tree it moves to the new `HEAD`
([0035](decisions/0035-threads-follow-head.md)). Details and the wire
protocol are in [0014](decisions/0014-mcp-server-and-socket-v1.md);
[0062](decisions/0062-one-version-no-compatibility.md) trims it to
the requests that have a client and one protocol version.

### Hooks: comments reach the agent

Without a hook the agent only sees comments when it polls. With the
hooks, the harness runs `fathomable pending` at up to three points,
and each thread reaches the agent once, at whichever comes first
([0042](decisions/0042-turn-start-delivery.md)):

| Hook event (Claude / Codex · Copilot) | When it delivers | What the agent sees | Cost |
|---|---|---|---|
| `Stop` · `agentStop` | the agent tries to end its turn | the turn continues with the threads as the prompt; the only forced channel, and the one that counts toward `nag-after` | one spawn per turn; **install this one** |
| `UserPromptSubmit` · `userPromptSubmitted` | a prompt is submitted — typed, or the synthetic one Claude Code wakes the model with when a background task finishes | the threads as context at the start of the turn; a comment posted while the agent waited is there on the wake | one spawn per prompt (Copilot: per stop-block continuation too); silent unless something is new |
| `PostToolUse` · `postToolUse` | every tool result | the threads as context before the agent's next step, so a comment posted mid-task lands within the turn | one spawn and two small file reads per tool call; silent unless something is new; optional, for long turns |
| — · `notification` | a detached (`async`) shell the agent started finishes (`shell_detached_completed`; every other notification type, permission prompts included, gets silence) | the threads queued as a message that starts a turn, even if the agent was idle | one spawn per notification; silent unless something is new |

A session that never called `follow` with an `id`, a subagent, or a
directory Fathomable has not seen all get silence and exit 0 at every
point ([0040](decisions/0040-agent-subscriptions-and-hooks.md)). The
agent is told not to poll: after a wait it ends its turn, and the
hooks do the rest.
`hello` also notes which processes it ran under, so the `fathomable
--mcp` the same harness started can tell the session it serves from its
own process ancestry: a `follow` with only a `type`, and a `thread_reply`
after a resume, are then signed with that session without the model
repeating its id ([0041](decisions/0041-session-bonds.md)). Where that
bond cannot be made — `/proc` denied, the server not under the harness,
or two sessions under one recorded shell — the tools ask for the `id`
instead, and `thread_reply` takes one. Only processes born within 30 s
of the hook count, so a shell opened just before the harness can be
recorded; a hand-run `fathomable --mcp` in that shell would then sign
as that session.
A directory is *seen* once a viewer has opened it or
`fathomable --register [DIR]` has marked it; `scripts/demo-repo.sh`
(`just demo`) builds a throwaway repository, registers it, and seeds
threads and a subscriber to try the loop against (through the hidden
`fathomable seed FILE` subcommand, so the script never writes the store
files itself).

Two subcommands, both reading the harness's hook JSON on stdin:

- `fathomable hello --hook <harness>` (session start) tells the model what
  Fathomable is and that it is already connected as an MCP server, then
  its workspace, session id, the user's name (`user.name`), and the
  whole `agents.types` list as labelled fields, and the `follow`,
  `thread_reply`, `thread_start`, and `thread_watch` calls to make. The tools are spelled as the harness shows them —
  `mcp__fathomable__follow` under Claude Code, `fathomable.follow` under
  Codex, the bare name elsewhere — and Copilot's text adds that a
  detached shell finishing brings comments too
  ([0043](decisions/0043-agent-vocabulary.md)). When the workspace it
  resolved contains the cwd only by prefix — it is neither the cwd nor
  the cwd's git root, as when a parent directory was registered and the
  repository was not — it adds a warning that comments left at the cwd
  will not reach the session, and names `fathomable --register`; with
  `--verbose` the same is one line on stderr. On a *resume* —
  `source: "resume"` in the hook JSON — it prints the pending threads
  after it, so a session that comes back after being stopped starts with
  them in context instead of waiting for a turn to end. The other
  sources are left to the stop hook: `startup` and `fork` have not
  subscribed yet, and a `compact` happens mid-task, where the blob would
  be consumed into context the model may never act on.
- `fathomable pending --hook <harness>` (stop, prompt-submit, and
  post-tool-use) blocks the stop with the pending threads, or says
  nothing. Run from the prompt-submit or post-tool-use event — told
  apart by `hook_event_name`, or by the `prompt` and `toolName` fields
  no stop payload carries — it prints them as context and exits 0, and
  never counts toward `nag-after`, so a harness that runs it on every
  stop-block continuation (Copilot) or every tool call spends nothing.
  `--prompt` prints them to stdout instead, and `--id ID` names the
  session when no JSON is piped, so
  `claude -r ID "$(fathomable pending --id ID --prompt)"` wakes an idle
  session by hand.
- `--verbose` on `hello` or `pending` explains a silent hook: every
  lookup and what it found (stdin, workspace, live viewers, config,
  subscribers, MCP bonds, watches, thread counts) and the reason for
  silence, one `fathomable:` line each on stderr — stdout when stderr
  is the block reason. Add it to a hook command and read the harness's
  hook log (`claude --debug`, `Ctrl-O`) to see why an agent is not
  hearing you.

`agents.max-lines` bounds the newly pending threads only. Past it they
are named by id and place under `N more; call threads`, and are
deliberately *not* recorded as delivered, so that call returns them in
full. A fired watch and the threads it reminds of are always shown
whole: the watch is spent when it fires and a reminded thread need not
be pending, so neither could be fetched a second time.

In the viewer, an agent's messages are labelled `name (type)`, a thread
an agent is watching says `watched by name (type)` in its pane header,
and `:status` lists who is subscribed. `Space a w` wakes a subscriber by
hand with the same prompt the stop hook would give it, through the
`agents.wake` command — which runs detached, so it must be
non-interactive: `codex queue --thread {id} --message {prompt}` or
`claude -p -r {id} {prompt}`.

Install the hooks per user, not in the repository, so a clone does not
opt anyone in. `<harness>` is `claude`, `codex`, `copilot`, or `vscode`.

Claude Code, in `~/.claude/settings.json` or the project's
`.claude/settings.local.json`:

```json
{
  "hooks": {
    "SessionStart":     [{ "hooks": [{ "type": "command", "command": "fathomable hello --hook claude", "timeout": 5 }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook claude", "timeout": 5 }] }],
    "PostToolUse":      [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook claude", "timeout": 5 }] }],
    "Stop":             [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook claude", "timeout": 5 }] }]
  }
}
```

Codex CLI, in `~/.codex/hooks.json` (then trust it with `/hooks`):

```json
{
  "hooks": {
    "SessionStart":     [{ "hooks": [{ "type": "command", "command": "fathomable hello --hook codex", "timeout": 5 }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook codex", "timeout": 5 }] }],
    "PostToolUse":      [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook codex", "timeout": 5 }] }],
    "Stop":             [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook codex", "timeout": 5 }] }]
  }
}
```

Copilot CLI and VS Code read the same file, `~/.copilot/hooks/fathomable.json`
(or `.github/hooks/fathomable.json` once the folder is trusted); use
`--hook copilot` in the CLI and `--hook vscode` under VS Code, whose
`Stop` has no loop cap of its own:

```json
{
  "version": 1,
  "hooks": {
    "sessionStart":        [{ "type": "command", "bash": "fathomable hello --hook copilot", "timeoutSec": 5 }],
    "userPromptSubmitted": [{ "type": "command", "bash": "fathomable pending --hook copilot", "timeoutSec": 5 }],
    "postToolUse":         [{ "type": "command", "bash": "fathomable pending --hook copilot", "timeoutSec": 5 }],
    "notification":        [{ "type": "command", "bash": "fathomable pending --hook copilot", "timeoutSec": 5 }],
    "agentStop":           [{ "type": "command", "bash": "fathomable pending --hook copilot", "timeoutSec": 5 }]
  }
}
```

## 9. When something is off

```sh
fathomable --doctor        # terminal, directories, config, log locations
fathomable --viewers       # known workspaces and their viewer records
fathomable --config-show   # effective configuration
fathomable --register [DIR] # mark a workspace known without starting a viewer
echo '{"session_id":"ID","cwd":"'$PWD'"}' | fathomable pending --hook claude --verbose
                           # why a hook is silent for session ID
```

Each run logs JSON lines to `$XDG_STATE_HOME/fathomable/log/<session-id>.log`;
`:status` inside the app shows the subscribed agents (`name (type) id`),
the open document, the terminal size, the
viewer name and id, the socket, and every state path. Set
`FATHOMABLE_LOG=debug` for more. A viewer killed without a clean quit is
swept away by the next start.

If Fathomable dies, it hands the terminal back and prints one block between
two rules: what it was showing, where its state lives, and a backtrace with
the runtime plumbing dropped. Paste that block at your agent. A copy is
written to `$XDG_STATE_HOME/fathomable/log/<session-id>.crash`, so a report
that has scrolled away is still there; `--doctor` counts what is waiting
there ([0022](decisions/0022-crash-reports.md)).
