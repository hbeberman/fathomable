---
type: Guide
title: Setup guide
description: Install Fathomable, open a workspace, learn the keys, configure themes, and connect an agent over MCP.
tags:
  - onboarding
---

# Setup guide

The human-facing walkthrough: from a fresh checkout to a viewer and
repository-bound review discussion. Keep it current whenever a flag, key, file path, or tool
name changes; the [decisions](decisions/index.md) hold the reasoning, this
page holds what to type. Linux only for now (see the
[charter](charter.md)).

## 1. Install

Requires Rust 1.97 or newer and Linux: `rust-toolchain.toml` pins the
toolchain, so rustup installs 1.97.0 for you inside the checkout. Viewer
liveness is read from `/proc`, so on another OS every viewer looks dead.
Chat identity uses harness environment or MCP metadata, not process
ancestry; see [Connect an agent](#8-connect-an-agent).

```sh
git clone <this repository> fathomable
cd fathomable
make install          # cargo install --path crates/fathomable --locked
fathomable --version
```

The [README](../README.md) lists the system packages per distribution.
Contributors also want the build tooling and the commit gate; see
[CONTRIBUTING.md](../CONTRIBUTING.md).

## 2. Open something

```sh
fathomable README.md     # single file
fathomable               # workspace rooted at the enclosing git root, or cwd
fathomable path/to/dir   # workspace rooted there
fathomable --name review # a second window on the same workspace, named for you
```

A file argument opens the workspace *and* shows that file. Several
Fathomables may show one workspace; they share its threads. `--name` (or
`:name` later) gives a window a human-readable label; MCP does not route to
or control viewers. The viewer
re-reads a file when it changes on disk and keeps your position, so leave
it open next to an editor or an agent.

A git workspace is the repository, every worktree of it included
([0070](decisions/0070-one-workspace-many-worktrees.md)): a checkout
made with `git worktree add` shares the threads, seen marks, and
checkpoints of the main one, and a viewer opened in any of them lists
them all. One worktree is *active* at a time; `]w` and `[w` page
through them, the files pane header leads with the active branch while
there are several, and a click on that branch opens a picker of them.
Paging re-roots the viewer: the file you are reading stays open by its
path when the worktree has it, the other open files close. A thread an
agent started on a branch you have not merged still shows, with the
branch after its author, and opening it pages there.

The workspace is live too. A file or directory the agent creates,
deletes, or renames shows up in, leaves, or moves within the tree on its
own (within `watch.debounce`) — a whole new directory arrives
collapsed, and is listed when you expand it — so `R` is only for a
listing you suspect is stale. Fathomable watches visible directories
individually and does not watch ignored trees, so build output under
`target/` or another ignored cache consumes no watches, event-loop work,
or redraws. An ignored file you explicitly open is watched narrowly and
still reloads, including while another file is in front. If the file you are reading is deleted,
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
bundle (TOML, KDL, Dockerfile among them) shows plain. `Space v s` flips any
file between the two views.

A binary file — one git would diff as binary: `-diff` or `binary` in
`.gitattributes`, else a `NUL` in its first 8000 bytes — opens as a
**file-info pane** instead: the format its magic number gives away
(WebAssembly, PNG, ELF, gzip, …), its size, mode, and modification time,
and its git state with the `HEAD` size beside the size on disk. A text
file over `viewer.max-file-size-mib` (64 MiB by default) gets the same
pane with a notice saying which config line raises the limit. Neither can
be annotated: `c` says so.

### Menu bar

The first row is a persistent, borderless menu bar on `ui.menu`:

```text
 ☰  Go  Review  Diff                         main · docs/guide.md
```

The right side passively names the active branch or worktree, the current
path, and a major view such as `SOURCE`, `review threads`, or the current
diff pair. Those facts leave the bottom status line while the bar is shown
and return there when it is hidden. On a narrow terminal the context drops
first; when all four labels no longer fit, Go, Review, and Diff move under
`☰`. The row never wraps.

Hover highlights a label. Click to open it; while a menu is open, moving
across the labels switches menus. Drop-downs have rounded titled borders,
stable rows, dim shortcut hints, checks for active state, and dim rows for
actions that cannot run now. `j`/`k` or Down/Up move through enabled rows
without wrapping. Up from the first row focuses the menu-bar label, where
`h`/`l` or Left/Right switch menus and Down re-enters the first row. Right
or `l` enters a submenu, Left or `h` returns, Enter runs, and Esc closes.
A short terminal scrolls a long menu in place with the wheel or movement
keys rather than moving it away from its label.

`☰` contains **Layout** (sidebar, Files, Threads) and **Help** (Getting
started, Doctor, View keymap), then Status, About, and Quit. **Go** contains
the file pickers, Back/Forward, and Newest change. **Review**
contains the review view and filters plus new thread, file comment, reply,
edit, and resolve/reopen. **Diff** contains comparisons, base and target
pickers, whitespace, checkpoints, and mark-seen state. Cursor movement,
selection, deletion, and bracket-pair navigation stay in the keymap and
contextual surfaces rather than filling these application menus.

`Space p m` hides or shows the menu bar; the visible `☰` menu deliberately
cannot hide itself. `Space p s` hides the whole sidebar and later shows its
remembered Files/Threads composition. The individual pane toggles still work
and establish a new restore composition when the sidebar is hidden.

## 3. Keys

Vim-style movement in the text; `Space` opens a Helix-style menu, and any
prefix (`g`, `[`, `]`, `Space`, `d`) shows the keys that continue it under
a row naming the prefix (`Space c · threads`).
`Space ?` opens **View keymap**, which lists every binding inside the app.
Every key below is checked
against the binding table by a test, so what is written here exists.
At an ordinary 80-column terminal the help is a compact two-column
grouped action list; it collapses to one column when narrow. Long keys
and descriptions wrap, and the complete table scrolls rather than
dropping later actions. Use `j` / `k` or `Up` / `Down` to scroll,
`PgUp` / `PgDn` to page (`Ctrl-u` / `Ctrl-d` also work), and the mouse
wheel when preferred. `/` filters by key spelling, action, or group;
type and use `Backspace`, then `Enter` to apply the query and return the
motion keys. `Esc` clears a filter and exits filter editing; a second
`Esc` closes help. A query with no matches says so. Help never sends
typed filter or motion keys to the document underneath.

Text:

| Keys | Action |
| --- | --- |
| `j` `k` `h` `l`, arrows | move; `h` at the first column wraps onto the end of the row above and `l` at the last onto the start of the row below |
| `0` `$`, `Home` `End` | start / end of the current rendered row |
| `gg`, `ge` / `G` | top / bottom |
| `gh`, `gl` | goto logical line start / end, across wrapped rows; start includes indentation, end lands on the last character |
| `Ctrl-d` `Ctrl-u` | half page down / up |
| `/` `?`, `n` `N`, `:noh` | search, next / previous match, clear highlight |
| `:N` | go to source line N |
| `Space v s` / `:source` | toggle raw source view |
| `Space d d` / `:diff`, `Space d D` / `:diff seen` | the net diff from `HEAD` to the worktree; the diff against last seen; the same key again, or `Esc`, closes it |
| `h` `l`, `b` `t`, `w` in a diff | earlier / later pair along the file's checkpoint timeline; pick the base / target from checkpoints, the worktree, last seen, `HEAD`, `INDEX`, and commits; ignore whitespace |
| `D` | the next diff: unstaged (`INDEX → WORKTREE`), staged (`HEAD → INDEX`), last seen, newest checkpoint, then the file, skipping unavailable pairs |
| `]g` `[g`, `]G` `[G` | next / previous hunk, crossing into the next uncommitted file; next / previous uncommitted file |
| `]f` `[f` | next / previous changed file |
| `]w` `[w` | next / previous worktree of the repository, wrapping ([0070](decisions/0070-one-workspace-many-worktrees.md)) |
| `Alt-Left` `Alt-Right` | back / forward through the jumplist: the positions far moves leave behind (another file by any route, `gf`, a search jump, `gg` / `G`, `:N`, `]c`, `]g`); `j` `k`, paging, and the mouse leave nothing |
| `v` / `V` / `x` or mouse drag, then `y` / `c` | select text / lines (`x` grows a line per press), then copy or comment; `y` with nothing selected copies the cursor line |
| `gf`, Ctrl-click | open linked file/URL: local files open in the viewer at their line, URLs through `xdg-open`; accepts Markdown links and bare references, including `path:line`, `path:line:col`, or `path#L12`, read against the file's directory and then the root; `Alt-Left` returns from a file hop |
| `c` with nothing selected | expand the thread at the cursor in place, or comment on the line when there is none; on an expanded thread, fold it and expand the next thread covering the lines, until none is |
| `r` `e` `o`, `dd` | reply to the thread here, edit the message here when yours, resolve or reopen, delete (on an expanded thread's rows, or the thread at the cursor) |
| `C` | always start a new thread, on the selection or the cursor line |
| `z`, `Z` | expand or fold the thread at the cursor; expand every thread in the file, or fold them all when any is expanded |
| `]c` `[c`, `]C` `[C` | next / previous thread in the file; across the workspace, opening its file |
| `]r` `[r`, `Tab` `Shift-Tab` | next / previous thread waiting on you, crossing into the next file, expanded where it lands |
| `:help`, `:doctor`, `:about` | reopen the first-workspace Getting started page; run the in-app diagnostics view; show project identity and repository link |
| `:status`, `:name NAME` | viewer and path popup; name this viewer window (`:name` alone clears it) |
| `Esc`, `:q` | clear the input, prefix, selection, or highlight, else leave the diff; quit |

The `Space` menu, from any pane:

| Keys | Action |
| --- | --- |
| `Space f` | file picker |
| `Space F i`, `Space F r` | files: the picker including ignored files; the recent files |
| `Space F c`, `Space F u`, `Space F g` | files: only changed files in the files pane; hide untracked files; show ignored files (session toggles, from any pane; each entry says what pressing it does now) |
| `Space r` | the review list: every thread on the work in place of the document, by file then line under a row per file, resolved hidden; on the focused list, close it |
| `Space w h`, `Space w l` | window: the pane left of the text (the files pane, or the threads pane when the files pane is hidden; the files pane is shown when neither is); back to the text |
| `Space w j`, `Space w k` | window: from the files pane down to the threads pane, and back up, when both are shown |
| `Space w w` | the next pane: text, files pane, threads pane, text, skipping a hidden pane |
| `Space w f`, `Space w t` | window: the files pane, the threads pane, from any pane; a hidden one is shown first |
| `Space p f`, `Space p t` | panes: hide the files pane or the threads pane, or show it again without taking the keys (the other pane keeps the sidebar) |
| `Space p s`, `Space p m` | panes: hide/show the whole sidebar with its remembered composition; hide/show the persistent menu bar |
| `Space c c`, `Space c r`, `Space c o`, `Space c e`, `Space c d` | threads, on the thread at the cursor from any pane: new thread, reply, resolve or reopen, edit your newest message, delete |
| `Space c f` | threads: a comment on the open file as a whole, written in a block above its first line |
| `Space v s`, `Space v t`, `Space v x` | view: toggle source view, thread stubs, stubs for resolved threads (hidden by default) |
| `Space d d`, `Space d D` | diff: the diff against `HEAD`, against last seen |
| `Space d r`, `Space d g` | diff: the checkpoint diff, opened on the latest checkpoint against the working file; pick a commit to diff against the working file |
| `Space d b`, `Space d t` | diff: pick the base, the target (as `b` `t` in a diff; from outside one, against the working file) |
| `Space d c`, `Space d C` | diff: checkpoint this file; checkpoint the workspace, every non-ignored text file whose content moved since its last checkpoint (a toast counts them) |
| `Space d w` | diff: ignore whitespace (as `w` in a diff) |
| `Space d s` | diff: mark every file seen, so `Space d D` from then on shows only what came after (a toast counts them) |
| `Space j j` | jump to the newest change |
| `Space a w` | show `Wake agent is not yet implemented` |
| `Space ?` | view keymap |
| `:` | the command line, from any pane |

Threads pane (the sidebar's lower pane; its header reads `threads ·
file` or `threads · workspace` with the counts by colour at its right
edge, `● 2 user ● 1 agent ◐ 1 resolve? ○ 3 resolved` when the row holds
the words and `● 2 ● 1 ◐ 1 ○ 3` when it does not; in workspace scope a
row per file over its threads, each thread's two rows sitting two cells
in under the path as the files pane nests a directory's children, and
in file scope the threads alone at the same indent; while the pane has
the keys, its bottom row is a key bar naming them):

| Keys | Action |
| --- | --- |
| `j` `k` | next / previous thread, wrapping, a folded file counting once; the text follows, and in workspace scope the file opens |
| `Enter` / `l` / `Right` | open the file with the thread expanded, the keys going to the text |
| `s`, `x` | list this file or the workspace; show or hide resolved threads, those resolved at earlier commits of the branch among them with the commit named (the review list shares the flag) |
| `z`, `Z` | in workspace scope: fold the cursor's file to its row, or unfold it; fold every file, or unfold them all; a file row carries `▾` open and `▸` folded, here and in the review list |
| `r` `o`, `dd` | reply, resolve or reopen, delete |
| `Esc` | back to the text; the pane stays (`Space p t` hides and shows it) |

Review list (`Space r`; its header reads `review threads  ● 2 user ● 1
agent ◐ 1 resolve? ○ 3 resolved`, the counts by colour as the threads
pane's, each with its word while the row holds them all and bare
otherwise, then ` · path` while `f` narrows it;
the keys below sit on a bar along the list's bottom row, and the
header and the bar stay on neutral `ui.header`; selectable file and
thread headers and folded rows use the shared list selection below.
A thread's rows sit two cells in under its file row, in file scope too.
The selected thread's header stays selected alongside its message;
the ancestor file's edge bar alone is muted context, unless that file
row itself is selected):

| Keys | Action |
| --- | --- |
| `j` `k` / `Down` `Up` | next / previous stop: a file row, then each thread of the file while it is unfolded, a folded thread being one row; a thread's newest message is highlighted, and on a file row the file's first thread is the cursor's |
| `l` `h` / `Right` `Left` | next / previous message in the thread; nothing on a file row or a folded thread |
| `gg` `ge` `G` | first / last stop |
| `Ctrl-d` `Ctrl-u` | half a page of rows |
| `Enter` | open the file with the thread expanded and the cursor on the highlighted message |
| `r` `e` `o`, `dd` | reply, edit your highlighted message, resolve or reopen, delete |
| `x`, `f` | show or hide resolved threads, those resolved at earlier commits of the branch among them with the commit named (the threads pane shares the flag); only this file |
| `z`, `Z` | fold or expand the thread here (one row, the stub's form, with no blank row after it), or on a file row fold the file to its row or unfold it; fold every thread, or expand them all when every one is folded; the list opens with every thread expanded and remembers its folds while the viewer runs |
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
| `j` `k` / `Down` `Up` | move, previewing the highlighted file or a brief summary of the highlighted directory without leaving the files pane |
| `h` / `Left` | collapse a directory or go to the parent |
| `l` / `Right` | expand or descend into a directory; do nothing on a file |
| `Enter` | open the file and focus the text; toggle a directory |
| `gg` `ge` `G` | top / bottom |
| `y` | copy the highlighted entry's path, relative to the root |
| `t` | the threads pane in file scope on the highlighted file, with the keys |
| `Esc` | back to the text; the pane stays |
| picker `Ctrl-j` `Ctrl-k` / arrows, `Enter`, `Esc` | move, open, close |

**List focus:** files, threads (including file-group rows), review entries,
and every picker's results use a blue-tinted selected row plus a bright
left-edge bar while they own the keys. A remembered selection has a
quieter tint without the bright bar; pane headers remain neutral. The
built-ins do not force bold for either state; the active bar is the
non-colour focus cue. Review
messages keep their author stripes instead of taking the list tint, with
the shared cursor bar down the selected message only bright while review
owns the keys. Its thread header stays selected alongside the message.
Thread-state circles and text keep their colours.

Help, Status, a picker, a right-click menu, a draft (Compose), and a pending
key prefix make underlying list selections inactive. Returning the keys
restores the active highlight without moving your position. Help and
menus have subtle hover highlights only, never a selected keyboard item
or bright cursor bar: their keys scroll, filter, or run shortcuts.
See [0079](decisions/0079-list-focus-language.md).

Copy uses OSC 52, so it lands in the system clipboard through most
terminals and multiplexers.

Key-chord helpers sit at the whole viewer's lower-right corner, above
the global status line, regardless of the focused pane. Their text stays
left-aligned inside the popup.

`gf`, Ctrl-click, and the context menu use the same opener. URLs require
`xdg-open` on the viewer host (normally supplied by `xdg-utils`) and a
working desktop URL handler. Missing commands and failed launches are
reported in the viewer; availability is determined by actually launching
the opener, not guessed from terminal capabilities. Over SSH this opens
on the remote host, not the local terminal. Fathomable does not emit OSC 8
hyperlinks or probe support for them.

All long lines wrap to the pane. Prose wraps at words; code blocks,
source files, diffs, and very narrow tables wrap between displayed
characters. Wrapped diff continuations align under a blank sign cell.
Copying a selection still yields the original source text.

Scrolling stops with one unnumbered `~` gutter row after the document.
When the pane's key bar is shown, that EOF row remains visible above it.
The cursor stays on document or thread rows, never the EOF marker.
While the file viewer has the keys, its current source line number is
brighter, using the active lists' `ui.list.cursor` colour without tinting
the text row. It dims back to `ui.linenr` when another pane, popup, key
prefix, or command/search input takes the keys. A wrapped continuation
highlights its source line's number; rows without a source line do not
highlight a number.

The mouse works on whichever pane it is over: the wheel scrolls the pane
under the pointer — over the tree it steps one row per tick, showing
each file it lands on — and a click focuses the pane. A click in the
tree stays in the tree: it expands a directory or shows a file like the
wheel does, and only `Enter` moves focus to the view. A click on a
stub's `▸`, or a double-click anywhere on the stub, expands its
thread; a click on the `▾` in the expanded thread's gutter, or a
double-click anywhere on its header row, folds it again. One click
elsewhere on either row only places the cursor, on that row itself:
the `▎` bar at the row's left edge stands in for the block cursor
there. In the review list a thread's header and its folded row take
the same chevron click and double-click, and a click on a file row
folds or unfolds the file and rests the cursor on it. Drag the
sidebar's divider or the threads pane's rule to resize them.

A **right-click** opens a menu of what the pointer is on, each entry
showing the key that does the same: on a selection, comment, new
thread, copy, and clear; on a line a thread covers, expand or fold,
reply, resolve or reopen, edit, and delete; on a URL or a path that names
a file, `open linked file/URL`;
on any line, comment, select, and copy. In the files pane it offers open,
checkpoint, copy path, the three filter toggles worded as they would
act now, and, on a file with threads, `threads` (the
threads pane on it) and `review`; on a threads pane thread, go to,
reply, resolve, edit, delete, and fold file; on a review list thread,
fold or expand thread, then go to, reply, resolve, edit, and delete;
on a file row of either, fold or unfold, in the pane fold all or
unfold all, open file, and the resolved toggle. A right-click
outside the selection moves the cursor there first; inside it keeps the
selection. Hover highlights an entry; a click or its key runs it; `Esc`
or a click elsewhere closes the menu. The `delete thread` entry deletes
at once. The `Space` menu and `Space ?` take clicks too, as do the key
hints on the text's, the review list's, and the threads pane's key bars (a click on a bar
that says `click or Space w l to focus` focuses that pane); a click on
either row of a thread in the threads pane
lands on it and one on a file row folds or unfolds it; a click
on the threads pane header's words toggles its scope and one on its
resolved count toggles `x`, as in the review list's header; on the
diff header's base or target
name opens that picker; and on the status line the waiting count opens
the review list and the thread count focuses the threads pane.
In `Space ?`, a click on any wrapped row runs that binding when it
applies to the pane that had focus; filtering, scrolling, and resizing
all update the click targets.

Selecting with the mouse: drag over text for a character selection; a
press or drag in the gutter selects whole lines; a double-click selects
the word and a triple-click the line; Shift-click extends the
selection to the pointer where the terminal passes Shift through (most
keep it for their own selection). Every gesture ends in `SEL` mode, so
`y`, `c`, `C`, and the right-click menu apply. A Ctrl-click places the
cursor and runs `gf` there. The right button and the Ctrl modifier reach
the viewer only where the terminal forwards them under mouse capture
(Ghostty, kitty, foot, WezTerm, and Alacritty do).
By default every launch shows both sidebar panes. Starting on a directory
gives the Files pane the keys; starting on a file opens it with the text
holding the keys. The `layout` configuration can choose another consistent
startup arrangement.

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
alone, and it is the colour of whoever has the last word: your blue
while open (`thread.open`, the hue of `thread.user`), an agent's bold
green when it waits on you (`thread.waiting`, the hue of
`thread.agent`), grey once resolved (`thread.resolved`); *edited* and
*detached* are words in an expanded
thread's header, not colours. Every surface that names a thread draws
one **circle** in that colour, the fill saying where the thread is in
its life:

| glyph | meaning |
| --- | --- |
| `●` | open (your blue), or waiting on you (the agents' green) |
| `◐` | an agent's newest reply proposes resolving it |
| `○` | resolved |
| `?` | the thread's lines are gone (detached) |

The gutter, a stub, an expanded thread's header, the threads pane, the
review list, the files pane, and the status line's waiting count all
draw it. A comment on the file as a whole
(`Space c f`, or an agent's `thread_start` with no `line`) has no
lines: its stub stands above the first line, its header reads
`file · open`, and it never moves, detaches, or re-anchors. The lines themselves carry no tint;
the gutter's bracket on the lines of the thread the cursor is on
lights up in yellow (`thread.bracket`), so the corners and the line
between them say which thread the keys act on
([0074](decisions/0074-the-bracket-marks-the-focused-thread.md)).
Each message of an expanded thread sits on a faint stripe of its
author's kind, blue for you and green for an agent (`thread.user`,
`thread.agent`; the name on the author row takes the same colour), and
the thread's rows begin two cells after the gutter, a gutter of the
thread's own: the header of the thread the keys act on carries a `▎`
bar (`thread.cursor`) there, and so does its stub while it is folded, and once your cursor is on the thread's
own rows the message it is on carries the same bar down its left edge
with its name in bold; from the thread's lines above, the header alone
is barred. In the review list, `ui.list.cursor` marks the selected
message in the first cell while the author and body keep their stripes;
only that bar changes with focus, bright while review owns the keys.
Selectable file/thread headers and folded rows take the shared list tint;
the selected thread's header stays selected alongside its message, and
the ancestor file's bar is muted info-colour context. A draft is written on its own
warm surface (`thread.draft`) and takes your stripe on submit
([0071](decisions/0071-author-stripes.md)).
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
(`~/.local/state/...` by default), one append-only JSON line per event;
`<hash>` is of the repository's git common dir, so every worktree
reads the same file, or of the root outside git
([0070](decisions/0070-one-workspace-many-worktrees.md)).
Each line carries format version **2**, the only annotation version this
build reads or writes. A file of another version is refused with the line
to blame, both versions, and actionable path/reset guidance; there is no
migration or older reader
([0062](decisions/0062-one-version-no-compatibility.md)).

Every open thread shows a **stub** under the last of its lines: one row,
its newest message, with a `▸`, the thread's circle, the author
(the configured user name, or an agent's stored name; no subscription
type), the age, and the first line of the message, on the author's stripe
with the name in the author's
colour, as the message reads once expanded (`thread.user`,
`thread.agent`, over `thread.inline`; a theme that sets no background
gets a `▎` at the left edge instead). Stubs are not lines: they carry
no line number and selection never takes one; but `j`/`k` and the
other motions stop on a stub as on a line, the cursor resting on the
stub itself with the `▎` bar at its left edge standing in for the
block cursor (a click rests it there too), the stub's thread the
cursor's, and `z`, its `▸`, or a double-click expands it. Stubs of
threads stacked on one row follow one another in line order. The stub
of the thread under the cursor reads in the text colour, bold for the
thread the cursor is on; the others are dimmed.
`Space v x` gives resolved threads a stub too, and `Space v t` hides
stubs altogether; `threads { stubs; stubs-resolved }` sets both
defaults. Resolving fixes a thread to the commit that is `HEAD`, and a
resolved thread shows in the file, its grey circle and tint included,
only while that commit is `HEAD`: the next commit or checkout takes it
out of the file and the tools, and checking that commit out again brings
it back ([0072](decisions/0072-a-resolved-thread-stays-at-its-commit.md)).

`c` or `z` on a line a thread covers **expands** its stub in place, the
view staying still: a header row with a `▾`, the state and placement,
then every message rendered as Markdown. The text's keys sit
on a **key bar** that replaces the bottom text row while it has
something to say (a thread under the cursor, a thread in the file, or a
draft); the viewport stays put unless its cursor would be covered, and
scrolling keeps the last line and its `~` marker above the bar. While the text has the keys it
names the keys of the thread the cursor is on (`r reply · e edit · o
resolve · z fold`, or `z expand` on a stub, `e` only on your own
message) and `Z fold all` or `Z unfold all` while the file has threads;
while another pane has the keys it reads `click or Space w l to
focus`. A hint on the screen always does what it says. Its message rows are
cursor rows: `j`/`k` walk the messages, `r` replies and puts the cursor
on the reply, `e` edits the message under the cursor when you wrote it,
`o` resolves or reopens, `dd` deletes the thread, and `z` or `c` on any
of its rows folds it. `z` only opens and closes; `c` walks on: when
several threads cover the line, `c` again folds the expanded one and
expands the next in line order, wrapping, and after the last leaves none
expanded. `Z` expands every thread in the file, or folds them all when
any is expanded. A click on a stub's `▸` or a double-click on the stub expands
it; a click on the header's `▾` or a double-click on the header folds
it. Folding from the thread's rows leaves the cursor on the stub, its
`▎` bar standing in for the block cursor until `j` or `k` steps off.

Writing happens in the same rows: the **draft** is not a box along the
bottom but rows of the text. A reply is written at the end of its
thread's expanded rows, under a ` User  draft` row (the name from `user.name`); the draft keys are
on the text's key bar (`Enter submit · Alt-Enter newline · Alt-k/j
scroll · Ctrl-e $EDITOR · Esc`, or `Esc again to discard` once you have
pressed Esc on a changed draft); an edit replaces the message it edits,
seeded with its text; and a new comment (`c` on a line with no thread,
`C`, or `Space c c`) gets a block of its own under its lines, headed
`comment on L3-5`; a comment on the file (`Space c f`) gets one above
the first line, headed `comment on README.md`. The draft wraps at the text width and grows with
what you type, the view scrolling just enough to keep its cursor on
screen while the text cursor stays on the message; a click in the draft
places its cursor. Submit, cancel, or clear an empty draft and the rows
go: a reply becomes the newest message under the cursor, a new comment
becomes a stub.

A draft stays with the file where you started it. Switch to another file
and it is hidden, not submitted or discarded; return and its text and
editor cursor are waiting. Each file can keep its own draft during the
session, including replies, edits, and comments on the whole file.

The left column is the **sidebar**: the **files pane** above the **threads
pane**, each shown or hidden on its own (`Space p f`, `Space p t`),
the sidebar drawn while either is. `Space p s` hides them as one unit and
shows the exact composition it hid; when both are already hidden, a
pane-specific toggle establishes the next composition. If the configured
composition contains neither pane, the whole-sidebar toggle reports that
there is no pane to show. The files pane's header row names
the repo's directory with its summed `+n -m`; three session toggles
under `Space F` narrow what it lists, and the header names each active
one after the counts by what is on screen: `Space F c` lists **only
changed** files (`M`, `A`, `D`, and `U` against `HEAD`, directories with
nothing to show left out; the header reads `· changed`), `Space F u`
**hides untracked** files (`tracked`), and `Space F g` **shows ignored**
files (`ignored`; an ignored file is never a changed one, so only
changed wins). Ignored entries are browsed from directory snapshots,
not recursively live-monitored; press `R` to refresh that view. The
popup's entries say what a press does now, `only
changed` or `all files`, and the pane's right-click menu carries the
same three. The toggles work from any pane; while the files pane is
hidden the status line names the new state instead. The open file may
drop out of the list and is highlighted again when it qualifies. The threads pane lists this file's
threads in line order or, after `s`, the whole workspace's grouped by
file, resolved ones hidden until `x` shows them, the ones resolved at
earlier commits of the branch among them. Each thread takes two
rows: its circle, `L3-5` (or `file`), and who wrote its newest message
(`name`), then the
branch of the worktree that shows it when the active one does not
([0070](decisions/0070-one-workspace-many-worktrees.md)), with
`↩n` when replied and the age at the edge; then that message's first
line, cut with `…`. In workspace scope a row per file in the files
pane's order sits over its threads with the count at the edge; `z`
folds a file to `▸ path  n` and `Z` every file, the fold outliving a
scope or file switch, and the current file's rows carry the focus
tint (`thread.focus`), independent of selection and overridden by an
actual active or remembered selected row. The header counts by colour
(`● 2 user ● 1 agent ◐ 1 resolve? ○ 3
resolved`, the words dropping together when the row is too narrow for
them; the resolved count is dim while hidden), and while the
pane has the keys its bottom row is a key bar (`s scope · x resolved ·
z fold · Z fold all`, from the end as the column narrows). Beside the
files pane it keeps `sidebar.split` rows (8 by default; drag its rule to
change that for the session), and alone it takes the whole column. The
highlighted entry is the thread under the cursor, both rows on the active
or remembered list surface according to which pane owns the keys, so
reading the file walks the pane; `j`/`k` step the cursor and the text follows, another
file opening in workspace scope, `Enter` opens the file with the thread
expanded, and `r` and `o` act on the highlight. A file with listed
threads shows its most urgent circle after its name in the files pane,
and a collapsed directory its children's.

While the files pane has the keys and its highlight rests on a directory,
the text column shows that directory's path, direct file and subdirectory
counts under the active files-pane filters, and any changed-file, `+n -m`,
open-thread, and waiting-thread totals across its subtree. It does not
repeat navigation hints; directory navigation remains in the files pane.

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

`Space r` shows the whole review at once: the threads pane full
screen. Every thread on the current work (the ones whose commit `HEAD`
can reach) sits under a row per file, files in the files pane's order
and threads by line, each open with its messages under a header that
reads as the expanded thread in the text does (`●  L14-16  waiting
10m ago`). Resolved threads
are hidden until `x` shows them dimmed (the threads pane shares the
flag), and with them the threads resolved at earlier commits of the
branch, which the file no longer shows: each names its commit after the
state (`○  L3  resolved  ab12cd3  2d ago`) at the lines its record
holds, `Enter` lands on those lines with nothing to expand, `r` and `e`
are refused, and `o` reopens it and brings it back into the file
([0072](decisions/0072-a-resolved-thread-stays-at-its-commit.md)). It takes the text column
the way a document does; the sidebar stays beside it. The newest message
in the selected thread starts highlighted, and the thread's header
carries the thread keys; `j`/`k` move between
threads (a folded file counting once), `l`/`h` move between their
messages, `Ctrl-d`/`Ctrl-u` move by
half a page of rows, `z` folds the cursor's file to its row and `Z`
every file (the list's folds are its own, apart from the pane's), and
`e` edits a highlighted
message you wrote. `Enter` opens the file with the thread expanded and
the cursor on that message, `o` resolves in place, `r` and `e` open the
file the way `Enter` does to write the reply or edit in the thread's
rows and bring the list back when the draft closes, `f` narrows the
list to the file you were reading, and `Esc` goes back to it.

A thread is **waiting** on you when it is open and an agent has the
last word on it; your reply, edit, resolve, or reopen ends the wait,
and until then the viewer keeps it in the human-facing waiting set
([0058](decisions/0058-the-user-has-the-last-word.md)). Waiting
threads have their own colour (`thread.waiting`) in the gutter
bracket, the threads pane, and the review list, the status line
counts them (`● 2 waiting` with a green circle, and `1 proposed` before
it while a thread on
the document carries a proposal; `:status` has both totals), the files
pane's circle after their file turns green, and a reply landing while you read raises a toast
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
`╰`, a thread on one row draws its circle, and a thread nested inside another
re-draws the corners on the outer one's line. The rows are the rendered
ones, so a thread on a long markdown paragraph is bracketed across the rows
it wraps to, and the blank rows between paragraphs inside a thread draw `│`.
The bar is thin (`▎`) for a change not yet in the index and thick (`▌`)
for one that is staged; a new untracked file is all thin green.
In rendered Markdown, spacing and table borders between matching added or
changed bars carry the same bar when both sides have the same staging state.
At the end of the file, the `~` line lets the last bar extend through trailing
table borders or other injected rows, but carries no bar itself. Source-backed
rows keep their own status; deletion rules and the leading edge are not
extended.
`Space d d` swaps the pane for the **net diff view**, `HEAD` to the
worktree: a unified diff of the file, a header naming the two sides
(`HEAD · now`), the badge `DIFF net` after the path, and `+added -removed`
counts in the status line. `Space d d` again, or `Esc` once there is nothing
else to clear, returns to the file.

`]g` and `[g` walk the hunks, and when a file's hunks run out they carry
on into the next uncommitted file in path order, wrapping at the end, so
holding `]g` from the top of the tree visits every uncommitted change.
`]G` and `[G` step by file instead, landing on the first hunk. The tree shows every uncommitted file with Git's two-character `XY`
status in its gutter: the first column is `HEAD → INDEX`, the second is
`INDEX → WORKTREE`, and an untracked file is `??`. Thus ` D` is a
worktree deletion, `D ` a staged deletion, and `MD` or `AD` preserves
both layers. Deletions are red, untracked marks green, and modifications
or additions use the staged or unstaged colour for their column. The
aggregate `HEAD → WORKTREE` `+added -removed` counts follow the name
(`bin` for a binary file, which has no lines to count).

Deleted files stay listed, even if their parent folders are gone, until
the deletion is committed or the file is restored. Opening one shows a
searchable, read-only tombstone: the index content for a worktree
deletion, or the `HEAD` content for a staged deletion. A banner names
the missing layer and retained source. Diffs still treat the missing
worktree or index as empty; the retained text is never mistaken for the
live file. A collapsed folder shows the summed
counts of everything beneath it, and the root header shows
the repo's totals (`demo +12 -3`). A save, `git add`, or commit updates all
of this within a beat. A separate cursor cell before the git gutter holds
the active file selection's bar; it never replaces the status letter.

There is a second base. **Last seen** is the file as it was when you last
looked at it: Fathomable snapshots a file when you switch away, quit,
comment on it, or leave it alone for five seconds. It never drives the bar
or `]g`; `Space d D` (or `:diff seen`) shows it in the diff view as `last seen ·
now`, badge `DIFF seen`, and `Space d D` again returns to the file. `Space d s`
marks every non-ignored text file seen at once (a toast counts the
new snapshots, `seen: 12 files`), so a later `Space d D` shows only what came
after. Snapshots
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
workspace-wide ones. The diff's keys sit on the text's key bar, the
bottom text row, while the text has focus: `h/l page` on a checkpoint
base, then `b base · t target · D next diff · w whitespace · Esc
close`; the header is the pair's names alone. With no checkpoint the view says so and names
`Space d c`; `Space d r` again leaves it.

Every diff is the same view with two **sides**, so its keys work in all
of them. `D` steps through the layers the file has: unstaged
(`INDEX · now`) first, staged (`HEAD · INDEX`) second, then last seen,
the newest checkpoint against the working file, and the file again.
Unavailable pairs are skipped; the aggregate `HEAD · now` pair is
available explicitly through `Space d d`. From a pair off the cycle,
`D` returns to the file. `b` and `t` pick the base or target from the
file's checkpoints, the working file, last seen, `HEAD`, `INDEX`, and the commits that
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
[0060](decisions/0060-one-diff-two-sides.md),
[0069](decisions/0069-the-diffs-keys-on-the-bar.md)).

## 6. Following changes

Any file written under the workspace (ignoring what git ignores) becomes a
*change*: a `●` next to it in the tree (and on collapsed folders above it),
a toast in the bottom-right corner for a few seconds, and a hint in the
status line, `→ src/foo.rs +12 -3 (3)`, naming the newest change and how
many are pending. `]f` and `[f` step through the changed files, newest
first; `Space j j` jumps straight to the newest. A jump lands on the first
hunk against `HEAD` (outside git, the first hunk against the last-seen
snapshot), and a change is forgotten once its target is on screen.
`Alt-Left` returns after a jump. Fathomable never automatically switches the
displayed file or scrolls to an agent edit: live reload preserves the current
reading position, and moving among changed files is always a human action.
File-renaming and thread-re-anchoring behavior described above still
preserves the document and discussion the reader already has open.

## 7. Configuration and themes

Configuration is optional KDL at `$XDG_CONFIG_HOME/fathomable/config.kdl`
(`~/.config/fathomable/config.kdl`):

```kdl
theme "default-light"

jump {
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

layout {
    menu-bar #true          // reserve the top application-menu row
    sidebar {
        visible #true       // show the configured panes at startup
        files #true         // include the Files pane when shown
        threads #true       // include the Threads pane when shown
        width 32            // columns for the sidebar
        split 8             // rows the Threads pane keeps under Files
    }
}

threads {
    stubs #true             // a stub under each thread's lines
    stubs-resolved #false   // resolved threads get one too
}

diff {
    context 3               // unchanged lines shown around each hunk
    ignore-whitespace #false // start with whitespace ignored (Space d w)
}

user {
    name "User"             // how your messages are signed, to you and to agents
}
```

Every key is optional; the values above are the defaults and
`--config-show` prints the effective ones.

Old automatic-jump and agent-runtime settings are unknown keys, not
compatibility aliases. `jump.toast` remains the change-toast duration; the
exact manual cleanup is listed in [Review workflow and migration](#review-workflow-and-migration).

Built-in themes are `default-dark` and `default-light`. Drop your own at
`~/.config/fathomable/themes/<name>.kdl`; it can `inherits` a built-in and
override only the keys it wants. `--theme NAME` selects one for a single
run; `--config PATH` points at another config file. The theme file shape
and the key vocabulary are in
[0011](decisions/0011-theme-schema.md). A theme's `code.syntect` picks one
of syntect's bundled themes for code colours (`--doctor` lists them); only
their foreground colours are used, so a transparent background stays
transparent ([0016](decisions/0016-syntax-highlighting.md)).
View keymap, pickers, Status, Doctor, and About use `ui.popup`; the menu
bar, its drop-downs, `Space` prefix menus, and right-click menus use
`ui.menu`. Every visual popup has a rounded titled border. Menu shortcut
columns and borders use the subdued `ui.statusline.info` face. The built-ins
give the two surfaces the same
overlay treatment, while custom and inherited themes may separate them
or leave either transparent. View keymap uses the regular `ui.popup.key`
key face; help and menus use `ui.list.hover` background for hover, without
a cursor bar. Help groups use their active overlay foreground in bold;
View keymap uses `ui.picker.match` for
its filter and the subdued info face for its footer.

List selection uses four shared theme roles: `ui.list.active` background,
`ui.list.inactive` background, `ui.list.cursor` foreground, and
`ui.list.hover` background. Both built-ins distinguish these from
`ui.header`; the exact colours are in
[0011](decisions/0011-theme-schema.md#selection-and-built-ins).
Review message backgrounds keep `thread.user` and `thread.agent` stripes.

**Theme migration:** remove `ui.sidebar.selected` and `ui.picker.selected`
from custom theme files, including any parent theme you maintain. They
are unknown-key errors, not aliases. Inherit the built-in shared defaults,
or move your selected background to `ui.list.active` and choose a quieter
`ui.list.inactive` background, a distinct `ui.list.cursor` foreground, and
a subtle `ui.list.hover` background. Keep `ui.picker.match`,
`ui.sidebar`, and `ui.sidebar.dir`; no navigation settings change.

## 8. Connect an agent

A repository's threads are shared by its worktrees
([0070](decisions/0070-one-workspace-many-worktrees.md)).
`fathomable --mcp [DIR]` is a stdio MCP server bound to the repository
checkout containing `DIR`, or to its startup directory when omitted. The
binding never changes, and no tool takes a workspace or viewer selector.
Reading and writing threads works without a viewer running.
The server discovers the checkout directly; it requires no workspace-marker
setup. Git state uses the repository's common-directory key directly, while
non-Git state uses its root; startup never adopts or moves an older key.
Starting the MCP server may persist current annotation housekeeping
once—offline snapshot/context re-anchoring for current records and
stranded-open follow-HEAD rescoping—before it accepts calls. It does not
backfill context onto historical records. A `threads` call itself reads the
shared store directly and computes current placement without persisting
changes. Exact `ids` use the same direct store, bypassing ordinary checkout
and status visibility as described below.

The viewer and MCP child must run matching builds. The internal socket accepts
protocol version **5** only. An “unsupported protocol version” error means
one process is stale: stop and restart the affected viewer and MCP child,
then reconnect the host if needed. Deleting annotation state does not repair
a process mismatch, and ordinary startup never deletes state.

Register it with your agent host once. Every host runs the same stdio
command, `fathomable --mcp`; only the host configuration differs. Register
it per user, not in the repository. To bind it explicitly, append the
checkout: `fathomable --mcp /path/to/repository` (JSON/TOML arguments:
`["--mcp", "/path/to/repository"]`). A host that starts one shared server
outside the intended checkout must configure an explicit directory; there
is no cross-workspace routing fallback.

Claude Code:

```sh
claude mcp add --scope user fathomable -- fathomable --mcp
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

`"stdio"` is accepted there as a synonym for `"local"`; `tools` may be
`["threads", "thread_start", "thread_reply"]` instead of `"*"`.

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

The viewer and MCP server are independent: start the viewer when the human
wants the terminal interface, and connect MCP when a chat needs the
discussion. The tools are:

| Tool | Use |
| --- | --- |
| `threads` | Read open discussions regardless of who spoke last, with their actual history, authors, and current placement. Filter by `status` (`open`, `resolved`, or `all`), `path`, or `since`; use `ids` alone for exact discussions, including older resolved history. |
| `thread_start` | Start discussions with a non-empty `comments` array of `{path, line?, end_line?, body}`. Paths are relative to the bound checkout; omit `line` for a file-wide comment. |
| `thread_reply` | Continue discussions with a non-empty `replies` array of `{thread, body, resolve?, line?, end_line?}`. A range re-anchors rewritten lines; `resolve` proposes closure, and only the human closes or reopens a thread. |

Reads include discussions reached by any current worktree of this repository.
When the relevant checkout differs from the bound one, the result names it in
`worktree`; exact-ID reads retain the same placement. Replies use that
discussion's checkout without moving a viewer. A `path` filter must name a
file or directory in the repository's worktrees; directories include their
subtrees.

Listings are ordered by `(updated, id)`, with `limit` defaulting to 50
(zero means 1). The response is `{threads, more, next_after}`. Continue
with the returned `next_after` object as `after`, retaining the same filters;
the pair is exclusive, so equal timestamps do not trap pagination.
`since` is an inclusive update-time filter, not a page cursor.
`ids` preserves request order and cannot be combined with other selectors;
duplicate or missing IDs fail.

Both write tools reject invalid batches before writing any item: empty
bodies, unknown fields, invalid 1-based ranges, or an `end_line` without
`line`. Replies also reject duplicate or missing IDs, resolved threads,
range overrides on file-wide threads, and detached threads without a new
line. Prevalidation is not an I/O transaction: a later runtime failure can
leave earlier items written, and the error names those completed items.
See [0082](decisions/0082-three-tool-review-core.md) for the full contract.

There are no top-level single-comment or single-reply arguments and no
compatibility aliases. There is no `open`, `workspaces`, `follow`, or
`thread_watch` tool, and no per-call workspace or viewer routing.

Threads remain commit-aware: unrelated work may hide a discussion and merging
brings it into reach ([0024](decisions/0024-workspace-sessions.md)); an open
thread survives a rewrite while its lines remain
([0035](decisions/0035-threads-follow-head.md)); resolved history follows
[0072](decisions/0072-a-resolved-thread-stays-at-its-commit.md). Worktrees
of one repository share the thread store, while this MCP server stays bound
to its configured checkout.

### Automatic chat identity

No tool takes a caller id. Fathomable selects the adapter for the MCP
client's harness and reads only that harness's channel:

| Harness | Required channel | Stored identity |
| --- | --- | --- |
| Copilot CLI | MCP launch environment `COPILOT_AGENT_SESSION_ID` | `copilot:<native>` |
| Claude Code | MCP launch environment `CLAUDE_CODE_SESSION_ID` | `claude:<native>` |
| VS Code | per-call `params._meta["vscode.conversationId"]` | `vscode:<conversation>` |
| Codex | per-call `params._meta.sessionId` and `params._meta.threadId` | `codex:<thread>` |

Codex requires both fields. `params._meta.sessionId` describes the
root/family, not the concrete resumable chat; only `threadId` keys the
stored identity. Each thread keeps its own identity even under the same
root. A missing thread identifier cannot fall back to the root, and
either field missing means identity is unavailable.
An inherited environment variable from a different harness is ignored.
These channels need no native extension and no identity text injected
into the prompt when connecting.

A new agent author stores the stable harness label as `name`, the raw MCP
client as `client`, and the harness-qualified chat id as `id`; no role, type,
or persona is registered. On the wire the user author is the string
`"user"`; every attributed agent is one object shape,
`{name, client?, id?}`. There is no bare-agent string and no `kind` field.
New MCP writes include `client` and `id`. Human labels use the configured
user name; agent labels use the stored name, or `name (client)` when the
observed client is shown, never a subscription type. Replies retain
`author`, `created`, `body`, and optional `proposed_resolved` and `edited`;
a proposal does not change the thread's open status.

Identity records authorship only. It does not select a repository, register a
role, create a subscription, or establish delivery. Unknown clients and calls
missing the required channel may use `threads`; `thread_start` and
`thread_reply` fail with an error naming the expected harness channel.
Malformed identity metadata on a write is rejected rather than treated as missing.
Tools accept no manual id, type, role, or persona.

**Harness lifecycle matters.** Copilot CLI 1.0.84-8 was tested with
distinct MCP children for separate top-level chats and a shared process
for subagents; that is not a guarantee for every older release.
Copilot and Claude subagents sharing the parent's identity are an
accepted limitation.

For Claude, start a **fresh invocation** for a distinct Fathomable chat
identity. To return to a particular chat, start a fresh
`claude --resume <id>`. `/clear` and in-process resume do not refresh the
MCP child's launch identity and are unsupported as distinct identity
boundaries. Implicit `--continue` or `--resume` without a specified id
is not a guarantee of the correct launch identity.

Codex requires a version that emits the metadata above; source was
inspected at v0.155-alpha, not all stable versions validated. VS Code
likewise needs its conversation metadata to survive any gateway.
These are capability requirements, not blanket version guarantees.
The reasoning and limits are in
[0080](decisions/0080-automatic-chat-identity.md).

The current format does not preserve or read historical subscription
profiles. The inert `agents.jsonl` register has no reader and is discarded
only by the explicitly authorized operator reset, never by ordinary startup.
Do not delete `threads.jsonl` during ordinary startup or upgrade; the
operator reset is the separate clean-slate action that discards app-owned
state.

### Review workflow and migration

Fathomable does not push, assign, or consume discussions. Tell an agent when
to read them and what job it has. A useful handoff is:

> Read the relevant Fathomable threads and their history. The user decided
> [decision in the user's words]. Implement or defend only that scope, and
> reply where useful. Treat proposals as proposals. Reading a thread does not
> authorize other changes, and only the user closes threads.

A reviewer may start findings, the human may discuss them, and a coder may
later read the same actual conversation. An answer by one agent does not hide
the thread from another or mark it handled. There is no structured approval
state: the user's prompt and thread replies carry the decision.

The server itself gives agents this short instruction:

> Fathomable holds review discussions attached to files in this repository.
> When asked, read the relevant threads and their history. Use thread_start
> for new findings or questions and thread_reply to continue existing
> discussions. Treat proposals as proposals; follow the user's stated
> decisions and your assigned task. Reading a thread does not authorize
> changes. Only the user closes threads.

`scripts/demo-repo.sh` (`just demo`) builds an isolated throwaway
repository and seeds discussion threads through the hidden `fathomable seed`
subcommand. It never adds hooks, subscriptions, or watches, and always puts
its XDG state and configuration under the demo directory.

**Remove old integration manually.** The upgrade does not edit installed
files. Remove every old `fathomable pending` and `fathomable hello` entry
from Claude, Codex, Copilot, or VS Code hook configuration. Remove the
obsolete `agents` config node and `jump.auto` / `jump.debounce`; keep
`jump.toast` if customized. These hooks and config entries require manual
cleanup. `agents.jsonl` has no reader and is discarded only by the operator
reset; ordinary startup does not remove it or any thread store.

`Space a w` is reserved for a possible user-directed integration and now
always reports `Wake agent is not yet implemented`. A future design may send
all open threads plus an optional user instruction to a chosen chat, but the
selection and transport mechanism are not decided and hooks alone are not
treated as a reliable idle-agent wake.

## 9. When something is off

```sh
fathomable --doctor        # terminal, directories, config, log locations
fathomable --viewers       # known workspaces and their viewer records
fathomable --config-show   # effective configuration
```

Each run logs JSON lines to `$XDG_STATE_HOME/fathomable/log/<session-id>.log`;
`:doctor` opens a fresh, scrollable rendering of the same checks as
`fathomable --doctor`; use `j`/`k`, PgUp/PgDn, Home/End, or the wheel,
`r` to rerun, and Esc to close. `:status` inside the app shows the open
document, the terminal size, the
viewer name and id, the worktrees with the active one marked, the
socket, and every state path. Set
`FATHOMABLE_LOG=debug` for more. A viewer killed without a clean quit is
swept away by the next start. `--viewers` groups viewers by workspace
and, when there are several, lists its worktrees and says which one
each viewer shows; `--doctor` counts the worktrees sharing the state and
the visible directories that consume inotify watches. Ignored trees are
not part of that watch budget.
Git state is keyed directly by the repository's common directory and
non-Git state by its root; no old root-keyed directory is adopted or moved
on startup
([0070](decisions/0070-one-workspace-many-worktrees.md)).

If Fathomable dies, it hands the terminal back and prints one block between
two rules: what it was showing, where its state lives, and a backtrace with
the runtime plumbing dropped. Paste that block at your agent. A copy is
written to `$XDG_STATE_HOME/fathomable/log/<session-id>.crash`, so a report
that has scrolled away is still there; `--doctor` counts what is waiting
there ([0022](decisions/0022-crash-reports.md)).
