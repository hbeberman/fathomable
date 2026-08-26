#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$repo_root"

fail() {
    printf 'staged gate failed: %s\n' "$*" >&2
    exit 1
}

if [[ ! -d .git || -L .git ]]; then
    fail "linked worktrees and external Git directories are not supported"
fi

git_root=$(env -u GIT_DIR -u GIT_WORK_TREE -u GIT_COMMON_DIR git rev-parse --show-toplevel 2>/dev/null) \
    || fail "not a Git repository: $repo_root"
git_root=$(cd "$git_root" && pwd -P)
[[ $git_root == "$repo_root" ]] || fail "repository root mismatch: $git_root"

snapshot=$(mktemp -d "${TMPDIR:-/tmp}/fathomable-staged.XXXXXX")
cleanup() {
    rm -rf -- "$snapshot"
}
trap cleanup EXIT

env -u GIT_DIR -u GIT_WORK_TREE -u GIT_COMMON_DIR \
    git checkout-index --all --prefix="$snapshot/" \
    || fail "cannot check out the staged index"

gate="$snapshot/scripts/gates.sh"
[[ -x $gate ]] || fail "staged scripts/gates.sh is missing or not executable"

env \
    CARGO_TARGET_DIR="$repo_root/target" \
    FATHOMABLE_GATE_ROOT="$snapshot" \
    "$gate"
