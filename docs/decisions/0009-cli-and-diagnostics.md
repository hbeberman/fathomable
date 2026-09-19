---
type: Decision
title: CLI and diagnostics
description: Command-line shape, admin flags, and logging so agents can debug Fathomable from its own output.
resource: crates/fathomable/src/main.rs
related_resources:
  - crates/fathomable/src/doctor.rs
  - crates/fathomable/src/logging.rs
  - crates/fathomable/examples/seed.rs
  - crates/fathomable-core/src/xdg.rs
  - crates/fathomable-core/src/private_state.rs
tags:
  - decision
  - diagnostics
---

# 0009 CLI and diagnostics

Status: accepted (2026-08-26)

Runtime transport retired 2026-09-18 by [0089](0089-store-only-mcp.md):
`--viewers` retains viewer metadata but no socket column, `--doctor` drops
runtime/socket checks, and XDG path discovery no longer needs a runtime
directory. Viewer registrations, workspace markers, logging, and all
persistent-state privacy checks remain.

Agent-delivery CLI removed 2026-09-15 by
[0082](0082-three-tool-review-core.md): `pending` and hook diagnostics no
longer exist. The obsolete `--register` command is also removed because
`--mcp [DIR]` discovers its checkout directly and viewers maintain the
markers used by viewer diagnostics and worktree state. Existing installed
hook entries are removed manually; the program does not edit user
configuration or erase legacy state.

Terms renamed 2026-09-03 by [0047](0047-one-vocabulary.md): *session* is
*viewer* or *workspace* (the harness session keeps the word), *follow
mode* is *auto-jump*, *annotation* is *thread*, *panel* is *pane*,
`Scope` is `Reach`, *local*/*global* are *in file*/*across the
workspace*, and the `session` tool parameter is `workspace`; the text
below keeps the old words where it describes what was decided then.

## Context

Fathomable will be developed largely by agents. The user reports bugs by
pointing an agent at Fathomable's own logs and state, so the binary must be
self-inspectable.

## Decision

Command line:

- `fathomable [PATH]`: a file opens the single-file view; a directory or no
  argument opens the workspace rooted there (workspace root is the enclosing
  git root when one exists, otherwise the directory itself).
- `fathomable --mcp [DIR]`: run the stdio MCP server instead of the TUI;
  `DIR`, or startup cwd when omitted, anchors its immutable default
  workspace ([0080](0080-automatic-chat-identity.md)).
- `--config PATH` overrides the config file; `--theme NAME` selects a theme
  from `$XDG_CONFIG_HOME/fathomable/themes/` for this run.
- `--discovery-entries`, `--workspace-watches`, `--retained-paths`,
  `--comparison-paths`, `--comparison-bytes`, and `--pending-events` accept
  positive finite counts once per invocation and override the matching
  `limits` setting for both the viewer and `--config-show`.
- Admin flags, all non-interactive and printing to stdout:
  `--doctor` (terminal capabilities, XDG directories, config parse, git,
  thread-store readability and recovery, live sessions), `--sessions`
  (list sessions and sockets),
  `--dump-state` (session and thread state as JSON), `--replay-log`
  (re-emit the log for a session in order), `--config-show` (effective
  configuration after defaults and overrides).
- The hook subcommand `pending` (0040, 0042, 0080) takes
  `--verbose`: an account of every lookup — stdin, workspace, viewers,
  config, register, subscriber, thread counts — and why the hook
  stayed silent, on stderr so the harness's hook log carries it and
  stdout still means what it did; on stdout only when stderr is the
  answer (a Claude or Codex stop block).

Logging:

- `tracing` writes to `$XDG_STATE_HOME/fathomable/log/<session-id>.log`
  always; level from `FATHOMABLE_LOG` or config, default `info`.
- Log lines are structured (JSON) so an agent can grep and parse them; the
  session id appears in the TUI status so the user can name it.

### Persistent-state privacy

Application-owned persistent state directories, starting at
`$XDG_STATE_HOME/fathomable`, are created with mode **0700**. Sensitive files
are created with mode **0600**, before any content is written, independently
of permissive umasks such as 000 or 022. This includes annotation logs (also
their locking inode), review-point manifests and blobs, viewer records,
workspace markers, comparison preferences and replacement files, logs, crash
reports, and diagnostic probes.

Existing owned directories must be 0700 and owned by the effective UID.
Existing files must be regular, singly linked, 0600, and owned by that UID.
Symlinks, unsafe ownership, loose permissions, and special files are refused
with a path-specific error; files are validated before truncation. Exclusive
creation prevents a stale probe or replacement path from being overwritten.
No permissions are repaired, no stores migrated, and no old state reset or
deleted as part of this validation.

The shared `private_state` helpers use Linux metadata, exclusively claim new
paths, and obtain the kernel's effective UID from `/proc/self/status`, not an
environment UID. Existing files are validated before and after opening; a
concurrent creator requires fresh validation. No architecture-specific open
flags are needed. Every existing ancestor must be a real directory owned by
root or this UID. Non-sticky world-writable ancestors are refused.
Group-writable external ancestors are allowed without inferring trust from a
group name, UID/GID numeric equality, or group-database membership.

The accepted group sharing is reported after logging starts and as a
non-failing warning in both `fathomable --doctor` and `:doctor`. The Doctor
names each shared ancestor and recommends removing group write (`chmod g-w`)
or choosing a private `XDG_STATE_HOME`; rerunning it reflects permission
changes. The startup TUI briefly points to `:doctor`. Fathomable never chmods
HOME or an XDG base. Group members can rename entries in a shared ancestor,
interfering with availability or which otherwise-valid application tree a
path selects, so this mode does not provide strict path integrity against
those users. Missing parents are created privately. Read-only configuration
is unchanged.

`XdgDirs` path getters remain pure. `prepare_state_dir` validates every owned
component from the application root through the requested directory.
`validate_state_dir` performs the same checks on existing directories without
creating missing components. Viewer recovery uses `Store::reload_workspace`
with this validation rather than treating application-owned parents as
arbitrary external directories; an initial privacy refusal remains a refusal
until the state is safe.
`Store::open_workspace` and `ReviewPointStore::open_workspace` apply that
contract at XDG boundaries. The arbitrary-path `Store::open` instead treats
existing caller-supplied parents as external: it protects its file and creates
missing parents privately without claiming an arbitrary existing directory
as application-owned. `ReviewPointStore::open` owns and validates its supplied
store and blobs directories.

These are filesystem access controls against other local UIDs, except for the
documented path-integrity risk accepted at group-writable external ancestors.
They are not encryption, secure erasure, or a sandbox against root, the OS, or
same-UID programs.
External editors and their temporary drafts have their own
[contract](0018-comment-editor.md). Logs and reports can contain private paths
and source-derived text; users must still review them before sharing.

Note (2026-08-29): `--register [PATH]` writes the workspace marker of
[0024](0024-workspace-sessions.md) for the root around `PATH` without
starting a viewer, and prints the root and its state directory. It exists
so scripts and tests (`scripts/demo-repo.sh`) can make a directory known
to headless `--mcp` and the hooks of
[0040](0040-agent-subscriptions-and-hooks.md) without mirroring the
marker format.

Note (2026-09-05): `fathomable seed FILE [--workspace DIR]`, hidden from
`--help`, writes the threads, replies, resolutions, subscribers, and
watches a JSON file declares through `annotations::Store` and
`agents::Register` (`crates/fathomable/src/seed.rs` documents the
shape). It replaces the Python in `scripts/demo-repo.sh` that wrote
`threads.jsonl` and `agents.jsonl` by hand from a copy of the serde
shapes, so the formats have one writer. It exists for the demo and for
tests, not for users; the viewer and the tools are how threads are made.

Amended 2026-09-18: seed is no longer a hidden product subcommand. The same
Store-backed implementation lives at `crates/fathomable/examples/seed.rs`,
and `scripts/demo-repo.sh` invokes it through Cargo. Normal builds and
`cargo install --path crates/fathomable` therefore ship no seeding command.

## Consequences

- Every subsystem is expected to emit enough tracing to reconstruct a bug
  report; this is part of code review.
- Admin flags reuse `fathomable-core` readers, so they never need a terminal.
- A thread-store version mismatch keeps failed-action notices short: they
  name the on-disk and expected versions and point to `:doctor`. The shared
  Doctor report names the state file to delete and the required restart.
