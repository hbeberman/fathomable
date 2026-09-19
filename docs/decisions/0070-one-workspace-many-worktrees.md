---
type: Decision
title: One workspace, many worktrees
description: A git workspace is the repository, keyed by its common dir, and every worktree of it is one checkout of the same threads; the viewer finds the worktrees itself, pages through them with `]w` / `[w`, names the active one in the global menu bar, and shows a thread from any worktree's branch with that branch on the entry; the tools and hooks resolve a worktree the way they resolve a root, and no tool is added.
resource: crates/fathomable-core/src/worktrees.rs
related_resources:
  - crates/fathomable-core/src/workspace.rs
  - crates/fathomable-core/src/xdg.rs
  - crates/fathomable-core/src/session.rs
  - crates/fathomable/src/app/worktrees.rs
  - crates/fathomable/src/app/watch.rs
  - crates/fathomable/src/mcp/mod.rs
tags:
  - decision
  - git
  - sessions
  - annotations
  - architecture
---

# 0070 One workspace, many worktrees

Status: accepted (2026-09-07); amended 2026-09-15 (the optional
`worktrees/` registry is watched only while it exists)

Watch discovery amended 2026-09-19: the core supplies only the active Git
directory, common directory, refs, and existing registry as deduplicated
anchors. Registry children and nested refs are discovered and watched by
the finite background worker described in [0028](0028-live-workspace.md).
This includes newly created namespaces outside the active linked checkout;
partial coverage is explicit rather than an unlimited UI-thread walk.

Thread landing amended 2026-09-18 by
[0090](0090-direct-workspace-navigation.md): direct thread traversal never
activates another worktree. It shows projected source in the current
worktree when available and otherwise shows the exact Reviews evidence.
Only the explicit `]w`/`[w` cycle and worktree picker switch worktrees.

Amended 2026-09-17: repository and active-worktree identity move from the
Files header to the global menu bar beside the current filename. Clicking
that identity opens the worktree picker when the repository has several.

Amended 2026-09-16 by
[0087](0087-global-comparisons-and-board-history.md): the repository still
shares threads and review points across linked worktrees, while comparison
preferences are checkout-local. Every unarchived thread remains on the board;
worktree ancestry qualifies projection rather than membership.

MCP routing amended again 2026-09-15 by
[0082](0082-three-tool-review-core.md): a server binds one repository
checkout at startup and tools have no per-call workspace or viewer selector.
The common-dir thread store and manual viewer worktree navigation remain.
Cross-workspace MCP routing is parked rather than retained as dead code.

MCP routing amended 2026-09-15 by
[0080](0080-automatic-chat-identity.md): `workspaces` only lists and
has no `switch`. `fathomable --mcp [DIR]` anchors the default at startup;
per-call workspace roots select other worktrees. Shared-server callers
must name that override rather than rely on a subagent's cwd or chat
metadata to change the default. Process bonds and `hello` are removed.

State-key adoption amended 2026-09-15 by
[0083](0083-single-user-alpha-clean-slate.md): common-dir identity and
shared worktree state remain, but the one-time root-keyed directory move is
retired. Fresh viewer, register, and MCP startup use the current key
directly; they do not probe an old root key or rename a directory. A plain
directory later initialized as Git does not transfer its prior state
automatically. The old move is historical context, not a compatibility
exception.

## Context

An agent that is asked for a change increasingly does not make it where
it was asked. It adds a git worktree, hands the work to a subagent whose
cwd is that worktree, and merges when the subagent is done. The user is
looking at the host checkout. The lines the subagent writes, and the
threads it starts on them ([0061](0061-agents-start-threads.md)), are
somewhere else.

Today a workspace is one root path ([0024](0024-workspace-sessions.md)):
its threads, seen marks, checkpoints, register, and sockets live under
the short hash of that path, and `fathomable --mcp` and the hooks find
the workspace by the longest root that prefixes the caller's cwd. A
worktree is a different path, so it is a different workspace with an
empty store, and it is not even known until something runs
`fathomable --register` there. The subagent's `follow` binds to
nothing, its `thread_start` lands in a store no viewer shows, and the
user's viewer, rooted at the host, never learns any of it happened.

0024 parked keying the store by the git common dir, "to survive renames
and share across worktrees". This is the reason to un-park it. A thread
already belongs to a commit and shows only where that commit is `HEAD`
or an ancestor ([0024](0024-workspace-sessions.md),
[0035](0035-threads-follow-head.md)); a store shared by every checkout
of one repository needs no new rule to keep the branches apart.

The user raised the idea on 2026-09-06 and a question round settled
four things: the viewer finds worktrees itself rather than being told;
a worktree is the same workspace as its host, not a linked one; the
viewer pages through worktrees, one active at a time, rather than
drawing a merge of them; and a thread the host has not merged yet shows
in the host, labelled with its branch.

## Decision

### Vocabulary

- A **workspace** is a repository: every checkout that shares one git
  common dir. A directory outside git is a workspace of one.
- A **worktree** is one checkout of a workspace: the *main* worktree,
  whose `.git` is the common dir, or a *linked* one made by
  `git worktree add`. A bare repository has no main worktree. The word
  is git's; the viewer does not coin another.
- A **root** is a worktree's path. The `workspace` parameter of every
  tool, `workspaces` with `switch`, `--register`, and `fathomable DIR`
  keep taking a root; a root now names its workspace and, within it,
  a worktree.

### The key is the common dir

- A git workspace's state directory is
  `$XDG_STATE_HOME/fathomable/workspaces/<hash>`, `hash` the short
  SHA-256 of the canonical common dir. A plain directory keeps the
  hash of its root. Threads, the agent register, seen snapshots,
  checkpoints, the marker, and the viewer sockets all follow the key:
  every worktree of a repository reads and writes one store. Thread,
  seen, and checkpoint paths are already root-relative, so a record
  written in one worktree names the same file in every other.
- Seen snapshots and checkpoints are shared as the threads are. What
  the reader last saw of `src/a.rs` is one thing whether they saw it
  on `main` or on a branch; a per-worktree mark would make `gD` in a
  freshly added worktree show the whole file as unseen.
- The marker `workspace.json` names the common dir and every worktree
  root the workspace had when it was written. A viewer rewrites it at
  start and whenever the worktree set changes.
- A state directory keyed by a main root under the old rule is moved
  to its common-dir key, once, by the first viewer, `--register`, or
  `--mcp` that finds the new key absent and the old present. It is a
  rename of a directory, not a translation of a format, and the
  alternative is a repository whose threads vanish on upgrade;
  [0062](0062-one-version-no-compatibility.md)'s rule against
  compatibility code is about formats and stands.

### The viewer finds the worktrees

- A viewer opened anywhere in a repository lists its worktrees through
  the existing `gix` access ([0006](0006-git-access.md)): the main
  worktree, then the linked ones in the order git keeps them, each
  with its root, its branch or short commit when detached, and its
  `HEAD`. A linked worktree whose directory is gone is not listed; a
  locked one is. The worktree the viewer was opened in is the
  **active** worktree.
- The viewer watches the common dir for its `HEAD` and index, its
  `refs` for a branch moving, its `worktrees/` registry while that
  optional directory exists, and each linked worktree's git dir for
  its `HEAD`, one watch each and the refs recursively. The common-dir
  watch observes Git creating the registry for the first linked
  worktree. A commit in a worktree moves the branch under the common
  dir, not the worktree's `HEAD`.
  Only the active worktree's visible directories are walked and watched;
  ignored trees spend no inotify watches ([0015](0015-follow-mode.md)).
- No tool adds a worktree. The set is git's; an agent that ran
  `git worktree add` has already told the viewer everything. A `link`
  flag on `workspaces` was considered for a clone elsewhere and
  rejected: a clone is another repository, and a thread cannot follow
  a commit across one.

### Paging

- `]w` / `[w`, from any pane, make the next or previous worktree
  active, wrapping; the order is the listing's. With one worktree the
  key says so. The keys sit in the bracket family with `]g` and `]c`;
  there is no leader entry, by [0056](0056-the-leader-trimmed.md)'s
  rule against entries that duplicate a bare key, and `Space ?` names
  them.
- Making a worktree active re-roots the viewer: the files pane walks
  that tree, the watcher and the status walk move to it, and the
  gutter, the diffs, and the placement of threads read its files. The
  current file stays open by its relative path when the worktree has
  it, at the same line, else the welcome shows; other open documents
  close, and the jumplist and the recent list are cleared. A viewer is
  one window onto one checkout; two checkouts at once are two viewers.
- The global menu bar ([0081](0081-the-menu-bar.md)) centers the repository
  name, then the active worktree's branch or short detached commit whenever
  there is more than one, then the current filename when one is open. The
  repository/worktree segment uses the menu accent and opens a picker of
  worktrees on click, with the active one marked. With one worktree the
  repository name is passive and subdued.
- `:status` lists the worktrees with the active one marked; the viewer
  record names the worktree the viewer is on, `--viewers` groups by
  workspace, then worktree, and `--doctor` counts the worktrees that
  share the state.

### A thread from any worktree shows everywhere

- The **reach** of a workspace is the union over its worktrees: a
  thread shows when its commit is `HEAD` or an ancestor of `HEAD` in
  any worktree. The walk of 0024 runs once per worktree and is cached
  per worktree `HEAD`; a thread carries the set of worktrees that
  reach it.
- A thread the active worktree reaches is placed against the active
  worktree's files and drawn as today. A thread only another worktree
  reaches is placed against the first worktree that reaches it, and
  its entry in the threads pane ([0066](0066-one-circle-language.md))
  carries that worktree's branch, dim, after the author's words; the
  file rows group it under its path in the pane's order, at the end
  when the active worktree has no such file. Its stub is not drawn in
  the active worktree's text, since the lines it is on are not there.
- Opening such a thread, from the threads pane or the review list,
  makes its worktree active first. A thread from a worktree is the way
  into it.
- The files pane's circles count the threads the active worktree
  reaches; the threads pane's header counts every thread shown.
- A branch merged into the active worktree stops needing a label: its
  commits are then ancestors here, and the entries read as any other.
  A worktree removed takes its unmerged threads out of reach, as a
  branch switch does under 0024.

### The tools and the hooks

- `fathomable --mcp` and the hooks resolve the caller's cwd against
  every worktree root of every known workspace, longest first, as
  they resolve roots today. When none prefixes the cwd, they discover
  the cwd's common dir through `gix` and look its key up, so a
  worktree added after the marker was last written is found without
  a `--register`. The matched worktree is the **caller's worktree**. A
  tool goes through a viewer only when that viewer shows the caller's
  worktree; otherwise it reads and writes the store itself, as it does
  with no viewer, and the viewers pick the write up through the store
  watch. A subagent's `thread_start` in its worktree therefore never
  flips the user's viewer; only `open` pages it.
- The six tools of [0055](0055-six-tools.md) stay six. `workspaces`
  lists each workspace with its worktrees (root, branch, `main`), the
  caller's marked, and each viewer with the worktree it shows;
  `switch` takes a worktree root and pins the workspace and, within
  it, the caller's worktree. Every other tool's `workspace` parameter
  accepts a worktree root the same way.
- `open` takes a path relative to the caller's worktree; a viewer that
  is on another worktree pages to the caller's first, as it would jump
  to the file. `thread_start` and `thread_reply` record the caller's
  worktree `HEAD` ([0024](0024-workspace-sessions.md)); nothing in the
  record says which worktree, since the commit does. `threads` places
  each thread against the caller's worktree when it reaches it, else
  against the first that does, and then names that worktree in a
  `worktree` field; the hooks' deliveries carry the same field. A
  subscription covers the workspace, every worktree of it, as 0055
  already says.
- The `pending` hook, run from a subagent whose cwd is a worktree,
  binds to the workspace and delivers what waits there. A session
  whose hello came from the host and whose subagent works in a
  worktree is one session in one workspace, and the bond of
  [0041](0041-session-bonds.md) is unchanged.

### Amendments

- 0024's *A session is a workspace* bullet carries a dated note: the
  common-dir key is taken here; the store is the repository's.
- 0055's `workspaces` row: worktrees in the listing, `switch` and
  `workspace` take a worktree root; the `threads` row gains
  `worktree`.
- 0068's header bullet: repository and branch identity leave the Files
  header for the global menu bar.
- 0066's entry bullet: the branch after the author's words on an
  entry the active worktree does not reach.
- 0062: the one-time move of a state directory is noted as a rename,
  outside the rule.

## Consequences

- `fathomable-core/src/worktrees.rs`, which this record backs, lists a
  workspace's worktrees (`Worktree { root, branch, head, main }`),
  computes the common-dir key, and answers the union reach.
- `Workspace` gains `key()`, `common_dir()`, and `worktrees()`;
  `XdgDirs` takes the key where it took the root, and the marker
  carries `common_dir` and `roots`.
- `app/worktrees.rs` holds the active worktree, `]w` / `[w`, the
  re-root, and the picker; `app/watch.rs` watches the common dir, its
  refs, its existing `worktrees/` registry, and each linked worktree's
  git dir; `Reach` learns which worktrees reach a thread (`here`,
  `elsewhere`).
- `mcp/mod.rs` resolves a worktree; `mcp/tools.rs` lists them and
  names the caller's; `hooks.rs` resolves the cwd the same way and
  carries `worktree` in a delivery.
- The binding table gains `WorktreeNext` and `WorktreePrev`; the
  viewer record and `--viewers` name the worktree; `--doctor` counts
  worktrees; the socket layout keys by the common dir.
- The guide's §2 says a worktree is the workspace, §3's key tables
  gain `]w` / `[w`, §4 the branch on an entry, §8 the key, the marker,
  the `workspaces` listing, and the `worktree` field, and §9
  `--viewers` and the one-time move.
