---
type: Software
title: Documentation system
description: Ownership and validation rules for durable project knowledge.
resource: .agents/skills/open-knowledge-format/SKILL.md
related_resources:
  - scripts/okf-lint.py
tags:
  - documentation
---

# Documentation system

Durable project knowledge lives in `docs/` as an Open Knowledge Format 0.2 bundle. `docs/index.md` is the entry point. Transient plans and handoffs stay in the Git-ignored `.tmp/` directory until reviewed and promoted.

The repository-local `open-knowledge-format` skill defines when to create concept documents, how concepts own implementation resources, and how source backlinks connect implementation to its canonical documentation.

## Validation

Run both checks after changing project documentation or a documented resource:

```sh
just okf
just links
```

`just okf` validates bundle structure, tags, resource ownership, and source backlinks. `just links` checks maintained local links and anchors without requiring network access. Both checks are part of the canonical commit gate.

A document's `resource` is the file it owns; that file carries exactly one `@okf-doc` backlink, to its owner. `related_resources` are forward pointers: each must exist, but it needs no backlink, so a decision record may point at the concept module it changed without taking it from the record that owns it, and an ADR need not own a new file at all.
