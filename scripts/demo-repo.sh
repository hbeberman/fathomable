#!/usr/bin/env bash
# Create an isolated throwaway git workspace and seed review discussions so
# the three MCP tools can be tried safely (ADR 0082). Needs bash, git, and
# Cargo, and an installed `fathomable` (or FATHOMABLE=path/to/binary).
#
#   scripts/demo-repo.sh [--isolated] [DIR]
#
# DIR defaults under this checkout's ignored .tmp directory. State and config
# always live under DIR/.xdg instead of the caller's real XDG directories.
set -euo pipefail

FATHOMABLE=${FATHOMABLE:-fathomable}
if [ "${1:-}" = "--isolated" ]; then shift; fi
REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
DIR=${1:-"$REPO_ROOT/.tmp/demo-repos/$(date +%Y%m%d-%H%M%S)-$$"}
mkdir -p "$DIR"
DIR=$(cd "$DIR" && pwd -P)

case "$DIR" in
    /|"$HOME"|"$REPO_ROOT")
        echo "refusing unsafe demo directory: $DIR" >&2
        exit 2
        ;;
esac
if [ -n "$(find "$DIR" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
    echo "refusing non-empty demo directory: $DIR" >&2
    exit 2
fi
export XDG_STATE_HOME="$DIR/.xdg/state" XDG_CONFIG_HOME="$DIR/.xdg/config"
mkdir -p "$XDG_STATE_HOME" "$XDG_CONFIG_HOME"

# 1. A small repository with real line content: threads anchor by line hash.
cd "$DIR"
git init -q -b main .
mkdir -p src docs
cat > README.md <<'EOF'
# Demo workspace

A throwaway repository for exercising Fathomable review discussions.

## What is here

- `src/lib.rs` — a tiny library with one bug
- `src/main.rs` — a binary that uses it
- `docs/plan.md` — what the agent is supposed to do

## Status

| Piece | State |
| --- | --- |
| library | needs a fix |
| binary | works |
| docs | draft |
EOF
cat > src/lib.rs <<'EOF'
//! Greeting helpers for the demo binary.

/// Build a greeting for `name`.
pub fn greet(name: &str) -> String {
    format!("Hello, {name}!")
}

/// Count the words in `text`.
pub fn word_count(text: &str) -> usize {
    text.split(' ').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_by_name() {
        assert_eq!(greet("demo"), "Hello, demo!");
    }
}
EOF
cat > src/main.rs <<'EOF'
use demo::{greet, word_count};

fn main() {
    let line = greet("world");
    println!("{line}");
    println!("{} words", word_count(&line));
}
EOF
cat > docs/plan.md <<'EOF'
# Plan

1. Fix `word_count` so runs of whitespace count as one gap.
2. Add a test for the empty string.
3. Mention both in the README status table.

## Out of scope

- Anything beyond `src/lib.rs` and the README status table.
EOF
cat > Cargo.toml <<'EOF'
[package]
name = "demo"
version = "0.1.0"
edition = "2024"
EOF
printf '.xdg/\ntarget/\n' > .gitignore
git -c user.name=demo -c user.email=demo@example.invalid add -A
git -c user.name=demo -c user.email=demo@example.invalid commit -q -m "chore: demo workspace"
HEAD=$(git rev-parse HEAD)

# 2. Seed discussions through the repository-only Cargo example, so the store
# has one writer without putting demo tooling in the installed product binary.
# Neither seeding nor a repository-bound MCP server needs a workspace marker.
SEED="$DIR/.xdg/seed.json"
cat > "$SEED" <<EOF
{
  "threads": [
    {"key": "lib-open", "path": "src/lib.rs", "line": 9, "end_line": 11,
     "comment": "This splits on a single space; two spaces in a row give a phantom word. Use split_whitespace."},
    {"key": "readme-replied", "path": "README.md", "line": 14, "end_line": 16,
     "comment": "Please update this table once the fix lands.",
     "replies": [{"author": {"name": "Copilot", "client": "copilot-cli",
                              "id": "copilot:demo-review"},
                  "body": "Agreed; the coder should do this after fixing word_count."}]},
    {"key": "plan-open", "path": "docs/plan.md", "line": 3, "end_line": 5,
     "comment": "Step 2 first: a failing test for the empty string proves the fix."},
    {"key": "main-resolved", "path": "src/main.rs", "line": 4, "end_line": 5,
     "comment": "Fine as it is.", "resolved": true},
    {"key": "readme-detached", "path": "README.md", "line": 3,
     "comment": "This paragraph was rewritten; the thread no longer matches any line.",
     "detached": true}
  ]
}
EOF
cargo run --quiet --locked --manifest-path "$REPO_ROOT/Cargo.toml" \
    -p fathomable --example seed -- --workspace "$DIR" "$SEED"
rm -f "$SEED"

# 3. Where everything is and what to run.
cat <<EOF
root:      $DIR
commit:    $HEAD
export XDG_STATE_HOME=$XDG_STATE_HOME XDG_CONFIG_HOME=$XDG_CONFIG_HOME
EOF
cat <<EOF
try:
  cd $DIR
  $FATHOMABLE
  claude -p --allowedTools "mcp__fathomable__*" "Read the Fathomable threads for this repository and their history. The user approved fixing word_count with split_whitespace, adding an empty-string test, and updating the README status. Implement only that scope, explain any disagreement in thread_reply, and do not close threads; only the user closes them."
EOF
