---
type: Decision
title: Configuration format
description: KDL configuration in the XDG config directory.
resource: crates/fathomable-core/src/config.rs
tags:
  - decision
  - configuration
---

# 0008 Configuration format

Status: accepted (2026-08-26); amended 2026-09-03 by
[0047](0047-one-vocabulary.md): the `follow` block is `jump { auto
debounce toast }` and `watch { ignore debounce }`, and `seen-idle` is
under `viewer`; a `follow` block is an error naming each setting's new
home.

Amended 2026-09-15 by [0081](0081-the-menu-bar.md): startup chrome and
sidebar state live under `layout`; the former top-level `sidebar` block is
retired without a compatibility alias.

Amended 2026-09-15 by [0082](0082-three-tool-review-core.md): remove the
`agents` block and `jump.auto` / `jump.debounce`. `jump.toast` and filesystem
`watch.debounce` remain. Removed settings are unknown-key errors; the
upgrade never rewrites installed configuration.

## Context

Fathomable needs user configuration (theme, follow behavior, later keymaps)
and the user prefers KDL.

## Decision

- Configuration is KDL, parsed with the `kdl` crate, at
  `$XDG_CONFIG_HOME/fathomable/config.kdl`; themes at
  `$XDG_CONFIG_HOME/fathomable/themes/*.kdl`.
- Missing config is valid; every setting has a default. Unknown nodes are
  errors with a location, not silently ignored.
- XDG paths are resolved from environment variables with the standard library.

## Consequences

- KDL is the one place Fathomable uses a non-JSON text format; state files
  stay JSON Lines for machine consumers.
