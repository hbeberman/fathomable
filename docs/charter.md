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
  reader's position; a lazy "follow the agent" mode keeps the viewer near what
  the agent is touching without constantly jumping.
- A **reviewer**: line-range annotations on rendered content, captured with the
  snippet, the source range, a timestamp, and the user's comment. Annotations
  form threads. Agents reply into threads, so a document can carry a
  long-running local review conversation across many agent sessions.
- A **diff lens**: a Git gutter strip showing changed lines, and diff views for
  both "working tree vs HEAD" and "what changed since I last looked".
- An **agent endpoint**: `fathomable mcp` is a stdio MCP server that binds to a
  running session so an agent can open files, jump to locations, list
  annotations, and reply to threads.
- **Modal**: Vim grammar for navigation, `:` command line, `/` search, and
  first-class mouse support so selecting lines to annotate is a drag.

## What Fathomable is not

Permanent non-goals:

- **Not a text editor.** Fathomable never writes to the files it displays.
  Annotations and session state live outside the workspace.
- **Not an agent runtime or chat client.** It does not run, host, or converse
  with an agent; it exposes an MCP endpoint and otherwise stays out of the way.
- **Not an IDE or file manager.** No build, run, rename, move, or delete.
- **Not bound to one agent product.** Anything that can speak MCP over stdio
  can use it.

Deferred, not rejected:

- Sixel/Kitty image rendering, rendered Mermaid diagrams, syntax highlighting
  inside Markdown inline code.
- Helix-style selection-first key grammar.
- Multiple panes inside Fathomable (v1 is one pane; the layout is designed so
  splitting can be added).
- HTTP transport for the MCP server.
- macOS and Windows. Linux is the only supported platform.

## Principles

1. **Read-only by construction.** The workspace is an input. Every write goes to
   XDG state or config directories, never to the watched tree.
2. **Follow, don't fight.** Live updates always win. The viewer preserves the
   reader's position and re-anchors annotations rather than blocking or
   discarding updates.
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

- **Workspace**: the directory tree Fathomable is viewing.
- **Session**: one running Fathomable instance bound to one workspace root,
  discoverable by agents.
- **Annotation**: a user comment attached to a line range of a document, with
  its captured snippet and source mapping.
- **Thread**: an annotation plus the replies (human or agent) attached to it.
- **Anchor**: the durable identity of an annotated range, derived from line
  content hashes so it survives re-renders.
