# Changelog

## Unreleased

- Refresh Gitoxide, MCP, Tokio, filesystem notification, Markdown, and Unicode
  dependencies for v0.1.2 while retaining the Rust 1.95 minimum.
- Honor Git replacement-ref settings in viewer revision reads; exact commit
  sources continue to ignore replacements.
- Update contributor tools, checksum-pinned Betterleaks, and CI action pins.
- Isolate RustSec database updates from the repository's commit index.
- Refresh the bundled third-party license notices.

## v0.1.1

- Discover review threads across the selected diff's commit range.
- Browse repository-wide review history with the All threads toggle.
- Browse larger repositories with expanded watch and comparison limits.
- Skip unchanged file content when comparing commits.

## v0.1.0

- Review Git diffs in a read-only Linux terminal workspace.
- Browse syntax-highlighted source and rendered Markdown.
- Exchange line-linked review comments with agents over MCP.
- Compare commits, staged changes, working files, and saved review points.
- Use keyboard shortcuts, mouse navigation, and customizable themes.
