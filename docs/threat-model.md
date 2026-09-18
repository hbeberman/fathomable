---
type: Concept
title: Fathomable threat model
description: Assets, local trust assumptions, and review obligations for customer-data protection.
resource: .agents/skills/fathomable-threatmodel/SKILL.md
tags:
  - architecture
  - security
  - sessions
---

# Fathomable threat model

This is the baseline for security design and review, not an audit certificate.
The contracts below describe intended boundaries; a review must establish
which protections the implementation actually enforces. Proposed hardening
does not become an implemented guarantee by appearing here.

## Scope and assets

Fathomable is a Linux, local, read-only workspace viewer with an annotation
side-car, a stdio MCP endpoint, and local viewer sockets. It is not a hosted
multi-tenant service, an agent sandbox, or an HTTP MCP service
([charter](charter.md), [MCP core](decisions/0082-three-tool-review-core.md)).
A review of this checkout does not cover a separate Kyber product, deployment,
or customer-data pipeline.

Sensitive assets include source text and paths, annotation excerpts and
messages, external-editor drafts, saved review-point content, diagnostic and
crash output, session metadata, and the user's authority over thread lifecycle.
Follow copied data as well as originals: deleting or restricting a checkout
does not erase its persisted excerpts, snapshots, logs, or backups.

## Actors and trust assumptions

Repository contributors, file contents, filenames, Markdown, thread bodies,
and model/tool output can supply untrusted data. Content is not permission to
execute a command, change a policy, disclose a file, or resolve a discussion.

Other local OS users are outside the user's trust boundary. Shared temporary
directories and traversable state ancestors must be considered even when the
original checkout is private.

The OS, administrator, and processes running as the same UID are not isolated
from one another by Fathomable. Harness identity records provenance, not an
authentication barrier against a hostile same-UID process
([socket trust](decisions/0014-mcp-server-and-socket-v1.md),
[identity](decisions/0080-automatic-chat-identity.md)).
This does not waive the narrower promises made by the MCP API: supported
callers must still encounter its checkout and user-authority restrictions.

User-selected editors, Git, the terminal, the host agent, and backup/sync
software have their own privileges and data handling. Distinguish deliberate
user delegation from behavior induced only by untrusted content. Do not claim
that Fathomable confines these external programs.

## Contract boundaries

| Boundary | Contract to preserve |
| --- | --- |
| Workspace to application | The viewed workspace is input; application state lives outside it ([charter](charter.md)). |
| MCP to checkout source | Fresh agent-driven source reads stay within the bound checkout at the actual read, including symlink resolution. This is not a sandbox for the human viewer or Git ([confined reads](decisions/0061-agents-start-threads.md#checkout-confined-reads)). |
| MCP to repository history | The server binds one repository and checkout. Linked worktrees intentionally share discussion history; stored history is not a fresh source read ([MCP core](decisions/0082-three-tool-review-core.md), [board history](decisions/0087-global-comparisons-and-board-history.md)). |
| Local process to viewer socket | Runtime-directory protection and a peer-UID check restrict other OS users; there is no same-UID impersonation guarantee ([socket trust](decisions/0014-mcp-server-and-socket-v1.md)). |
| Caller to annotation write | Anonymous reads are supported; writes require the supported harness identity channel. Identity does not choose the repository ([identity](decisions/0080-automatic-chat-identity.md), [MCP core](decisions/0082-three-tool-review-core.md)). |
| Agent reply to user authority | Reading grants nothing. Auto-resolution requires a user-granted one-shot permission; consuming it and recording the result are atomic per item, including replay behavior ([lifecycle](decisions/0085-thread-lifecycle-and-auto-resolve.md)). |

Read amendments before relying on an older decision's tool names or formats.

## Security goals to verify

These are review obligations, not assertions that every current path satisfies
them:

- Copies of private data remain private independently of umask. Examine
  temporary and persistent files, their ancestors, ownership, symlinks,
  replacements, cleanup failures, and existing installations. A private file
  does not make its later copies private.
- Permission repairs affect only verified application-owned objects; they do
  not follow links into unrelated files, chmod shared XDG/home ancestors, or
  discard user state. Filesystem permissions are not encryption or secure
  erasure, and do not protect against the trusted OS or same-UID processes.
- Untrusted text remains data when rendered or returned. Examine terminal
  control sequences, link handling, external commands, and prompt-injection
  content without assuming that rendering libraries or agent hosts solve it.
- Invalid input, denied access, partial writes, races, and exhausted resources
  produce explicit, bounded failures rather than misleading success or leaked
  content. Examine limits before allocation or expensive work.
- Diagnostics and disclosure reports minimize sensitive data. Dependency and
  distribution checks supplement, rather than replace, boundary review
  ([dependency monitoring](dependency-monitoring.md)).

## Using the model

The repository-local `fathomable-threatmodel` skill applies this baseline to
design, implementation, and review. Each assessment records its revision,
coverage, attacker prerequisites, evidence, and unresolved gaps. Confirmed
vulnerabilities, accepted trust assumptions, and unvalidated hardening proposals
remain distinct. A narrow fix or a clean diff does not establish release safety.

Responsible reporting follows repository `SECURITY.md` when available, through
a verified private GitHub reporting/advisory channel. Missing policy or an
unverified channel is a release-readiness gap, not permission to publish
undisclosed details in an issue, PR, discussion, or tracked audit report.
Agents obtain operator approval before sending reports and use synthetic,
redacted evidence rather than customer data.
