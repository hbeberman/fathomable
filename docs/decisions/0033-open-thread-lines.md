---
type: Decision
title: Open thread lines
description: The lines of the thread shown in the thread pane draw in their own colour, and an agent's reply may carry the lines it rewrote so the thread follows a rewrite the reload diff would have lost.
resource: crates/fathomable/src/app/open_thread.rs
tags:
  - decision
  - annotations
  - rendering
  - sessions
---

# 0033 Open thread lines

Status: accepted (2026-08-28)

## Context

With the thread pane open, the text shows the same `annotation.line`
tint on every annotated row ([0013](0013-annotation-storage-and-ux.md)),
so the reader cannot tell which rows the pane is talking about when
threads sit close together or overlap. The reader asked (2026-08-28) for
the open thread's lines in a distinct colour, and raised the case that
would make the colour useless: the agent edits the lines and replies.

That case is mostly covered. A local edit is followed through the
reload diff and shows as *edited* ([0019](0019-reanchoring-edited-lines.md));
an edit made while the viewer was closed is followed from the last-seen
snapshot ([0020](0020-reanchoring-across-restarts.md)). What still
detaches is the rewrite that reaches further than one line around the
range, the very thing an agent does when it "expands this" or "extracts
a function"; [0032](0032-placement-and-state.md) looked at widening the
window and left it, because pinning to an arbitrary line of a large
hunk misleads. The agent, though, knows exactly where the lines went:
it just wrote them.

Settled with the recommended options; no user round was held. The
choices:

- *What to tint?* The rows of the open pane's thread, over the
  `annotation.line` tint, under the cursor line. A detached thread
  tints nothing: its last known range is not its lines, and the red
  note in the gutter already says so.
- *How does the thread follow a large rewrite?* The agent says where
  the lines are now when it replies. A viewer-side guess was rejected
  again for the reason 0032 gives.
- *A new event?* No. The move is the `relocate` event 0019 added, so it
  persists, bumps `updated`, and shows as *edited* until the user
  answers, exactly as a followed rewrite does.

## Decision

- `app/open_thread.rs`, which this record backs, answers
  `App::open_thread_in(lines)`: whether the open pane's thread, unless
  detached, overlaps those source lines. `text_lines` patches such rows
  with `annotation.focus`, a theme key added to the
  [0011](0011-theme-schema.md) table and both bundled themes as a
  stronger version of `annotation.line`.
- `Request::ThreadReply` gains an optional `lines: LineRange`; the wire
  version stays 2 because a missing field reads as `None`. The MCP
  `thread_reply` tool exposes it as `line` and `end_line`, the names
  `open` uses. Its description tells the agent to pass them when it
  rewrote the thread's lines.
- `follow_reply_lines` re-anchors the thread onto those lines of the
  file *as it is on disk*, not the viewer's document: the agent speaks
  of the text it just wrote, which the viewer may not have reloaded. A
  range the file does not have fails the whole call; no reply is added.
  Both the viewer's `agent_reply` and the headless `--mcp` path call
  it before appending the reply.

## Consequences

- The tint follows the mark's range, so it is right after every kind
  of re-anchoring, and absent when the lines are gone.
- An agent that rewrites a block and replies with `line`/`end_line`
  leaves an *edited* thread on the new block instead of a *detached*
  one on the old range. An agent that does not pass them gets 0019's
  behaviour unchanged.
- One more theme key; a theme file that lacks it draws no extra tint.
  (Amended by [0036](0036-gutter-rows-and-focus-colour.md), 2026-08-28:
  the bundled value is a blue tint, not a stronger yellow, so the rows
  read as a different colour.)
