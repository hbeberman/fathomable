---
type: Decision
title: Binary files and the file-info pane
description: A file is binary when git would say so — the diff attribute, else a NUL in its first 8000 bytes; opening one, or a text file over the configured size, shows a file-info pane instead of a dead-end notice, and the sidebar tags it.
resource: crates/fathomable-core/src/content.rs
related_resources:
  - crates/fathomable/src/app/draw/info.rs
tags:
  - decision
  - git
  - configuration
  - rendering
---

# 0026 Binary files and the file-info pane

Status: accepted (2026-08-27)

## Context

The viewer had one notion of a file: UTF-8 text, or an error. Opening a
`.wasm` bundle, an image, or any other binary posted the io error
*"stream did not contain valid UTF-8"* to the status line and showed
nothing ([0012](0012-workspace-mode.md) said as much: "a file that is not
valid UTF-8 is not opened"). The git status of
[0017](0017-git-status-navigation.md) was right about such files — it
hashes bytes, as git does — but the marker it drew was a dead end: the
`+n -m` counts came from a text diff that silently failed, so a rebuilt
`.wasm` showed as modified with no counts and nothing to open.

A large text file had no ceiling either: the viewer read it whole,
indexed its lines, and highlighted it, however big it was.

Settled in a question round on 2026-08-27; the choices are recorded
below.

## Decision

### What counts as binary

The rule is git's own, so the viewer and `git diff` agree on every file:

1. The `diff` attribute of `.gitattributes` decides when it is set:
   `-diff` (which the built-in `binary` macro expands to) makes the file
   binary; `diff` or `diff=<driver>` makes it text. Attributes are read
   through the same gix stack that already answers ignore queries.
2. Otherwise the file is binary when a `NUL` byte appears in its first
   8000 bytes, git's `buffer_is_binary` heuristic. Every real binary
   format trips it; text in any encoding does not.
3. A file that passes both tests but is not valid UTF-8 (Latin-1 source,
   say) is still not opened; the status line says it is not UTF-8 text.
   Rendering it lossily is parked.

Classification lives in `fathomable_core::content`, which this record
backs: the sniff, the attribute precedence, a small magic-number table
that names the common formats (WebAssembly, ELF, PNG, gzip, zip, SQLite,
…), and the size formatting the pane uses. No dependency is added; the
`attributes` feature of the existing `gix` dependency is enabled.

### The size ceiling

A text file larger than `viewer.max-file-size-mib` (default **64**) is
not read. The default is generous — a multi-megabyte log or lock file
opens without a thought — while a stray gigabyte dump can no longer
stall the viewer. The limit is a plain integer of MiB, in the style of
the other config nodes, and `--config-show` prints it:

```kdl
viewer {
    max-file-size-mib 64
}
```

Binary files are never read whole: their size comes from the metadata,
and only the sniff prefix is read.

### The file-info pane

Opening a binary or over-limit file shows a **file-info pane** in the
text column, where the document would be. It is a document in every
other respect — it joins the history and the recent list, the tree
highlights it, `[o` returns to it — so the reader lands on it the same
way and leaves it the same way. The pane lists:

- the path, and the format the magic table recognises ("WebAssembly
  module", "PNG image", … or "binary data");
- the size, the mode (`executable`, `symlink → target`), and the
  modification time;
- the git state: `untracked`, `modified`, `staged`, `unchanged`, or
  `not in git`; and for a tracked file the size in `HEAD` beside the
  size on disk with the delta, which is the binary equivalent of the
  `+n -m` counts;
- for an over-limit text file, the notice in place of the format line:

  ```
  Too large to view: 312.7 MiB, limit is 64 MiB.
  Raise it with `viewer { max-file-size-mib 512 }` in <config path>
  ```

  The suggested value is the file's size rounded up to the next power of
  two, so it can be pasted as is; the path is the resolved config file
  (`--config` when given, else `$XDG_CONFIG_HOME/fathomable/config.kdl`).

The pane is read-only text with no cursor, gutter, or line numbers. `c`
declines with *"cannot annotate a binary file"* (or *"… a file this
large"*): threads are anchored to lines, and a file has none here.
File-wide threads, with a way to show them, are a noted follow-up.

### The sidebar and the status line

- A dirty binary file's sidebar row shows a `bin` tag where the counts
  would be, so the marker is no longer blank.
- The status line's counts stay empty for such a file; the pane carries
  the size delta instead.
- Follow-mode toasts for a binary file show the path alone, as they do
  for any file without counts.

## Consequences

- `Document` holds a `Content` — `Text`, `Binary`, or `TooLarge` — and
  `text()` returns `Option<&str>`. Loading takes a `content::Policy`
  (the file's diff attribute and the size ceiling) that the reload
  reuses. Every reader of the text (deltas, seen snapshots, the diff
  bases, re-anchoring) skips a document without it.
- `Workspace` gains `diff_attr`, answered by the attribute-and-ignore
  stack that replaces the ignore-only one, and `head_size`, read from
  the object header without loading the blob. `status::Entry` gains
  `is_binary`, set from the same classification the counts use.
- `Config` gains the `viewer` block and `ViewerConfig`; `Options` gains
  `viewer` and the resolved `config_path` the notice names.
- The pane's rows are assembled in `app/draw/info.rs`; `ui` draws them.
- 0012's "a file that is not valid UTF-8 is not opened" narrows to the
  non-UTF-8 text case; 0015's snapshot rules are unchanged (a binary or
  over-limit file has no text to snapshot).
- `docs/guide.md` gains the `viewer` block, the pane, and the `bin`
  tag in the same change.
