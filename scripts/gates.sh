#!/usr/bin/env bash
set -euo pipefail

gate_root=${FATHOMABLE_GATE_ROOT:-}
if [[ -z $gate_root ]]; then
    gate_root=$(git rev-parse --show-toplevel)
fi
cd "$gate_root"

verbose=${FATHOMABLE_HOOK_VERBOSE:-0}

script_args=()
if [[ $verbose -eq 1 ]]; then
    script_args=(-v)
fi

run() {
    local label=$1
    shift

    if [[ $verbose -eq 1 ]]; then
        printf '▶ %s\n' "$label"
        "$@"
        return
    fi

    local log
    log=$(mktemp)
    if "$@" >"$log" 2>&1; then
        printf '✓ %s\n' "$label"
        rm -f "$log"
    else
        local status=$?
        printf '✗ %s\n' "$label" >&2
        cat "$log" >&2
        rm -f "$log"
        exit "$status"
    fi
}

run "fmt"              cargo fmt --check
run "clippy"           cargo clippy --all-targets --all-features -- -D warnings -F unsafe-code
run "nextest"          cargo nextest run --all-targets --all-features
run "doctest"          scripts/test-doctests.sh
run "okf"              python3 scripts/okf-lint.py --repo-root . docs
run "links"            lychee --offline --no-progress docs README.md AGENTS.md .agents/skills/open-knowledge-format/SKILL.md
run "boundaries"       scripts/check-boundaries.sh "${script_args[@]}"
run "rustdoc"          env RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --all-features
run "public-api"       scripts/check-public-api.sh "${script_args[@]}"
run "audit"            cargo audit
run "deny"             cargo deny check
run "unused-dependencies" cargo +nightly udeps --all-targets --all-features
