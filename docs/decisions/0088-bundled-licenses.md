---
type: Decision
title: Bundled license notices
description: Offline first- and third-party license notices travel with the executable and are readable from Help.
resource: crates/fathomable/src/app/licenses.rs
related_resources:
  - scripts/configs/about.toml
  - scripts/generate_licenses.py
  - scripts/check-licenses.sh
tags:
  - decision
  - dependencies
  - input
---

# 0088 Bundled license notices

Status: accepted (2026-09-17)

Toolchain selection amended 2026-09-19 by the
[independent toolchain roles](0001-dependency-policy.md#rust-toolchain-roles):
notice generation and distributed releases use the recorded release compiler,
not the floating development channel or the application's supported floor.

Crates.io distribution amended 2026-09-19 by the
[crate layout](0002-crate-layout.md#package-metadata): publishable workspace
packages remain covered by the single first-party MIT section. The generator
excludes only their exact manifests from third-party inventory and still
rejects every unrecognized path or non-crates.io dependency source.

## Decision

Fathomable's own source remains MIT-licensed. Third-party code and embedded
syntax/theme data retain their respective licenses and copyright notices;
the application's license does not replace those terms.

Each Cargo package also includes the root project `LICENSE` through the
[shared package metadata](0002-crate-layout.md#package-metadata), independently
of the executable's embedded notice bundle.

**Help > Licenses** opens the read-only, scrollable pane.
The executable embeds the complete notice bundle at compile time, so
reading it needs neither a checkout nor network access. Opening the pane
preserves the current document and parks any draft. `Esc` or a click outside
closes it; the menu bar remains reachable.

The pane displays notice text verbatim, wrapping long lines to the terminal
width without Markdown interpretation. Arrow keys or `j`/`k` scroll, the
wheel scrolls, `PgUp`/`PgDn` page, `Ctrl-u`/`Ctrl-d` half-page, and
`Home`/`End` or `g`/`G` reach the beginning/end. Resizing preserves the source
location at the top where possible and clamps the viewport. Wrapped layout
is reused between frames.

The committed bundle is generated from the locked dependency inventory and
reviewed supplementary asset notices. It includes actual license texts,
copyright and NOTICE content, package versions and source references, and
source-availability information for MPL-covered code. Package-level SPDX
metadata alone is not sufficient for syntect's embedded grammar/theme data.

`cargo-about` **0.9.2** inventories the x86_64 GNU/Linux normal/build
dependency graph and gathers license texts from cached crate sources.
`scripts/configs/about.toml` selects accepted licenses, excludes dev/private workspace crates,
and records hash-checked clarifications for combined or unrecognized license
files. The generator separately removes exact first-party workspace manifests
that cargo-about includes after they become publishable; an unrecognized path
or non-crates.io source still fails closed. For dual licenses it prefers MIT
when available; `cargo deny` remains the independent dependency-policy gate.

`scripts/generate_licenses.py` invokes `cargo about generate --frozen --fail`
and formats its JSON report as plain text, preserving package versions,
exact source links and copyright text without HTML escaping. It adds the
first-party license, separate package NOTICE/COPYRIGHT files, and reviewed
supplements from `licenses/manifest.json`. Missing-file SPDX templates are
rejected unless a matching pinned upstream notice supplies the actual terms.
The Rust standard-library/runtime inventory and syntect embedded-asset
notices remain supplemental: cargo-about does not supply them automatically.
The exact release compiler is recorded in
`licenses/manifest.json` under `rust_standard_library.release`. The generator
selects it through `scripts/rust-toolchain.py release` for Cargo and rustc,
including subprocesses of cargo-about, and verifies its release and commit
identity. It does not compare against the development channel in
`rust-toolchain.toml` or require the caller's active compiler to match.
Toolchain identities and asset hashes must still match their reviewed records.
Line endings and trailing horizontal whitespace are normalized.

This is not a byte-for-byte binary inventory. System linker/startup objects
and dynamic OS libraries are outside the bundle's scope. A new target,
toolchain, native input, or static-linking policy requires reviewing that
scope, not merely regenerating existing records.

Generation is contributor tooling, not a build step or runtime dependency.
Dependency changes require refreshing the notices, including those proposed
by Dependabot. A gate rejects stale output rather than silently updating it.
`scripts/check-licenses.sh` runs generator tests and the read-only `--check`.
Normal product installation compiles the committed bundle without installing
the notice-generation machinery.

`just release` checks that bundle before building the locked x86_64 GNU/Linux
binary with the recorded compiler. A normal source build or compatibility test
may use another supported compiler; its embedded runtime notices describe
the recorded release, not that alternative compiler. The bundle says so
explicitly. Before redistributing such a binary, review and regenerate the
runtime inventory for its compiler. MSRV support is not attribution approval
for every compiler version.

## Related contracts

- [Dependency policy](0001-dependency-policy.md)
- [The menu bar](0081-the-menu-bar.md)
- [Dependency monitoring](../dependency-monitoring.md)
- [Contributor setup](../../.github/CONTRIBUTING.md)
