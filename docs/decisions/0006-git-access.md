---
type: Decision
title: Git access
description: Use gix for gutter status and diff views instead of shelling out or binding libgit2.
tags:
  - decision
  - git
---

# 0006 Git access

Status: accepted (2026-08-26)

## Context

Fathomable needs per-line change status for a gutter strip and whole-file
diffs. Shelling out to host git is rejected; `git2` builds libgit2 C code.

## Decision

- Use `gix` (GitoxideLabs) for repository discovery, HEAD blob access, status,
  and diffing, behind an internal `Vcs` trait in `fathomable-core`.
- Two diff bases are supported: **HEAD** (working tree vs last commit) and
  **last seen** (working tree vs the content Fathomable snapshotted when the
  user last viewed the file). Snapshots live in the session state directory.
- The gutter strip colors added, modified, and removed lines like Zellij and
  editors do; diff views render side-by-side or unified in the same layout
  engine.
- Navigation commands jump between hunks and between changed files so a user
  can walk through what an agent has done.

## Consequences

- `gix` is the heaviest dependency in the tree; it is accepted under the
  dependency policy as organization-owned pure Rust.
- "Last seen" snapshots grow state; they are bounded per workspace and pruned.
