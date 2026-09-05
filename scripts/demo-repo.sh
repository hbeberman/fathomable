#!/usr/bin/env bash
# Create a throwaway git workspace, register it with Fathomable, and seed
# threads and a subscriber so `--mcp`, the hooks, and an agent can be
# smoke-tested against it (ADR 0040). Needs bash, git, python3, and an
# installed `fathomable` (or FATHOMABLE=path/to/binary).
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
    "SessionStart":     [{ "hooks": [{ "type": "command", "command": "fathomable hello --hook claude", "timeout": 5 }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook claude", "timeout": 5 }] }],
    "PostToolUse":      [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook claude", "timeout": 5 }] }],
    "Stop":             [{ "hooks": [{ "type": "command", "command": "fathomable pending --hook claude", "timeout": 5 }] }]
  }
}
EOF

# 2. Make the workspace known without a viewer.
STATE_DIR=$("$FATHOMABLE" --register "$DIR" | sed -n 2p)

# 3–4. Seed threads and a subscriber. The JSONL shapes are those of
# fathomable-core `annotations::Event` and `agents::Event`.
AGENT_ID=${DEMO_AGENT_ID:-demo-1}
python3 - "$DIR" "$STATE_DIR" "$HEAD" "$AGENT_ID" <<'EOF'
import hashlib, json, os, sys, time

root, state, head, agent = sys.argv[1:5]
now = int(time.time())
CONTEXT = 3

def h(s):
    return hashlib.sha256(s.rstrip().encode()).hexdigest()[:16]

def lines_of(path):
    return open(os.path.join(root, path)).read().split("\n")[:-1]

def annotate(n, path, start, end, comment, detached=False):
    ls = lines_of(path)
    body = ls[start - 1:end]
    anchor = {"lines": [h(l) for l in body] if not detached else ["0" * 16] * len(body)}
    if start > 1:
        anchor["before"] = h(ls[start - 2])
    if end < len(ls):
        anchor["after"] = h(ls[end])
    ctx = {"lines": body}
    before = ls[max(0, start - 1 - CONTEXT):start - 1]
    after = ls[end:end + CONTEXT]
    if before: ctx["before"] = before
    if after: ctx["after"] = after
    tid = f"{now}-demo-{n}"
    event = {
        "event": "annotate", "v": 2, "id": tid, "path": path,
        "range": {"start": start, "end": end}, "snippet": "\n".join(body),
        "anchor": anchor, "created": now - 600 + n, "comment": comment,
        "commit": head,
    }
    # A detached thread carries no context either, or ADR 0038 would
    # place it again from the surrounding lines.
    if not detached:
        event["context"] = ctx
    return tid, event

events, ids = [], {}
t, e = annotate(1, "src/lib.rs", 9, 11, "This splits on a single space; two spaces in a row give a phantom word. Use split_whitespace.")
events.append(e); ids["lib-open"] = t
t, e = annotate(2, "README.md", 14, 16, "Please update this table once the fix lands.")
events.append(e); ids["readme-replied"] = t
events.append({"event": "reply", "v": 2, "thread": t,
               "author": {"name": "rev", "id": "other", "kind": "reviewer"},
               "created": now - 500, "body": "Agreed; the coder should do this after fixing word_count."})
t, e = annotate(3, "docs/plan.md", 3, 5, "Step 2 first: a failing test for the empty string proves the fix.")
events.append(e); ids["plan-open"] = t
t, e = annotate(4, "src/main.rs", 4, 5, "Fine as it is.")
events.append(e); ids["main-resolved"] = t
events.append({"event": "resolve", "v": 2, "thread": t, "created": now - 400})
t, e = annotate(5, "README.md", 3, 3, "This paragraph was rewritten; the thread no longer matches any line.", detached=True)
events.append(e); ids["readme-detached"] = t

os.makedirs(state, exist_ok=True)
with open(os.path.join(state, "threads.jsonl"), "a") as f:
    for e in events:
        f.write(json.dumps(e, separators=(",", ":")) + "\n")
with open(os.path.join(state, "agents.jsonl"), "a") as f:
    f.write(json.dumps({"event": "subscribe", "v": 1, "id": agent, "kind": "coder",
                        "name": "demo", "created": now}) + "\n")
    f.write(json.dumps({"event": "watch", "v": 1, "id": agent, "on": ids["plan-open"],
                        "when": "resolved", "remind": [ids["lib-open"]], "created": now}) + "\n")

print("threads:")
for k, v in ids.items():
    print(f"  {k:16} {v}")
EOF

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
