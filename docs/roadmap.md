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
46. **Six tools.** The agent surface is `workspaces`, `follow`,
    `threads`, `thread_reply`, `thread_watch`, and `open`; a
    subscription covers the whole workspace and `follow` takes no
    paths; `threads` lists open threads by default, delivers the ones
    waiting on the caller, and widens to resolved ones; every thread an
    agent sees carries its placement and no anchor hashes;
    `thread_reply` answers with the updated thread and refuses a
    detached thread without a line; failures name the call that fixes
    them ([0055](decisions/0055-six-tools.md)).
47. **The leader, trimmed.** The `Space` menu drops the tree actions,
    the change-queue clear, and `Space c n`; `Space w` is Helix's
    window submenu (`h j k l w`) and `Space w w` cycles the panes;
    unmatched continuations cancel without a menu entry;
    `Space p f` and `Space p t` toggle the files pane and the threads
    pane; the review list is `Space r`, the rarer pickers `Space F i`
    and `Space F r`, wake `Space a w`, new thread `Space c c`; labels
    are a few words; the tree pane is the files pane; the menu draws
    on `ui.menu` ([0056](decisions/0056-the-leader-trimmed.md)).
48. **The sidebar.** The left column is the sidebar, the word it had
    before 0049; the `rail` config node is `sidebar`, the `ui.rail*`
    theme keys are `ui.sidebar*` with no old spelling accepted, the
    column's state has its own module, and the files pane's module is
    `app/files_pane.rs` ([0057](decisions/0057-the-sidebar.md)).
49. **The user has the last word.** A thread is pending only while the
    user's act — comment, reply, edit, or reopen — is its newest, so an
    answer from any session leaves it waiting on the user; `threads`
    says `answered by` or `proposed by`, takes `status "pending"`, and
    marks edited messages; an agent is named once at `follow` from its
    harness, `thread_reply` loses `persona`, and the user is named by
    `user { name }`, `User` by default
    ([0058](decisions/0058-the-user-has-the-last-word.md)).
50. **Headers and the key bar.** Every pane header and the review
    list's entry headers draw on the new `ui.header` surface; the review
    list's keys move from its header to a bar along its bottom row so
    they survive a half-width terminal; the header's counts are dotted
    with the sort word at the right edge, where a click switches the
    sort; a stub's age reads in the info colour; the `proposed` wording
    stays ([0059](decisions/0059-headers-and-the-key-bar.md)).
51. **One diff, two sides.** The `HEAD`, last-seen, and checkpoint diffs
    are one diff view with a base and a target, each with the pair
    header and the `b` / `t` pickers; the badge reads `DIFF` and the
    base; `Space d` holds the comparisons, the checkpoint marks, and a
    whitespace toggle backed by `diff { context; ignore-whitespace }`;
    `Space v` is source view and the stub toggles
    ([0060](decisions/0060-one-diff-two-sides.md)).
52. **Agents start threads.** A seventh tool, `thread_start`, opens a
    thread on a line range of a file, one or several per call, signed
    and stamped with `HEAD` as a reply and a user's comment are; a
    thread records its comment's author, so an agent's thread waits on
    the user from birth and reaches no agent until the user speaks;
    the viewer names the author on the comment's rows
    ([0061](decisions/0061-agents-start-threads.md)).
53. **One version, no compatibility.** The socket drops `ping` and
    `session_info`, refuses every protocol version but its own, and
    restarts at v1; the thread store and the agent register read the
    format version they write and refuse another, naming the file to
    delete; a `follow` config block is an unknown setting; nothing
    written before the first tag is owed a reader
    ([0062](decisions/0062-one-version-no-compatibility.md)).
54. **A comment on the file.** A thread may be on a file as a whole:
    `Space c f` starts one, `thread_start` without `line` starts one,
    the record carries no range, anchor, or snippet, the viewer shows
    it as a stub above the first line with `file` as its placement
    word, and every list names it by its path alone
    ([0063](decisions/0063-a-comment-on-the-file.md)).
55. **Hints you can press.** A key hint is drawn only where pressing
    the key now runs the action it names: the thread header's keys on
    the thread cursor's thread while the text has focus, `e edit` on
    the user's own message, the diff header's keys and `(c expand)` on
    the text alone; a header row inside a thread block paints its
    gutter cells too ([0064](decisions/0064-hints-you-can-press.md)).
56. **z folds and unfolds.** In the text `z` folds the expanded thread
    the cursor is on or expands the thread cursor's stub, and does
    nothing else; `Z` expands every thread in the file or folds them
    all when any is expanded; `c` keeps its cycle and its comment, and
    the fold hints name `z`
    ([0065](decisions/0065-z-folds-and-unfolds.md)).
57. **One circle language.** Every surface draws one circle in the
    state colour (`●` open or waiting, `◐` proposed, `○` resolved, `?`
    lines gone; `•`, `✓`, and `↩` retire); the threads pane lists two
    rows per thread grouped by file in the files pane's order, `z`
    folds a file and `Z` every file, its header counts by colour and
    its keys sit on a bar while it has focus; the review list is the
    same view full screen with `s` gone; the files pane, the status
    line, and the file rows take clicks and menus
    ([0066](decisions/0066-one-circle-language.md)).
58. **The text's key bar.** A key bar on `ui.header` replaces the
    bottom text row while it has something to say, carrying the thread
    cursor's keys, the draft's keys, and `Z` for the file, or the focus
    tip, the text never moving for it; the thread
    header, the stub, the draft's author row, and the review list's
    entry header give up their keys, the cursor's stub reads bold, the
    diff header alone keeps its keys, and `ui.hint` retires
    ([0067](decisions/0067-the-texts-key-bar.md)).
59. **What the files pane shows.** Three session toggles under `Space F`
    filter the files pane, only changed files (`c`), hide untracked
    files (`u`), show ignored files (`g`), from any pane; the popup's
    entries and the right-click menu say what a press does now; the
    pane's header row moves onto `ui.header` and names the active
    filters after the repo's counts
    ([0068](decisions/0068-what-the-files-pane-shows.md)).
60. **The diff's keys on the bar.** The diff header is its words and its
    keys move to the text's key bar, `h/l page` drawn on a checkpoint
    base only; `D` steps the diff through `HEAD`, last seen, the newest
    checkpoint, and the file; `Space d s` marks every file seen
    ([0069](decisions/0069-the-diffs-keys-on-the-bar.md)).
61. **One workspace, many worktrees.** A git workspace is keyed by its
    common dir and every worktree of it shares one store; the viewer
    lists the worktrees itself, `]w` / `[w` page through them, the
    files pane header names the active one, and a thread from any
    worktree's branch shows everywhere with the branch on its entry;
    the tools and hooks resolve a worktree root as they resolve a root
    ([0070](decisions/0070-one-workspace-many-worktrees.md)).
62. **Author stripes and the cursor bar.** A message's rows sit on a
    faint stripe of its author's kind, blue for the user and green for
    agents, the name in the same hue; the thread cursor is a `▎` bar
    down its message and on the thread's header instead of the
    selected surface; the expanded thread has a two-cell gutter of
    its own in the file; a draft is written on a warm surface
    ([0071](decisions/0071-author-stripes.md)).
63. **A resolved thread stays at its commit.** Resolving fixes a thread
    to the commit that is `HEAD`, and a resolved thread shows in the
    file, the tools, and the hooks only while that commit is `HEAD`;
    the review list and the threads pane list earlier commits' resolved
    threads under `x` with the commit named, where `o` brings one back
    ([0072](decisions/0072-a-resolved-thread-stays-at-its-commit.md)).
64. **The chevron.** An expanded thread's header draws a `▾` in the
    thread's gutter and a stub a `▸` in the same column; a click on
    either, or a double-click on the row, folds or expands the thread,
    and one click elsewhere only places the cursor
    ([0073](decisions/0073-the-chevron.md)).
65. **The bracket marks the focused thread.** Annotated rows carry no
    tint; the gutter cells that draw the focused thread's bracket sit
    on a brighter yellow (`thread.bracket`), and `thread.line` is gone
    ([0074](decisions/0074-the-bracket-marks-the-focused-thread.md)).
66. **The header names its counts.** The review list is `review
    threads`, each count on it and on the threads pane carries a word
    (`● 2 user ● 3 agent ◐ 1 resolve? ○ 1 resolved`), a proposed thread
    counts on its own, and the words drop together when the row is
    too narrow ([0075](decisions/0075-the-header-names-its-counts.md)).
67. **Threads fold in the list.** Every thread in the review list folds
    and expands with the text's chevrons, `z`, `Z`, clicks, and menu
    entries; a folded thread is one packed row; file rows and folded
    threads are stops for `j`/`k`, `z` acts on the row the cursor is
    on, and `Z` folds or expands every thread; file rows always draw
    `▾` or `▸`; and a stub is a stop in the text
    ([0076](decisions/0076-threads-fold-in-the-list.md)).
68. **Threads nest under their file.** In the review list and the
    threads pane a thread's rows sit two cells in under the file row,
    one level as the files pane nests, in file scope too; and the
    list's file row draws the cursor bar while the cursor is on one of
    its threads ([0077](decisions/0077-threads-nest-under-their-file.md)).
69. **All keys stays reachable.** `Space ?` is a compact grouped action
    list: two columns at an ordinary terminal, one when narrow, with
    wrapped descriptions, keyboard and wheel scrolling, key-and-word
    filtering, no-match feedback, current-layout click targets, and the
    built-ins' shared overlay treatment through separate popup and menu
    roles; every row still comes from the binding table
    ([0078](decisions/0078-all-keys-stays-reachable.md)).
70. **Shared list focus.** Files, threads, review entries, and every
    picker share a blue active tint and bright edge bar, with a quieter
    remembered tint while another surface owns the keys. Headers stay
    neutral, help and menus use hover alone, and thread-state colours,
    author stripes, and muted ancestor-file context remain independent; four
    `ui.list.*` roles replace the old sidebar and picker selection keys
    ([0079](decisions/0079-list-focus-language.md)).
