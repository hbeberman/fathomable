---
type: Decision
title: Workspace mode
description: The tree sidebar, the space menu, the fuzzy file picker, open-file history, and the first session record and socket.
resource: crates/fathomable/src/app/mod.rs
related_resources:
  - crates/fathomable/src/app/ui.rs
  - crates/fathomable-core/src/workspace.rs
  - crates/fathomable-core/src/tree.rs
  - crates/fathomable-core/src/picker.rs
tags:
  - decision
  - input
  - sessions
---

# 0012 Workspace mode

Status: accepted (2026-08-26)

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
- `fathomable-core` owns all of this in `workspace`, `tree`, and `picker`
  and exposes none of `gix`'s types.

### Sidebar

- Open and focused when Fathomable starts on a workspace (`fathomable`
  or `fathomable DIR`; amended 2026-08-27, it was closed by default), and
  closed when it starts on a file. `Space e` opens it and gives it focus,
  and `Ctrl-b` does the same for one-handed use. With the sidebar focused,
  `Space e` or `Esc` returns focus to the view and leaves the tree
  visible; `Space E` hides it. Opening a file from the tree also returns
  focus to the view.
- Width is 32 columns, clamped to a third of the terminal, until the
  divider is dragged ([0007](0007-key-grammar-and-mouse.md)).
- Before a file is open the text column shows a welcome block, not a
  document: the name, the workspace root, the session id, and the keys
  that get going. It has no gutter and no cursor (2026-08-27).
- Directories are read only when expanded (lazy). `l`/`Enter`/`Right`
  expand or open, `h`/`Left` collapse or go to the parent, `j`/`k` move,
  `gg`/`G` jump, a click on an entry does what `Enter` does. `R` re-reads
  the expanded directories. Amended 2026-08-27: moving the highlight
  onto a file also shows it in the main pane without taking focus, so the
  tree pages the viewer; `Enter` and a click still move focus to the view
  ([0023](0023-sidebar-paging.md)).
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

- `Space f` opens a centred popup: an input line on top, the ranked list
  below, the best match selected. Typing filters immediately; `Ctrl-n`/`Ctrl-p`
  (since 2026-09-03 `Ctrl-j`/`Ctrl-k`, [0045](0045-bindings-are-data.md))
  are taken by zellij, so `Up`/`Down` and `Ctrl-j`/`Ctrl-k` move, `Enter`
  opens, `Esc` closes.
- Matching uses `nucleo-matcher` (Helix's matcher; helix-editor org,
  MPL-2.0, already in the licence allow-list) with its `Pattern` parser, so
  Helix users get the same `^`, `$`, `!`, and `'` syntax. The 0001 table is
  amended. It runs synchronously on the main thread over the index; the
  index is a plain `Vec<String>` of root-relative paths built by walking
  the workspace when the picker first opens and reused afterwards; `Space F`
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
