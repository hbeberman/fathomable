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
  (`user` or an agent-supplied name), timestamp, and body. Threads can be
  resolved.
- Anchors are content hashes (`sha2`) of each annotated line plus a small
  context of neighboring lines. On refresh the anchor is re-located by hash;
  when the exact lines are gone the annotation is shown as detached at its
  last known position. Smarter re-anchoring for modified lines is a future
  decision.
- Storage is append-only JSON Lines under
  `$XDG_STATE_HOME/fathomable/workspaces/<workspace-hash>/threads.jsonl`, keyed
  by workspace root path. The workspace tree is never written to.
- Selection for annotation is by mouse drag or Vim visual mode; the comment
  is entered in a small input box, the only text entry in Fathomable.
- Agents read threads via the MCP `annotations_list` tool and reply via
  `thread_reply`; there is no push. The user tells the agent when to check.

## Consequences

- The record format is a public contract and is versioned.
- Detached annotations are visible rather than silently lost, which keeps
  "follow, don't fight" honest.
