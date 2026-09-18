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

Amended 2026-09-18 by [0089](0089-store-only-mcp.md): `watch.debounce`
continues to group workspace and Git changes, while thread-store refresh
bypasses that quiet period. No new setting is required.

Amended later 2026-09-18: `jump` is retired. The shared five-second toast
duration moves from `jump.toast` to `watch.toast`; zero still disables both
file-edit and thread/activity toasts. The obsolete top-level node is an
unknown-setting error with no compatibility alias or automatic rewrite.

## Context

Fathomable needs user configuration (theme, follow behavior, later keymaps)
and the user prefers KDL.

## Decision

- Configuration is KDL, parsed with the `kdl` crate, at
  `$XDG_CONFIG_HOME/fathomable/config.kdl`; themes at
  `$XDG_CONFIG_HOME/fathomable/themes/*.kdl`.
- Missing config is valid; every setting has a default. Unknown nodes are
  errors with a location, not silently ignored.
- `--config-show` prints every effective setting with a KDL usage comment,
  including units and special values. The guide's default example matches
  that text; comments do not change parsing or round-trip values.
- XDG paths are resolved from environment variables with the standard library.

## Consequences

- KDL is the one place Fathomable uses a non-JSON text format; state files
  stay JSON Lines for machine consumers.
