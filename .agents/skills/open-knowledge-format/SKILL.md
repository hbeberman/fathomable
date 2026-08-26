---
# @okf-doc: /okf.md

name: open-knowledge-format
description: "Create and maintain Open Knowledge Format knowledge docs, related resources, tags, and source ownership links. Use for architecture docs, design decisions, durable concepts, docs/index.md, @okf-doc backlinks, or OKF validation."
---

# Open Knowledge Format

The `docs/` bundle follows [Open Knowledge Format v0.2](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/3fcbb9f828c2f23d109c855ee403c3a4c81f3a96/okf/SPEC.md). Use it for durable project knowledge tied to implemented repository resources.

## When To Add A Concept

Add a concept document for architecture, contracts, workflows, or design decisions that a future contributor needs to understand. Do not create one for every helper, test, task file, or transient plan.

## Procedure

1. Read `docs/index.md` and the nearest related concept before editing.
2. Create or update a focused Markdown concept with YAML frontmatter containing a non-empty `type` field. Add the recommended `title` and `description` fields when they improve discovery.
3. Add a short entry to `docs/index.md` when introducing a concept.
4. Inspect the files named by the concept body and the nearest implementation boundary. Put the canonical entry point in `resource` and every additional concrete file whose behavior the concept explains in `related_resources`.
5. Do not declare directories, generated files, tests, incidental imports, or a file already owned by another concept. Abstract concepts may omit both resource fields.
6. Add a single `# @okf-doc: /concept.md` backlink to every declared local resource, and declare any existing backlink in frontmatter. Use the appropriate comment syntax for its language.
7. Use standard Markdown links between concepts. Read `docs/tags.md` before tagging and reuse an existing definition when it fits.
8. New tags are welcome when no existing term accurately describes a durable, distinct discovery concept. Add the new tag and a concise `tag_definitions` description to `docs/tags.md` in the same change; do not add synonyms, temporary work-item names, or title restatements. When tags are no longer useful, remove them or merge them into a more general tag. Tags are not a static list; they must evolve with the knowledge base to remain both concise and useful.
9. Keep tags unique, sorted, and lowercase kebab-case.
10. Run `just okf` and `just links`.

Example concept frontmatter:

```yaml
---
type: Software
title: Command-line interface
description: CLI behavior and package loading boundaries.
resource: src/hello_world/cli.py
related_resources:
  - src/hello_world/__main__.py
  - src/hello_world/greeting.py
tags: [cli, packaging]
---
```

## Bundle Rules

- The root `docs/index.md` declares `okf_version: "0.2"` in frontmatter.
- Every non-reserved concept document has parseable frontmatter and a non-empty `type`.
- `title`, `description`, `resource`, and `tags` are optional OKF fields.
- `related_resources` is a repository extension and, when present, is a list of local repository-relative paths.
- Tags are unique lowercase kebab-case strings declared in `docs/tags.md`.
- The tag vocabulary is intentionally extensible: the linter rejects undeclared tags, not well-considered additions.
- Nested `index.md` files do not contain frontmatter.
- Reserved `log.md` files use `## YYYY-MM-DD` date headings.
- Local resources use canonical repository-relative paths and cannot escape the repository.
- Source backlinks must refer to an existing document slug.
- External-resource URLs do not require source backlinks.

The repository-owned checker at `scripts/okf-lint.py` enforces these structural and ownership rules. `just links` uses Lychee in offline mode to check maintained local links and anchors deterministically.

