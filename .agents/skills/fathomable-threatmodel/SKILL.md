---
# @okf-doc: /threat-model.md

name: fathomable-threatmodel
description: Applies Fathomable's local trust boundaries to customer-data protection and security reviews. Use when threat modeling, auditing a release, reviewing security-sensitive changes, or changing MCP, IPC, filesystem access, persistence, rendering, diagnostics, or external-process behavior in this repository.
---

# Fathomable threat model

## Quick start

Read the [threat model](../../../docs/threat-model.md) and follow its linked
contracts, including their amendments. Treat the model as review guidance,
not proof that a protection is implemented.

Example: for an external-editor change, identify who can read the draft and
editor-created files, examine creation and cleanup paths, and verify privacy
with synthetic content under a permissive umask.

## Workflow

1. Record the revision, dirty-worktree scope, requested release or change, and
   the surfaces actually reviewed. A clean diff is not an audit of the product.
   Do not treat this checkout as evidence about a separate Kyber deployment.
2. Name the assets, attacker-controlled inputs, actor privileges, entry point,
   sensitive operation, and expected boundary before calling something a flaw.
   Distinguish another local UID, same-UID tooling, repository-controlled
   content, and an explicitly user-selected external program.
3. Read the nearest current contract and trace the relevant input to its read,
   write, display, response, or process boundary. Follow all equivalent paths:
   viewer and headless, MCP and socket, fresh and existing state, success and
   failure, concurrent calls and retries.
4. Check the applicable obligations below. Cite implementation evidence rather
   than inferring safety from Rust, a dependency, a path prefix, or a passing
   test suite. Separate documented trust assumptions from missing protections.
5. Validate with isolated, synthetic fixtures and existing tooling. Do not read
   real customer secrets, expose listeners, modify live state, or transmit
   private evidence. Keep an audit read-only unless changes are authorized.
6. Report confirmed findings separately from hypotheses and hardening advice.
   Give file/lines, prerequisites, impact, severity, confidence, evidence,
   the smallest complete fix, and remaining uncertainty.
7. For an authorized fix, preserve user intent and existing data, cover the
   public behavior and failure paths, and update the owning documentation.
   Invoke both repository-required Rust skills before Rust work; invoke
   `open-knowledge-format` before durable documentation changes. New direct
   dependencies still require explicit approval.

## Review obligations

- **Source reads:** enforce checkout confinement at the actual read, including
  symlink resolution and races; distinguish stored history from a fresh read.
- **MCP and IPC:** separate caller provenance, peer access, repository binding,
  and one-shot user authority; do not infer permission from a read or retry.
- **Persistence:** follow every sensitive copy, including snippets, review
  blobs, drafts, logs, crash reports, replacements, and backups. Check creation
  modes, accessible ancestors, ownership, links, cleanup, and existing state.
  A private source file or a restrictive developer umask is not sufficient.
- **Untrusted content:** keep filenames, text, messages, and tool output as
  data; examine terminal controls, URI handling, subprocess arguments and
  configuration, input sizes, and expensive parsing at their actual sinks.
- **Release:** inspect shipped artifacts and dependency checks separately from
  product boundaries. Report unreviewed surfaces; never turn a partial review
  into release clearance.

## Reporting and disclosure

Use a compact findings table followed by evidence, remediation order, and
coverage limits. "No confirmed findings in these surfaces" is not "secure."

Follow repository `SECURITY.md` when present. If absent, report the disclosure
policy gap; do not invent a reporting address or claim GitHub private reporting
is enabled. Get operator approval before submitting any report. Use the
verified private GitHub reporting/advisory channel, never public issues, PRs,
or discussions for undisclosed details. Minimize and redact evidence; never
include customer data or credentials. Local notes do not authorize publication.
