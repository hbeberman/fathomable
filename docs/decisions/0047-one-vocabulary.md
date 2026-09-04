---
type: Decision
title: One vocabulary for the viewer
description: One word per idea across code, screen, config, CLI, wire, theme, and docs — workspace, viewer, agent session, thread and comment, ThreadState, placement, waiting and pending, reach, coverage, follow versus auto-jump, subscribe — with the renames that carry it, the compatibility each keeps for one release, and the rule that retired words are noted, not rewritten, in older records.
resource: crates/fathomable-core/src/vocabulary.rs
tags:
  - decision
  - documentation
  - configuration
  - sessions
---

# 0047 One vocabulary for the viewer

Status: accepted (2026-09-03)

## Context

[0043](0043-agent-vocabulary.md) put the agent-facing names in one table
and checked every hook string against it. The human-facing words had no
such table and had drifted with the milestones: *session* meant a
running TUI ([0003](0003-sessions-and-mcp.md)), then a workspace's
annotation state ([0024](0024-workspace-sessions.md)), and all along the
harness session an agent runs in; *follow* was the agent's tool, the
viewer's auto-jump mode, its `:follow` command, and its config block;
*scope* was both the git-reachability filter and a subscriber's paths;
*annotation*, *note*, and *comment* named the same thing; the pane was a
*panel*; the thread list's motions were *local* and *global*. The review
of 2026-09-03 listed the collisions and Henry accepted one word per idea.

## Decision

The vocabulary below is final; code, screen text, config, CLI, wire,
theme, and docs use these words and no other for these ideas.

| Idea | Word | Retired |
| --- | --- | --- |
| directory tree being viewed | **workspace** | root (as a noun), "the work" |
| running TUI | **viewer** | session, window |
| workspace's annotation state | **workspace** | session |
| harness session | **agent session**, `id` | session (unqualified) |
| comment plus replies | **thread** | annotation (user-visible), note |
| the opening message | **comment** | — |
| a thread placed in text | **mark** (code only) | — |
| four-way state | **`ThreadState`** (`waiting`, `open`, `resolved`, `auto-resolved`) | `MarkKind`, kind |
| where a thread's lines are | **placement** (`anchored`, `edited`, `detached`) | — |
| newest message is someone else's | **waiting** (user's chair), **pending** (agent's chair); `Thread::awaits(Party)` | `awaits_user`, `pending_for` |
| git reachability filter | **reach** (`Reach`, `reachable`) | `Scope`, "in scope" |
| subscriber's paths | **coverage** (`covers`) | scope, follow list |
| thread motions | **in file** / **across the workspace** | local/global |
| agent's edited-files list | **follow** (`followed`, `N followed`) | markers |
| viewer jumps on its own | **auto-jump** (`:auto`, `jump.auto`, `AUTO`) | follow mode, `:follow`, `follow.auto` |
| agent registers | **subscribe**, **subscriber**, **subscription** | — |
| agent category | `type` outward, `kind` in code | — |
| left column | **sidebar**; the widget in it is the **tree** | — |
| bottom-of-text thread view | **thread pane** (`ThreadPane`) | panel, `ThreadPanel` |
| `Space A` view | **thread list** | — |
| threads-of-this-file view | **file-threads pane** | — |
| what receives keys | **focus** | — |
| anything layered | **popup** | overlay |
| a queued write | **change**; its status segment the **change hint** | pending change |
| prefix key being held | **prefix** | pending |
| position in a list | **cursor**; the row drawn for it the **highlight**; **selection** only for `v`/`V` | selected, current |
| "what changed since I looked" base | **last seen**, **snapshot** | seen (alone) |

The renames that carry it, each landing as one commit:

1. **Code.** `ThreadPanel` → `ThreadPane`, `MarkKind` → `ThreadState`,
   `annotations::Scope` → `Reach`, `mcp::Session` → `Target`,
   `App.session` → `viewer_id`, `Thread::awaits_user` and `pending_for`
   → `awaits(Party::User | Party::Subscriber(id))`, `app/rescope.rs` →
   `app/threads/reach.rs`.
2. **Screen.** Notices and `:status` rows say *workspace*, *changes*,
   *followed*, *subscribers*; nothing says *the work*, *local*, or
   *global*.
3. **Config.** `follow { auto jump-debounce toast }` → `jump { auto
   debounce toast }`; `follow { ignore hint-debounce }` → `watch { ignore
   debounce }`; `follow { seen-idle }` → `viewer { seen-idle }`. A
   `follow` block is an error whose message names the new node for each
   old setting.
4. **CLI.** `--sessions` → `--viewers` (the old flag a hidden alias for
   one release); `:follow [on|off]` → `:auto [on|off]`; viewer records
   move from `sessions/` to `viewers/` under the state directory, and
   dead records are swept from both for one release.
5. **Wire.** Tool parameter `session` → `workspace`; `session_list` →
   `workspace_list`; `session_switch` → `workspace_switch`;
   `annotations_list` → `threads_list`. `threads_pending` and the
   `pending` subcommand keep their name: *pending* is the agent's word.
   Hosts re-list tools on connect, so nothing is cached.
6. **Theme.** `annotation.*` keys → `thread.*`; the old names still load
   and `--doctor` names each one used ([0011](0011-theme-schema.md)
   amended).
7. **Docs.** The charter's vocabulary section and its "agent endpoint"
   and "follower" bullets are rewritten. Every older record that used a
   retired word gets one dated line under its status naming the renames,
   rather than an edit of its history.

## Consequences

- A reader meets each idea under one word in the guide, the screen, the
  config, and the code; a contributor grepping for `session` finds only
  the harness session.
- Existing `config.kdl` files with a `follow` block stop loading until
  edited; the error says what to write. Existing themes keep working.
- Agent hosts pick up the new tool names on their next connection; a
  prompt or skill that hard-codes `annotations_list` or `session` must
  be updated, which the [0043](0043-agent-vocabulary.md) table and its
  tests make one sweep.
- The one-release compatibility (`--sessions`, the `sessions/` sweep,
  `annotation.*` theme keys) is removed by a later record.
