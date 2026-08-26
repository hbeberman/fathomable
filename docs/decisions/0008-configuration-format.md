---
type: Decision
title: Configuration format
description: KDL configuration in the XDG config directory.
tags:
  - decision
  - configuration
---

# 0008 Configuration format

Status: accepted (2026-08-26)

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
