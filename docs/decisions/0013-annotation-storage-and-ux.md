---
type: Decision
title: Annotation storage and UX
description: The JSONL event log, the line-hash anchor, the comment box, the gutter mark, the thread panel, and the keys that drive them.
resource: crates/fathomable-core/src/annotations.rs
related_resources:
  - crates/fathomable/src/app/threads.rs
tags:
  - decision
  - annotations
  - input
  - rendering
---

# 0013 Annotation storage and UX

Status: accepted (2026-08-26)

## Context

[0005](0005-annotations.md) fixes what an annotation is and where threads
live, but leaves the file format, the anchor algorithm, and the whole
viewer side open. Milestone 3 of the [roadmap](../roadmap.md) needs all of
them. Two things in earlier records also had to give: 0010's
copy-on-release meant every mouse selection wrote to the clipboard, which
is wrong once a selection can also start a comment, and the gutter had one
cell for the git diff bar and nowhere for an annotation. The choices below
were captured in a question round on 2026-08-26.

## Decision

### Store

- One file per workspace:
  `$XDG_STATE_HOME/fathomable/workspaces/<hash>/threads.jsonl`, where
  `hash` is the first 16 hex characters of the SHA-256 of the workspace
  root path (`XdgDirs::threads_file`). The workspace tree is never written
  to, as 0005 requires; an in-tree or committed file was considered and
  rejected for now because agents will read threads through MCP
  (milestone 4), and a config override can be added later without
  changing the format.
- The file is an append-only event log. Every line is one JSON object with
  `"v": 1` and an `event` of `annotate`, `reply`, `resolve`, or `reopen`;
  `fathomable_core::annotations::Store` folds it into threads on open and
  appends after every change. A line that is not a known event, or that
  refers to a thread the file never created, is an error naming the line
  number; the TUI then runs with annotations disabled and says so. An
  `annotate` line carries id, path, range, snippet, anchor, created, and
  comment; a `reply` carries thread, author, created, body, and an optional
  `proposed_resolved`; a `resolve` carries thread, `by` (author), and
  created; a `reopen` carries thread and created. `by` other than `user`
  is the 0005 `auto_resolved` case.
- Thread ids are `<unix-seconds>-<pid>-<n>`; authors are the string `user`
  or an agent-supplied name; timestamps are Unix seconds supplied by the
  caller so the store stays pure.

### Anchor

- `Anchor::capture` hashes each annotated line, plus the line above and the
  line below when they exist. The hash is the first 16 hex characters of
  SHA-256 over the line with trailing whitespace removed; `sha2`
  (approved in [0001](0001-dependency-policy.md)) joins `fathomable-core`
  with this consumer.
- `Anchor::locate` searches the current text for a window whose line
  hashes equal the annotated ones. Among matches, the one whose neighbours
  also match wins; ties go to the window nearest the last known start. No
  match means the exact lines are gone and the thread is `Detached` at its
  last known range. Nothing smarter is attempted for edited lines; that
  stays an open investigation in [parked ideas](../parked.md).

### Selection and the clipboard (amends 0010)

- Releasing a mouse drag no longer copies. The selection stays highlighted
  in `SEL` mode, exactly as after `V`; `y` copies the source Markdown via
  OSC 52, `c` opens the comment box, and Esc clears. Mouse and keyboard
  selections are therefore indistinguishable once made.
- `c` on a selection annotates the source lines it touches; a wrapped
  paragraph that renders as one row annotates all of its source lines.

### Comment box

- `c` opens a box anchored to the bottom of the text pane, above the status
  line, titled with the range (`comment on L3-5`) or `reply`. Enter adds a
  line; Ctrl-Enter or Alt-Enter submits; Esc cancels. The box grows to
  eight rows, then scrolls. An empty comment is discarded with a notice.
- Ctrl-Enter is only distinguishable from Enter when the terminal supports
  the kitty keyboard protocol, so the TUI pushes
  `DISAMBIGUATE_ESCAPE_CODES` when `supports_keyboard_enhancement` says it
  can, and Alt-Enter is always accepted as the fallback.
- The same box is used for replies, so there is still exactly one text
  entry in Fathomable.

### Rendering

- The gutter gains a fourth cell: `[line number][space][diff bar][note]`
  (moved to the far left by [0006](0006-git-access.md), 2026-08-26).
  The note cell shows `▎` coloured by thread state; the diff bar keeps its
  own cell so git and annotation information never hide each other. A
  thread's rows also get the `annotation.line` background.
- Theme keys (added to the [0011](0011-theme-schema.md) table):
  `annotation.open`, `annotation.resolved`, `annotation.resolved.auto`,
  `annotation.detached`, and `annotation.line`. When several threads
  overlap a row the most urgent colour wins: detached, then open, then
  auto-resolved, then resolved.
- The status line's right block shows `open/total threads` for the current
  file when there are any.
- Marks are re-located on every reload, so an agent's edit moves or
  detaches them immediately.

### Reading and replying

- `Space a` opens the thread panel over the bottom third of the text pane
  for the thread(s) touching the cursor row: a rule, a header with the
  position (`thread 1/2`), range, status in its gutter colour, and the
  keys right-aligned; the quoted snippet with line numbers (three lines,
  then `…`); the comment; and each reply with the author's short name,
  its age (`5m ago`, `yesterday 06:38`, then the UTC date), and a
  `[proposes resolving]` badge when set. Bodies are indented under their
  author. When the text overflows the last row reads `▼ N more`, and
  scrolling stops at the end. `j`/`k` scroll, `n`/`p` switch
  between threads on the row, `r` replies through the comment box, `x`
  resolves an open thread or reopens a resolved one, Esc closes.
- While replying the panel stays on screen above the box (taking up to
  half the pane) so the thread can be read; Up/Down scroll it, and Esc
  returns to the panel rather than closing everything.
- `]a` and `[a` jump to the next and previous thread in the file, wrapping
  with a notice. `Space A` opens the picker over the file's threads
  (`L3-5  open  first line of the comment`); choosing one jumps there and
  opens the panel. Both space entries follow 0012's rule that new
  commands live in the Space menu; none of the keys are zellij locks.

## Consequences

- 0010's "releasing the button copies immediately" and its two-cell gutter
  are superseded by this record; 0011's key table grows by five keys; the
  0005 storage path is now concrete.
- `check-public-api.sh`'s no-bool-parameter rule shaped the core API:
  `Reply::proposing_resolution()` instead of a flag, `Placement` instead of
  a detached bool.
- Agents get their MCP entry points (`annotations_list`, `thread_reply`) in
  milestone 4; until then a thread file is plain JSONL an agent can read.
- Editing a thread in `$EDITOR` and discouraging agent force-resolve stay
  parked.
