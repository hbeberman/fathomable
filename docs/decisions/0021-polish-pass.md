---
type: Decision
title: "Polish pass: one change source, diff toggles, status overlay"
description: Removals and merges from the first single-user polish pass, so the keymap, command line, and status line each do one thing.
resource: crates/fathomable/src/app/commands.rs
tags:
  - decision
  - input
---

# 0021 Polish pass: one change source, diff toggles, status overlay

Status: accepted (2026-08-27)

Amended 2026-09-19 by [0081](0081-the-menu-bar.md): `:mcp` opens the same
read-only MCP setup steps as **Help > MCP Setup**.

Amended 2026-09-18: the command surface drops `:nohlsearch`/`:noh`,
`:source`, and `:licenses` without aliases. `Esc` already clears search
highlights, source/rendered remains a file-view action through `Space v s`
and its menu row, and bundled notices remain under **Help > Licenses**.

Amended 2026-09-15 by [0081](0081-the-menu-bar.md): the structured
`--doctor` report is also available as a scrollable in-app Doctor view
through Help or `:doctor`; `:status` keeps its narrower live-viewer role.

Amended 2026-09-18 by
[0087](0087-global-comparisons-and-board-history.md): both historical diff
toggles and their `:diff` commands are removed without aliases. Explicit
Standard, Unified, and Off modes replace them; the diff-toggle and command
routing sections below remain historical rationale.

Amended 2026-09-18: the command line now lists every canonical command while
open and narrows the list with case-insensitive fuzzy subsequence matching.
Exact and prefix matches rank first. The first `Tab` selects a result and shows
its description and argument contract; later `Tab` or `Shift-Tab` presses cycle
the match set frozen from the original query. Typing or erasing starts a new
query. Aliases participate in matching but do not duplicate canonical list
entries. Numeric `:N` jumps remain valid but are not finite completion
candidates.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

Eleven milestones landed in quick succession, each adding a key, a
command, a config node, or a status segment for the idea it carried. With
one user and no compatibility promise, a pass on 2026-08-27 looked for
places where two mechanisms did one job or a mechanism did a job nobody
had asked for. Each cut below was put as a question and answered:

- *Follow sources.* [0015](0015-follow-mode.md) let `follow.source` pick
  between the whole workspace, only the agent's `follow` list, or only
  agent `open` calls, cycled with `Space j s` and `:follow source`, and
  shown as `src:` in the status line. Only `workspace` was ever used.
- *Diff bases.* `gd` cycled `HEAD` diff, last-seen diff, off
  ([0017](0017-git-status-navigation.md)). A three-state key is hard to
  read from the pill and hard to leave.
- *Status line.* The right-hand block carried the session id and the
  follow source next to position, counts, and threads. Neither changes
  while reading, and the session id is only needed to match a log file or
  an MCP session.
- *Duplicate keys.* Helix's `x` (select line, repeat to extend) sat beside
  Vim's `V`; `Ctrl-b` duplicated `Space e`; the picker moved on both
  `Ctrl-j`/`Ctrl-k` and `Ctrl-n`/`Ctrl-p`.
- *Dead flags.* `--dump-state` and `--replay-log` parsed and then printed
  "not implemented" ([0009](0009-cli-and-diagnostics.md)).
- *Construction.* `App::new` took six arguments and was followed by four
  setters before the first draw; `:follow` was the only command that
  crossed from the view to the app, through a string prefix match.

## Decision

### One change source

- Every non-ignored write under the workspace is a change. The `Source`
  enum, the `follow.source` config node, `Space j s`, `:follow source`,
  the `src:` status segment, and the `follow_source` field of the
  `session_info` response are gone. The agent's `follow` list still shows
  as `follow N` in the status line and in `:status`.

### Two diff toggles

- `gd` and `:diff` toggle the unified diff against `HEAD`; `gD` and
  `:diff seen` toggle the diff against the last-seen snapshot. Either
  key from the other diff switches bases. Outside git `gd` reports "no
  diff base"; a file never seen reports "no last-seen snapshot".

### Status overlay

- `:status` opens a popup listing the session id, workspace root, socket
  path, threads file, snapshots directory, what is being watched,
  auto-jump, pending changes, and the agent's follow list. Any key
  closes it. The status line keeps the mode pill, path, `[+]`, position,
  percentage, `+N -M`, thread counts, `follow N`, and the change hint.

### One key per job

- `x` in the view, `Ctrl-b`, and the picker's `Ctrl-j`/`Ctrl-k` are
  removed. Selection is `v`, `V`, and the mouse; the tree is `Space e`
  and `h` at column 0; the picker moves on arrows and `Ctrl-n`/`Ctrl-p`.
  (Amended 2026-09-03 by [0045](0045-bindings-are-data.md): the picker
  moves on `Ctrl-j`/`Ctrl-k`; `Ctrl-n`/`Ctrl-p` are zellij lock chords.)

### Command routing

- `View::execute` handles immediate quit and numeric line jumps and returns
  registered app commands as `Effect::Command`; `App::command` in
  `app/commands.rs` runs those app commands and refuses the rest with one
  message. Removed commands have no compatibility aliases.

### Construction

- `app::Options` carries the record, stores, follow config, highlighter,
  and Markdown list; `App::new` consumes it and runs the ADR 0020 startup
  re-anchoring itself. The only setter left is `set_watching_root`,
  which reports the watcher's outcome. `--dump-state` and `--replay-log`
  are removed along with their parked entry.

## Consequences

- The `follow` block of `config.kdl` no longer accepts `source`; a
  config that sets it fails to load with the usual "unknown follow
  setting" line. Agents reading `session_info` see only `auto_jump`.
- The `Space j` menu has three entries; `Space ?` lists one binding per
  action.
- Tests construct an app with `Options::for_test(root)` and struct
  update syntax instead of a chain of setters.
- Parts of [0007](0007-key-grammar-and-mouse.md), 0009, 0012, 0015, and
  0017 that describe the removed keys, flags, and cycle are superseded by
  this record.
