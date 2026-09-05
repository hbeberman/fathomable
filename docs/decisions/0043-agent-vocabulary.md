---
type: Decision
title: One vocabulary for the agent-facing text
description: The MCP tool and parameter names that hook output, the pending blob, the server instructions, and the guide may name live in one table in the core crate; every such string is composed from it and tested against the live tool schema; the hello text is one shared body with a per-harness spelling of tool names and an optional harness line; and the follow tool's schema carries the configured agent types.
tags:
  - decision
  - sessions
  - documentation
  - configuration
---

# 0043 One vocabulary for the agent-facing text

Status: accepted (2026-08-29); the table in `vocabulary.rs` is backed by
[0047](0047-one-vocabulary.md) since 2026-09-03, which renamed its entries.

## Context

A Codex session on 2026-08-29 was asked to subscribe to a file and
"see what types you can pick". It searched the repository, then the
web, while the `hello` output in its context said `a type (one of:
coder, reviewer, planner)`. Two things were wrong with the text it
had. The prose did not say that Fathomable is an MCP server already
connected, so "call the `follow` tool" was a phrase to resolve rather
than a call to make — and under Codex the tool is shown as
`fathomable.follow`, under Claude Code as `mcp__fathomable__follow`,
while the text named neither. And the list of agent types reached the
model only through `hello`; the `follow` schema said "one of the
configured ones", so a session that missed the hook, or came back on
a new connection, could learn them only from the error `follow`
returns.

Behind that sits a maintenance problem. Prose that names tools and
parameters is hand-written in four places — the `hello` paragraph in
`hooks.rs`, the blob header and overflow line in `agents.rs`, and the
server instructions in `mcp.rs` — plus the ten tool descriptions,
the parameter docs that become the JSON-Schema text, and the tool
table in the guide. Two of the four are in `fathomable-core`, which
by [0002](0002-crate-layout.md) never depends on `rmcp`, so the names
it hardcodes cannot be checked against the tool definitions in the
`fathomable` crate. No test anywhere asserted that a named tool or
parameter exists: renaming `thread_watch`'s `when` would have left
every mention wrong and every gate green.

The `hello` body was also byte-identical for every harness. The
envelope already branched on `Harness` (bare stdout, `additionalContext`,
`hookSpecificOutput`); the text did not, although tool spelling and
hook cadence differ per host.

## Decision

- **One vocabulary, in core.** `fathomable_core::vocabulary` holds the
  ten tool names and the parameter names of each as constants and one
  table (`Tool { name, params }`, `ALL`). It knows names only — no
  `rmcp` types. Every agent-facing string in either crate names a tool
  or parameter through it: the blob header and overflow line, the
  `hello` body, the server instructions. A small helper, `idents`,
  lists the backticked identifiers of a text; `is_known` says whether
  one is in the table.
- **Name-level generation, not templating.** The prose stays prose,
  written where it is read; only the identifiers are interpolated. The
  shapes a model is shown (`follow { paths: [...], type: "...", id:
  "..." }`) are written the same way.
- **The test that makes it load-bearing** lives in the `fathomable`
  crate, the only place that sees both sides. It walks
  `Server::tool_router().list_all()` and asserts, both ways, that the
  vocabulary's tools and top-level parameters are exactly the live
  schema's. It then renders the `hello` body for every harness, the
  server instructions, and every tool and parameter description, and
  asserts that each backticked identifier — after the harness's
  namespace prefix is stripped — is a known name or one of a short,
  explicit allowlist of non-tool words (`hello`, `fathomable
  --register`). The core crate tests its own strings against the table
  the same way, so a rename anywhere fails a gate.
- **The guide is gate-checked, not generated.** The same test reads
  the tool table in `docs/guide.md` §8: its first column must name
  every tool and nothing else, and the identifiers in its second
  column must be known or allowlisted. The prose around it stays
  hand-written.
- **One `hello` body, per-harness spelling.** The reviewed text says
  what Fathomable is and that it is an MCP server already connected,
  lists the workspace, session, and the whole type list as labelled
  fields, shows the `follow`, `thread_reply`, and `thread_watch` call
  shapes, and names the session id once. `Harness::tool(name)` spells
  each tool as the host shows it — `mcp__fathomable__follow` on Claude
  Code, `fathomable.follow` on Codex, the bare name on Copilot and VS
  Code, whose namespacing has not been verified and is not guessed.
  `Harness::extra()` is the one other override: an optional line for a
  host whose cadence differs (Copilot: a detached shell finishing also
  brings comments). Nothing else varies; there are not four paragraphs.
- **The `follow` schema carries the types.** The server's `list_tools`
  is written by hand (the `tool_handler` macro yields to it) and sets
  `enum` on `follow`'s `type` property from `agents.types`, so the
  model sees the valid values wherever it sees the tool.
- **`hello` warns on a prefix match** (separate commit, same record):
  when the workspace it resolved is neither the cwd nor the cwd's git
  root it says so — comments left at the cwd will not reach the
  session — and names `fathomable --register`.

Note (2026-09-04): [0055](0055-six-tools.md) brings the vocabulary to
six tools, and the `hello` body's `follow` call shape carries `type`
and `id` only.

## Consequences

- A tool or parameter rename is now a one-place edit in the vocabulary
  plus the Rust field; any string still naming the old word fails
  `cargo nextest`. Adding a parameter without listing it fails the
  schema comparison; adding a tool without a guide row fails the guide
  check.
- The core crate's strings depend on a table it owns and the
  `fathomable` crate proves the table against the schema; the layering
  of [0002](0002-crate-layout.md) is kept.
- A model on Codex is told `fathomable.follow`; if Codex changes its
  spelling the fix is one match arm. Copilot and VS Code get the bare
  name until someone verifies theirs.
- The hello text is longer (a labelled block rather than a paragraph)
  and repeats the id once instead of three times, relying on the bonds
  of [0041](0041-session-bonds.md) for `thread_reply`.
- `vocabulary.rs` backs this record; `hooks.rs`, `agents.rs`, and
  `mcp.rs` keep their earlier records. The guide's §8 table and hello
  description are updated in the same change.
