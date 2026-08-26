---
type: Decision
title: Annotations and threads
description: The annotation record, thread model, anchoring by content hash, and where it is stored.
tags:
  - decision
  - annotations
---

# 0005 Annotations and threads

Status: accepted (2026-08-26)

## Context

Annotations are Fathomable's feedback channel to agents and must survive the
agent rewriting the file. The user wants long-running, review-style threads
across many agent sessions, in files any agent can read.

## Decision

- An annotation records: workspace-relative path, source line range, the
  captured snippet, an anchor, a creation timestamp, and the user's comment.
  There is no kind/type field.
- A thread is an annotation plus an ordered list of replies, each with author
  (`user` or an agent-supplied name), timestamp, and body.
- Resolution: an agent reply may carry `proposed_resolved`; only the user
  resolves. As an escape hatch an agent may force-resolve, and the thread is
  then marked `auto_resolved` and shown distinctly in the TUI.
- Anchors are content hashes (`sha2`) of each annotated line plus a small
  context of neighboring lines. On refresh the anchor is re-located by hash;
  when the exact lines are gone the annotation is shown as detached at its
  last known position. Smarter re-anchoring for modified lines is a future
  decision.
- Storage is append-only JSON Lines under
  `$XDG_STATE_HOME/fathomable/workspaces/<workspace-hash>/threads.jsonl`, keyed
  by workspace root path. The workspace tree is never written to.
- Selection for annotation is by mouse drag or Vim visual mode; the comment
  is entered in a multi-line box at the bottom of the screen: Enter adds a
  line, Ctrl-Enter submits, Esc cancels. This is the only text entry in
  Fathomable.
- Agents read threads via the MCP `annotations_list` tool and reply via
  `thread_reply`; there is no push. The user tells the agent when to check.
- The file format, the anchor algorithm, the comment-box keys, and the
  viewer side (gutter mark, thread panel, `c`, `Space a`) are fixed in
  [0013](0013-annotation-storage-and-ux.md) (2026-08-26).

## Consequences

- The record format is a public contract and is versioned.
- Detached annotations are visible rather than silently lost, which keeps
  "follow, don't fight" honest.
