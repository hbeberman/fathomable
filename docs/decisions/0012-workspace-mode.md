---
type: Decision
title: Workspace mode
description: The tree sidebar, the space menu, the fuzzy file picker, open-file history, and the first session record and socket.
resource: crates/fathomable/src/app/mod.rs
related_resources:
  - crates/fathomable/src/app/draw/mod.rs
  - crates/fathomable/src/app/run.rs
  - crates/fathomable/src/app/background.rs
  - crates/fathomable/src/app/file_index.rs
  - crates/fathomable-core/src/workspace.rs
  - crates/fathomable-core/src/tree.rs
  - crates/fathomable-core/src/picker.rs
tags:
  - decision
  - input
  - sessions
---

# 0012 Workspace mode

Status: accepted (2026-08-26); amended 2026-09-06 (ignore rules reload);
amended 2026-09-14 (`l` / Right is directory navigation only);
amended 2026-09-18 (synchronized terminal frames).

Amended 2026-09-19: recursive picker discovery uses one cancellable worker
per index, one replaceable pending request, and one bounded result slot.
Superseded generations cannot replace the current index. A picker retains
useful partial results and names the examined-entry or retained-path limit,
cancellation, or filesystem error that prevented complete coverage.
Directory entries are counted before filtering or retaining them, including
in a single wide directory. Directory symlinks are not recursively followed.
Workers do not hold UI locks during I/O, and dropping them requests
cancellation without joining a potentially blocked filesystem call.
The `limits` block and invocation-only CLI overrides are described in
[configuration](0008-configuration-format.md).

Recursive `Z` expansion also runs on a cancellable worker. Materialized
physical entries and virtual ancestors are bounded; useful rows survive an
incomplete walk, and the persistent `files partial` marker and `:status`
explain the coverage. A newer tree interaction cancels the old generation
rather than allowing its result to overwrite the user's selection.
Physical-listing incompleteness belongs to the retained tree, not just the
worker that read it: a successful collapse or no-op cannot clear it. A
complete reread of the affected materialized listings can clear the warning.

Amended 2026-09-15 by [0081](0081-the-menu-bar.md): one `layout` config
sets the menu bar and sidebar startup state consistently for file and
directory launches. The default shows both sidebar panes; an explicitly
named file keeps text focus.

Amended by [0091](0091-pane-focus-navigation.md): File list uses `z` to
fold/unfold its selected directory, or a selected file's immediate parent, and
`Z` to unfold all admitted directories, or fold them all when already
expanded. Recursive unfolding respects filters and does not follow directory
symlinks. A failed read is reported while retaining successful expansions and
a valid visible cursor.

Selection amended 2026-09-14 by [0079](0079-list-focus-language.md):
files and all picker results use shared active and remembered list
styles; `ui.sidebar.selected` and `ui.picker.selected` below are retired.

File shortcuts amended 2026-09-19: `Space f f` opens the ordinary picker,
`Space f i` opens the ignored-inclusive picker, and `Space f r` opens recent
files. Uppercase `Space F` now contains File-list settings only.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

Milestone 2 of the [roadmap](../roadmap.md) turns the single-file viewer
into a workspace viewer: `fathomable [DIR]` per
[0009](0009-cli-and-diagnostics.md), the tree sidebar and fuzzy picker
[0007](0007-key-grammar-and-mouse.md) names, and the session record and
socket [0003](0003-sessions-and-mcp.md) describes. The user wants the
`Space` menu from Helix, a sidebar that is trivial to toggle, and no key
that collides with zellij's default `Ctrl-g/p/t/n/h/s/o/q/b` locks. These
choices were captured in a question round on 2026-08-26.

## Decision

### Terminal presentation

- A repaint brackets the complete Ratatui draw, including its final cursor
  position, with synchronized-update commands. Supporting terminals present
  the completed frame rather than intermediate cell writes and cursor moves.
  The cursor is hidden before cell painting, including on terminals that
  ignore synchronization commands.
  The real cursor remains available at the intended File-view or input
  position; synchronization does not replace it with a painted character.
- Normal frames remain differential. Synchronization changes presentation,
  not Ratatui's cell cache, and does not require periodic full repaints.
  Resize handling and the full redraw after an external editor still
  invalidate the cache where necessary.
- Frame failures attempt to end synchronization before propagating the error.
  Terminal restoration also ends synchronization and restores a visible
  default-shape cursor before handing control back to the shell or editor.
  Panic restoration follows [0022](0022-crash-reports.md).
- Terminals or multiplexers that ignore synchronized updates do not provide
  atomic frame presentation. Synchronization is not a guarantee against
  arbitrary terminal-state loss, a broken output stream, or emulator defects.
  Differential-versus-full-render regressions check cell consistency;
  visual behavior over SSH still needs verification in the affected terminal.

### Workspace

- The workspace root is the enclosing git work tree of the argument (or of
  the current directory), else the directory itself. `fathomable FILE` also
  discovers the workspace around the file so the sidebar and picker work
  from a single-file start; the file is simply opened first.
- Paths shown anywhere in the UI are relative to the workspace root.
- Directory listings and the picker index hide entries git ignores and the
  `.git` directory itself; other dotfiles are shown. Ignore rules are
  evaluated with `gix`'s excludes stack (`gix` with
  `default-features = false, features = ["excludes", "sha1"]`), so nested
  `.gitignore`, `.git/info/exclude`, and negations behave as git does.
  Outside a repository nothing is ignored. Milestone 5 needs `gix` anyway.
  The stack reads the root's rules once when the workspace opens, so an
  event on a `.gitignore` or `.gitattributes` anywhere, or on
  `.git/info/exclude`, reloads it (`Workspace::reload_rules`,
  2026-09-06); until then an edit to the root's `.gitignore` was never
  seen while the viewer ran, though a nested one was.
- `fathomable-core` owns all of this in `workspace`, `tree`, and `picker`
  and exposes none of `gix`'s types. Repository-open options are private;
  they ignore `GIT_*` overrides so a viewer launched from a Git hook still
  reads the repository at the requested path. The never-published testing
  crate owns its own fixture options with the same policy.

### Sidebar

- Open and focused when Fathomable starts on a workspace (`fathomable`
  or `fathomable DIR`; amended 2026-08-27, it was closed by default), and
  closed when it starts on a file. `Space e` opens it and gives it focus,
  and `Ctrl-b` does the same for one-handed use. With the sidebar focused,
  `Space e` or `Esc` returns focus to the view and leaves the tree
  visible; `Space E` hides it (amended 2026-09-04: and shows it again; later that day it moved to `Space p e`, [0049](0049-inline-threads-and-the-rail.md)). Confirming a file with `Enter` returns
  focus to the view. (Amended 2026-09-04 by
  [0049](0049-inline-threads-and-the-rail.md): the sidebar is the **rail**, holding
  the **tree pane** above the **threads pane** at a fixed split;
  `rail { width split }` configures it; `Space r` is the rail submenu
  for re-read, ignored, and reveal.)
- Width is 32 columns, clamped to a third of the terminal, until the
  divider is dragged ([0007](0007-key-grammar-and-mouse.md)).
- Before a file is open the text column shows a welcome block, not a
  document: the name, the workspace root, the session id, and the keys
  that get going. It has no gutter and no cursor (2026-08-27).
- Directories are read only when expanded (lazy). `l`/`Right` expand a
  directory or descend into an expanded one; on a file they do nothing
  (amended 2026-09-14: they previously opened it and transferred focus).
  `h`/`Left` collapse or go to the parent, `j`/`k` move, and `gg`/`G`
  jump. `Enter` toggles a directory or opens a file and gives the text
  the keys. Moving the highlight onto a file previews it without taking
  focus; clicking a file also keeps the keys in the files pane
  ([0023](0023-sidebar-paging.md), amended 2026-08-27). `R` re-reads the
  expanded directories.
- The tree shows the directory name of the root at the top; entries are
  sorted directories first, then files, case-insensitively.
- A file that is not valid UTF-8 is not opened; the status line says why.
  Amended 2026-08-27: a binary file by git's rule, or a text file over the
  configured size, opens as a file-info pane instead
  ([0026](0026-binary-files-and-file-info.md)); only non-UTF-8 text is
  still refused.

### Space menu

- In normal mode `Space` opens a Helix-style menu anchored to the bottom of
  the view, above the status line: one row per entry, `key  label`, laid
  out in columns when the rows do not fit. The next key runs its entry;
  `Esc` or an unlisted key closes the menu without effect.
- Entries in this milestone: `e` toggle tree focus, `E` hide tree, `f` file
  picker, `F` file picker including ignored files, `o` recent files, `?`
  all keys. `b` (buffers) is reserved for later.
- The `g` prefix reuses the same popup so `g` shows `g goto top`,
  `s source view` while pending. The bottom-line hint from 0007 stays for
  counts and messages.

### Picker

- `Space f f` opens a centred popup: an input line on top, the ranked list
  below, the best match selected. Typing filters immediately; `Ctrl-n`/`Ctrl-p`
  (since 2026-09-03 `Ctrl-j`/`Ctrl-k`, [0045](0045-bindings-are-data.md))
  are taken by zellij, so `Up`/`Down` and `Ctrl-j`/`Ctrl-k` move, `Enter`
  opens, `Esc` closes. The terminal cursor rests immediately after the typed
  query, on the cell where the next character will be inserted.
- Matching uses `nucleo-matcher` (Helix's matcher; helix-editor org,
  MPL-2.0, already in the licence allow-list) with its `Pattern` parser, so
  Helix users get the same `^`, `$`, `!`, and `'` syntax. The 0001 table is
  amended. It runs synchronously on the main thread over the index; the
  index is a plain `Vec<String>` of root-relative paths built by walking
  the workspace when the picker first opens and reused afterwards; `Space f i`
  builds a second index that skips ignore filtering. `R` in the sidebar
  drops both indexes.
- Matched characters are highlighted with `ui.picker.match`; the selected
  row uses `ui.picker.selected`.

### Opening files and history

- Opening a file replaces the single pane. Every document opened this
  session stays loaded with its cursor, scroll, source-view flag, and
  search state, so returning to it is instant and lands where the user
  left. The live-reload watcher follows the visible document.
- A jumplist records the order files were opened. `[o` goes back and `]o`
  forward (bracket pairs never collide with a multiplexer); `Space o`
  opens the picker over the loaded documents, most recent first.
  (Amended 2026-09-04 by [0049](0049-inline-threads-and-the-rail.md): `[o`/`]o` and
  the opened-file history are gone; `Alt-Left`/`Alt-Right` walk a
  jumplist of positions that every far move records. `Space o` stays.)
- Files are read-only, so no buffer can be "modified"; `[+]` continues to
  mean changed on disk.

### Session record and socket (provisional)

- On start the TUI mints a session id `<unix-seconds>-<pid>`, writes
  `$XDG_STATE_HOME/fathomable/sessions/<id>/session.json` with the id,
  pid, workspace root, socket path, and start time, and listens on
  `$XDG_RUNTIME_DIR/fathomable/<id>.sock`. The log file uses the same id,
  closing the milestone-1 TODO. Both are removed on a clean exit; on start
  any record whose pid is dead is removed.
- Protocol v0 is line-delimited JSON over the socket, defined in
  `fathomable_core::session`: a request `{"v":0,"op":"ping"}` or
  `{"v":0,"op":"session_info"}`, a response `{"ok":true,...}` or
  `{"ok":false,"error":"..."}`. Anything else is an error response. `v`
  is mandatory so the protocol [0003](0003-sessions-and-mcp.md) finishes
  can reject v0 clients cleanly. Everything beyond these two operations
  (auth, `open`, `follow`, annotations) stays an open investigation in
  [parked ideas](../parked.md).
- `--sessions` lists records with a liveness check; `--doctor` reports the
  session directory and runtime directory.

### Theme keys

Added to the [0011](0011-theme-schema.md) table: `ui.sidebar`,
`ui.sidebar.selected`, `ui.sidebar.dir`, `ui.popup`, `ui.popup.key`,
`ui.picker.match`, `ui.picker.selected`.

## Consequences

- `gix`, `nucleo-matcher`, `serde`, and `serde_json` join `fathomable-core`;
  `tokio`'s `net` feature joins the binary for the Unix socket.
- The binary's `viewer` module becomes `app`: workspace state, sidebar,
  popups, and the socket around the existing single-document `View`.
- Recursive filesystem watching of the whole workspace is not done; the
  tree is refreshed on demand. Automatic tree refresh is parked.
- Session records make the log file name stable; `--dump-state` and
  `--replay-log` still wait on annotations and a log reader.
