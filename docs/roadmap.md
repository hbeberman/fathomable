---
type: Concept
title: Roadmap
description: Ordered milestones toward the charter, each small enough to ship and use.
tags:
  - charter
---

# Roadmap

Milestones are ordered; each is usable on its own. Details live in the
[decisions](decisions/index.md); deferred work lives in
[parked ideas](parked.md).

1. **Single-file Markdown view.** `fathomable README.md` renders Markdown
   with the layout engine ([0004](decisions/0004-markdown-rendering.md)),
   live-reloads on change preserving position, Vim scrolling and `/` search,
   bottom-line status, `--doctor` and file logging
   ([0009](decisions/0009-cli-and-diagnostics.md)). Default light and dark
   themes.
2. **Workspace mode.** Toggleable tree sidebar plus a fuzzy file picker,
   `fathomable [DIR]`, session records and socket
   ([0003](decisions/0003-sessions-and-mcp.md),
   [0012](decisions/0012-workspace-mode.md)).
3. **Annotations.** Mouse drag and visual selection, comment box, JSONL
   threads, anchors ([0005](decisions/0005-annotations.md)).
4. **MCP server.** `fathomable --mcp` with session tools, `open`, `follow`,
   `annotations_list`, `thread_reply`.
5. **Git.** Gutter strip, HEAD and last-seen diff views, hunk navigation
   ([0006](decisions/0006-git-access.md)).
6. **Follow mode.** Change hints, badges, toasts, jump keys, debounced
   auto-jump, and the last-seen diff base
   ([0015](decisions/0015-follow-mode.md)).
7. **Syntax highlighting.** `syntect` for fenced code blocks and whole
   source files, plus the Markdown file list
   ([0016](decisions/0016-syntax-highlighting.md)).
8. **Git status navigation.** Dirty set, staged/unstaged gutter, `]g`
   across files, sidebar git marks
   ([0017](decisions/0017-git-status-navigation.md)).
9. **Comment editor.** A cursor-bearing buffer in core, motion and
   deletion keys, bracketed paste, click-to-place, a draggable box, and
   the `$EDITOR` hatch ([0018](decisions/0018-comment-editor.md)).
10. **Re-anchoring edited lines.** Threads follow a local rewrite of their
    lines through the reload diff, read as *edited* until the user answers,
    and persist the move ([0019](decisions/0019-reanchoring-edited-lines.md)).
11. **Re-anchoring across restarts.** Threads edited while Fathomable was
    closed are followed on start through the last-seen snapshot, which is
    pinned while the file has open threads and taken on annotate
    ([0020](decisions/0020-reanchoring-across-restarts.md)).
12. **Polish pass.** One change source, `gd`/`gD` as independent
    toggles, a `:status` overlay in place of status-line clutter, one key
    per job, dead flags gone, and app construction through `Options`
    ([0021](decisions/0021-polish-pass.md)).
13. **Crash reports.** A panic hands the terminal back, then prints one
    pasteable block — what the viewer was showing, where its state lives,
    and a backtrace with the plumbing dropped — and leaves a copy beside
    the session log ([0022](decisions/0022-crash-reports.md)).
14. **Sidebar paging.** The tree highlight is the file the main pane
    shows: moving it pages the viewer without taking focus, the sidebar
    wheel steps one row per tick, and `Enter` still commits focus
    ([0023](decisions/0023-sidebar-paging.md)).
15. **Workspace sessions.** A session is a workspace's annotation state;
    viewers are named windows onto it that agents broadcast to or target,
    `--mcp` binds per call and works headless against the store, and a
    thread belongs to the commit it was written against, shown only when
    that commit is reachable from HEAD
    ([0024](decisions/0024-workspace-sessions.md)).
16. **The thread list.** `Space A` shows every thread on the current
    work in place of the document: open then resolved, grouped by file,
    each in full; `Enter` jumps to one, `r` and `x` act in place, and the
    file-scoped picker is gone
    ([0025](decisions/0025-thread-list.md)).
17. **Binary files and the file-info pane.** A file is binary when git
    would say so — the `diff` attribute, else a `NUL` in its first 8000
    bytes; opening one, or a text file over `viewer.max-file-size-mib`,
    shows a file-info pane with its format, sizes, and git state, and
    the sidebar tags it `bin`
    ([0026](decisions/0026-binary-files-and-file-info.md)).
18. **Revisiting threads.** `c` on a thread opens it and `C` always
    starts one; the thread pane's `h`/`l` page threads, `j`/`k` select
    messages, `Tab` switches independently remembered local/global
    selections, and `e` edits a user message; the gutter brackets a
    thread's rows with `╭ │ ╰` and dots a one-row thread; and a
    file-threads pane under the tree lists the file's threads, open and
    resolved, highlighting the one under the cursor
    ([0027](decisions/0027-revisiting-threads.md)).
19. **Live workspace.** The tree follows the agent creating, deleting,
    and renaming files; a deleted open file keeps its content under a
    banner; a rename carries the file's threads and is recorded in the
    store ([0028](decisions/0028-live-workspace.md)).
20. **Horizontal scroll.** Unwrapped code lines scroll sideways with
    `zl`/`zh`/`zL`/`zH` and the horizontal wheel, with edge markers and
    search keeping its match in view
    ([0029](decisions/0029-horizontal-scroll.md)).
21. **Waiting threads.** An open thread whose newest message is an
    agent's is *waiting*: its own gutter colour, a status-line count, a
    toast when a reply lands, and `]r`/`[r` to step through them across
    files ([0030](decisions/0030-waiting-threads.md)).
22. **Lazy follow.** Auto-jump switches itself off when the reader
    navigates elsewhere, keeps still while the visible file's hunk is on
    screen, and prefers the file the agent says it is editing
    ([0031](decisions/0031-lazy-follow.md)).
23. **Placement and state.** The thread pane and file-threads pane say
    where a thread's lines went and what state it is in as two words, an
    agent reply toasts in the viewer it came through, and the store
    appends each event in one write
    ([0032](decisions/0032-placement-and-state.md)).
24. **Open thread lines.** The rows of the thread shown in the pane draw
    in `annotation.focus`, and `thread_reply` takes `line`/`end_line` so
    an agent that rewrote the lines re-anchors the thread as it answers
    ([0033](decisions/0033-open-thread-lines.md)).
25. **Deleting threads.** `d d` deletes a thread from the thread pane,
    the file-threads pane, or the thread list, as a tombstone in the
    store; the file-threads pane drives the thread pane without taking
    its focus, and the pane opens at its end under a dim `END` row
    ([0034](decisions/0034-deleting-threads.md)).
26. **Threads follow HEAD.** An open thread whose commit an amend,
    squash, or rebase dropped is rescoped to the new `HEAD` while its
    lines are still in the working tree, so committing as you go keeps
    the review ([0035](decisions/0035-threads-follow-head.md)).
27. **Gutter rows and focus colour.** The note cell brackets a thread
    across the rendered rows of a wrapped line instead of dotting each,
    and the open thread's lines take a blue tint distinct from the
    yellow of other annotated lines
    ([0036](decisions/0036-gutter-rows-and-focus-colour.md)).
28. **Markdown in the thread pane.** Comment and reply bodies render as
    Markdown with fenced code coloured by its language, and a newline in
    a comment stays a line break
    ([0037](decisions/0037-markdown-in-threads.md)).
29. **Re-anchoring without a snapshot.** Each thread stores a window of
    the text it was last placed in — its lines and three either side —
    and a thread edited offline on a file with no last-seen snapshot is
    followed through that window; older threads are backfilled on start
    ([0038](decisions/0038-reanchoring-without-a-snapshot.md)).
30. **Gutter colour and detached rows.** The note cell's colour is the
    thread's status alone (amber open, teal waiting, grey resolved), a
    detached thread draws on a blank row inserted where its lines were,
    and a bracket bridges the blank rows of rendered markdown
    ([0039](decisions/0039-gutter-colour-and-detached-rows.md)).
31. **Agent subscriptions and hooks.** An agent registers once with
    `follow(id, type)`, a harness stop hook delivers each thread whose
    newest message is someone else's exactly once as a self-contained
    prompt, `thread_watch` wakes it when another thread moves, and an
    unsubscribed session never hears from Fathomable
    ([0040](decisions/0040-agent-subscriptions-and-hooks.md)).
32. **Session bonds.** The `hello` hook records the young ancestors of
    its process under the session id; the MCP server signs as the
    session of the nearest unique one among its own ancestors, so
    replies stay signed after a resume; `thread_reply` takes an `id`
    as the fallback ([0041](decisions/0041-session-bonds.md)).
33. **Delivery at both ends of a turn.** `fathomable pending` also runs
    from the prompt-submit hook, adding undelivered threads to context
    and exiting 0, so a comment posted while an agent waits is there on
    the wake and no agent polls for comments
    ([0042](decisions/0042-turn-start-delivery.md)).
34. **One vocabulary for the agent-facing text.** Tool and parameter
    names live in one core table that every hook string, the server
    instructions, and the guide are checked against; the hello body is
    shared with a per-harness spelling of tool names; the `follow`
    schema lists the configured types
    ([0043](decisions/0043-agent-vocabulary.md)).
35. **Width-bounded wrapping.** Prose wraps at words and code, source,
    diff, and narrow table lines hard-wrap to the pane; horizontal-scroll
    state, keys, wheel handling, edge markers, and status are removed
    ([0044](decisions/0044-wrap-all-lines.md)).
36. **Bindings are data.** Every key is a row of one table that dispatch,
    the prefix menus, `Space ?`, the pane hints, and the guide's key
    section are read from or checked against; the picker moves on
    `Ctrl-j`/`Ctrl-k` ([0045](decisions/0045-bindings-are-data.md)).
37. **One thread cursor.** The thread pane, the file-threads pane, and
    the thread list show and move one thread-and-message cursor that
    rides the text cursor once the reader moves on; `]c`/`[c` and
    `l`/`h` step within the file, `]C`/`[C` and `L`/`H` across the
    workspace; `o` resolves; `Ctrl-d`/`Ctrl-u`, `gg`/`G`, and
    `Alt-j`/`Alt-k` page without `PgUp`/`PgDn`
    ([0046](decisions/0046-one-thread-cursor.md)).
38. **One vocabulary for the viewer.** Workspace, viewer, agent session,
    thread, `ThreadState`, reach, coverage, follow versus auto-jump, and
    subscribe are the words everywhere; the code, screen, config, CLI,
    wire, theme, and docs are renamed to match, with one release of
    compatibility for the flag, the record directory, and the theme keys
    ([0047](decisions/0047-one-vocabulary.md)).
39. **Modules by concept.** `app/` regroups into `threads/`, `draw/`,
    `input/`, `jump`, `agents`, and `run`; records point at the concept
    module they changed through `related_resources`, which need no
    backlink ([0048](decisions/0048-modules-by-concept.md)).
40. **Inline threads, the rail, checkpoints, and the jumplist.** A
    thread shows under its lines as a two-row stub that `c` expands in
    place and the bottom thread pane goes; the left column is the rail,
    a tree pane above a threads pane at a fixed split; `Space A` is the
    review list, newest agent reply first with resolved hidden;
    `Space v c`/`C` checkpoint a file or the workspace onto per-file
    timelines that `Space v r` pages through beside git diff;
    `Alt-Left`/`Alt-Right` walk a jumplist of positions; the leader
    gains `c`, `v`, and `r` submenus and the menu a breadcrumb row
    ([0049](decisions/0049-inline-threads-and-the-rail.md)).
41. **Mouse menus and gestures.** A right-click opens a context menu of
    the actions that apply under the pointer, in the text, the tree, the
    threads pane, and the review list, each entry showing its key; the
    `Space` menu and `Space ?` take clicks; the gutter, double- and
    triple-click, and Shift-click select; pane-header hints take
    clicks; `gy`/`gx` copy or open a link
    ([0050](decisions/0050-mouse-menus-and-gestures.md)).
42. **Retire the one-release compatibility.** The `annotation.*` and
    `ui.sidebar*` theme keys, the hidden `--sessions` flag, and the sweep
    of the old `sessions/` directory go before any release exists; the
    socket op `annotations_list` is `threads_list`
    ([0051](decisions/0051-retire-one-release-compatibility.md)).
43. **File references open in the viewer.** `gf`, a Ctrl-click, and the
    context menu open the file a Markdown link or a bare `path:line`
    under the cursor names, at that line, in the viewer; a scheme keeps
    a link external for `gx`; `Alt-Left` is the way back
    ([0052](decisions/0052-goto-file.md)).
44. **Resolution is the user's.** An agent's `resolve` proposes closing a
    thread and nothing more: the thread stays open and waiting, its
    headers read `waiting · proposed`, the status line and the review
    list count proposals, and `o` closes it; the agent force-resolve
    and the `auto-resolved` state go
    ([0053](decisions/0053-resolution-is-the-users.md)).
45. **The draft is written in the thread.** A reply is written at the
    bottom of its thread's expanded rows, an edit in place of the
    message it edits, and a new comment in a draft block under its
    lines; the review list opens the file to write and comes back
    after; the bottom comment box, its cap, and its drag go
    ([0054](decisions/0054-the-draft-is-written-in-the-thread.md)).
