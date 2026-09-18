---
type: Concept
title: Fathomable charter
description: What Fathomable is, what it is not, and the principles the project holds to.
tags:
  - charter
---

# Fathomable charter

Fathomable is an agent-agnostic, read-only terminal viewer that lives in a pane
beside an agent chat TUI. It renders the documents and code an agent is working
on, follows live changes, and lets a human leave review-style annotations that
an agent can read and reply to. It is the place you *read* and *respond* while
the agent does the writing.

## What Fathomable is

- A **workspace viewer**: open a directory to get a file tree, quick-switch, and
  a rendered view of the selected file; or run `fathomable README.md` to view a
  single file.
- A **renderer**: pretty Markdown (tables, nested lists, task lists, footnotes,
  links) and syntax-highlighted code, with a source-view toggle for Markdown.
- A **follower**: watched files re-render on change while preserving the
  reader's position; transient counted file-edit toasts report new work
  without moving the reader or requiring acknowledgement.
- A **reviewer**: line-range annotations on rendered content, captured with the
  snippet, the source range, a timestamp, and the user's comment. Annotations
  form threads. Agents reply into threads, so a document can carry a
  long-running local review conversation across many agent sessions.
- A **diff lens**: one pinned, checkout-wide comparison and session-global
  Standard, Unified, or Off presentation applies to every file. Standard
  supplies comparison gutters and navigation, Unified is a durable patch view,
  and Off is Target-only source browsing without comparison, Git-status, or
  file-edit toasts. Explicit review points provide deliberate
  "since this workspace state" comparisons.
- An **agent endpoint**: `fathomable --mcp [DIR]` is a repository-bound stdio
  MCP server that works without a viewer, so agents can read discussions,
  start threads, and reply to them.
- A **user-mediated discussion space**: reviewers, coders, and the human may
  use the same thread history. The user's stated decision and task assignment,
  not an inferred queue state, determine what an agent should do.
- **Modal and mouse-discoverable**: Vim grammar for navigation, `:` command
  line, `/` search, a persistent workflow menu bar, and first-class mouse
  support so selecting lines to annotate is a drag.

## What Fathomable is not

Permanent non-goals:

- **Not a text editor.** Fathomable never writes to the files it displays.
  Annotations and session state live outside the workspace.
- **Not an agent runtime or chat client.** It does not run, host, or converse
  with an agent; it exposes an MCP endpoint and otherwise stays out of the way.
- **Not an IDE or file manager.** No build, run, rename, move, or delete.
- **Not bound to one agent product.** Any stdio MCP client can read;
  writes require a supported harness identity channel
  ([0080](decisions/0080-automatic-chat-identity.md)).
- **Not an automatic agent scheduler or delivery service.** It does not
  subscribe, assign, acknowledge, automatically wake, remind, or route chats,
  and reading a thread does not authorize work. The visible manual wake key
  is an unimplemented placeholder for a future user-directed integration.

Deferred, not rejected:

- Sixel/Kitty image rendering, rendered Mermaid diagrams, syntax highlighting
  inside Markdown inline code.
- Helix-style selection-first key grammar.
- Multiple panes inside Fathomable (v1 is one pane; the layout is designed so
  splitting can be added).
- HTTP transport for the MCP server.
- A user-triggered wake integration that sends all open threads and an
  optional instruction to a chosen chat
  ([0082](decisions/0082-three-tool-review-core.md)).
- Cross-workspace MCP routing. A server is bound to one repository checkout;
  shared worktree storage and manual viewer worktree navigation remain.
- macOS and Windows. Linux is the only supported platform.

## Principles

1. **Read-only by construction.** The workspace is an input. Every write goes to
   XDG state or config directories, never to the watched tree.
2. **Follow, don't fight.** Mutable endpoints refresh while immutable commits
   stay pinned. The viewer preserves the reader's position and projects
   threads from bounded evidence rather than blocking updates or pretending an
   ambiguous match is current.
3. **Rendered view, source truth.** The human annotates what they see; the
   record that reaches the agent carries the source range and snippet so the
   agent can act on it.
4. **Modal and discoverable.** Vim grammar by default, with hints so the keymap
   is learnable without a manual. Mouse is a peer to the keyboard, not an
   afterthought.
5. **Boring dependencies.** Few, large, widely depended-on, organization- or
   well-known-maintainer-owned crates with locked versions; policy enforced by
   `cargo deny`, exceptions recorded as decisions
   (see [Dependency policy](decisions/0001-dependency-policy.md)).
6. **Correctness first.** The workspace lint set and local gates are the floor.
   Rendering, anchoring, and session logic live in a core crate that is tested
   without a terminal.
7. **Plain-file interop.** Annotations, sessions, and config are plain files in
   documented formats. An agent or a human can read them with `cat`.
8. **Decisions are written down.** Non-obvious choices get a decision record in
   [Design decisions](decisions/index.md) before or with the code.

## Vocabulary

One word per idea ([0047](decisions/0047-one-vocabulary.md)):

- **Workspace**: the directory tree Fathomable is viewing, and the home of
  its threads; there is no separate word for a workspace's annotation state.
- **Viewer**: one running Fathomable showing a workspace, named or by id.
- **Agent session**: the concrete harness chat an agent runs in, identified
  automatically by a harness-qualified native id, independently of its
  repository binding
  ([0080](decisions/0080-automatic-chat-identity.md)).
- **Thread**: a comment on a line range of a document plus the replies
  (human or agent) attached to it. The opening message is the **comment**.
- **Anchor**: the durable identity of a thread's range, derived from line
  content hashes so it survives re-renders.
- **Placement**: where a thread's lines are now: anchored, edited, or
  detached.
- **Lifecycle**: active, resolution proposed, or resolved
  ([0085](decisions/0085-thread-lifecycle-and-auto-resolve.md)).
- **Mark**: a thread placed in the text as the viewer draws it (code only).
- **Placement context**: the checkout or historical endpoint in which bounded
  evidence can currently place a thread. It qualifies a location; it does not
  decide repository-board membership.
- **Change**: a filesystem edit observed by the watcher, or a path in the
  selected comparison. An observed edit is not queued for acknowledgement.
- **Sidebar**: the left column, holding the **files pane** and the
  **threads pane** as peers ([0049](decisions/0049-inline-threads-and-the-rail.md),
  named by [0057](decisions/0057-the-sidebar.md)).
- **Stub**: the condensed block a thread shows under its lines, collapsed
  to two rows or expanded to the whole thread.
- **Review list**: the `Space t` view of every thread on the work.
- **Review point**: an explicit saved workspace manifest used as a temporal
  comparison base. It is not a review session, task boundary, or automatic
  reader snapshot.
- **Jumplist**: the positions far moves leave behind, walked with
  `Alt-Left` and `Alt-Right`.
