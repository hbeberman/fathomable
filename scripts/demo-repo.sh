#!/usr/bin/env bash
# Create a throwaway git workspace, register it with Fathomable, and seed
# threads and a subscriber so `--mcp`, the hooks, and an agent can be
# smoke-tested against it (ADR 0040). Needs bash, git, and an installed
# `fathomable` (or FATHOMABLE=path/to/binary).
#
#   scripts/demo-repo.sh [--isolated] [DIR]
#
# DIR defaults to a fresh mktemp directory. With --isolated, Fathomable's
# state and config live under DIR/.xdg instead of the caller's real
# $XDG_STATE_HOME / $XDG_CONFIG_HOME; the exports to reuse are printed.
set -euo pipefail

FATHOMABLE=${FATHOMABLE:-fathomable}
ISOLATED=0
if [ "${1:-}" = "--isolated" ]; then ISOLATED=1; shift; fi
DIR=${1:-$(mktemp -d -t fathomable-demo.XXXXXX)}
mkdir -p "$DIR"
DIR=$(cd "$DIR" && pwd -P)

if [ "$ISOLATED" = 1 ]; then
    export XDG_STATE_HOME="$DIR/.xdg/state" XDG_CONFIG_HOME="$DIR/.xdg/config"
    mkdir -p "$XDG_STATE_HOME" "$XDG_CONFIG_HOME"
fi

# 1. A small repository with real line content: threads anchor by line hash.
cd "$DIR"
git init -q -b main .
mkdir -p src docs
cat > README.md <<'EOF'
# Demo workspace

A throwaway repository for exercising Fathomable's agent loop.

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

- Anything beyond `src/` and this file.
EOF
cat > Cargo.toml <<'EOF'
[package]
name = "demo"
version = "0.1.0"
edition = "2024"
EOF
printf '.claude/\n.xdg/\ntarget/\n' > .gitignore
git -c user.name=demo -c user.email=demo@example.invalid add -A
git -c user.name=demo -c user.email=demo@example.invalid commit -q -m "chore: demo workspace"
HEAD=$(git rev-parse HEAD)

# Hooks for a Claude session started inside this repo (guide §8).
mkdir -p .claude
cat > .claude/settings.local.json <<'EOF'
{
  "hooks": {
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook claude", "timeout": 5 }] }],
    "PostToolUse":      [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook claude", "timeout": 5 }] }],
    "Stop":             [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook claude", "timeout": 5 }] }]
  }
}
EOF

# 2. Make the workspace known without a viewer.
STATE_DIR=$("$FATHOMABLE" --register "$DIR" | sed -n 2p)

# 3–4. Seed threads and a subscriber through the binary, so the store
# formats have one writer (`fathomable seed`, hidden; its file shape is
# documented in crates/fathomable/src/seed.rs).
AGENT_ID=claude:${DEMO_AGENT_ID:-demo-1}
SEED=$(mktemp -t fathomable-seed.XXXXXX.json)
trap 'rm -f "$SEED"' EXIT
cat > "$SEED" <<EOF
{
  "threads": [
    {"key": "lib-open", "path": "src/lib.rs", "line": 9, "end_line": 11,
     "comment": "This splits on a single space; two spaces in a row give a phantom word. Use split_whitespace."},
    {"key": "readme-replied", "path": "README.md", "line": 14, "end_line": 16,
     "comment": "Please update this table once the fix lands.",
     "replies": [{"author": {"name": "rev", "id": "claude:other", "type": "reviewer"},
                  "body": "Agreed; the coder should do this after fixing word_count."}]},
    {"key": "plan-open", "path": "docs/plan.md", "line": 3, "end_line": 5,
     "comment": "Step 2 first: a failing test for the empty string proves the fix."},
    {"key": "main-resolved", "path": "src/main.rs", "line": 4, "end_line": 5,
     "comment": "Fine as it is.", "resolved": true},
    {"key": "readme-detached", "path": "README.md", "line": 3,
     "comment": "This paragraph was rewritten; the thread no longer matches any line.",
     "detached": true}
  ],
  "subscribers": [{"id": "$AGENT_ID", "type": "coder", "name": "demo"}],
  "watches": [{"subscriber": "$AGENT_ID", "on": "plan-open", "when": "resolved",
               "remind": ["lib-open"]}]
}
EOF
"$FATHOMABLE" seed --workspace "$DIR" "$SEED"

# 5. Where everything is and what to run.
cat <<EOF
root:      $DIR
state:     $STATE_DIR
commit:    $HEAD
subscriber: $AGENT_ID (coder), watching plan-open for resolved, reminding lib-open
EOF
if [ "$ISOLATED" = 1 ]; then
    echo "export XDG_STATE_HOME=$XDG_STATE_HOME XDG_CONFIG_HOME=$XDG_CONFIG_HOME"
fi
cat <<EOF
try:
  cd $DIR
  $FATHOMABLE pending --id $AGENT_ID --prompt
  claude -p --allowedTools "mcp__fathomable__*" "You are working in this repo. Follow the Fathomable instructions you were given."
EOF
