#!/usr/bin/env bash
# @okf-doc: /decisions/0002-crate-layout.md
set -euo pipefail

verbose=0
for arg in "$@"; do
    case "$arg" in
        -v|--verbose)
            verbose=1
            ;;
        -h|--help)
            cat <<'USAGE'
Usage: check-boundaries.sh [-v|--verbose]

Runs source-level boundary tripwires for workspace crates.

Checks:
  - no public unsafe function declarations under crates/
  - no public glob re-exports under crates/
  - no source file of 1000+ lines whose inline test module is 35% or
    more of it (such tests live in a sibling tests.rs)
  - no `env!("CARGO_MANIFEST_DIR")` outside fathomable-testing, whose
    `repo_file` reads the variable at run time so reused test binaries
    find files in the current checkout rather than their build location
  - fathomable-core does not depend on ratatui, crossterm, or rmcp
    (docs/decisions/0002-crate-layout.md)

The compiler-wide workspace lint forbids unsafe code. The remaining source
checks supplement compiler validation; exported API shape checks live in
scripts/check-public-api.sh.

Requires ripgrep and Python 3.
USAGE
            exit 0
            ;;
        *)
            printf 'unknown argument: %s\n' "$arg" >&2
            exit 2
            ;;
    esac
done

command -v rg >/dev/null 2>&1 || {
    printf 'boundary check failed: ripgrep (rg) is required\n' >&2
    exit 1
}
command -v python3 >/dev/null 2>&1 || {
    printf 'boundary check failed: python3 is required\n' >&2
    exit 1
}

matches=$(mktemp)
trap 'rm -f "$matches"' EXIT

log() {
    if [[ $verbose -eq 1 ]]; then
        printf '%s\n' "$1"
    fi
}

fail() {
    if [[ -s "$matches" ]]; then
        printf -- '----- boundary matches -----\n' >&2
        cat "$matches" >&2
    fi
    printf 'boundary check failed: %s\n' "$1" >&2
    exit 1
}

deny_matches() {
    local description=$1
    shift

    log "checking: $description"
    : >"$matches"
    if rg --color never "$@" >"$matches"; then
        fail "source tripwire: $description"
    else
        status=$?
        if [[ $status -ne 1 ]]; then
            fail "ripgrep failed while checking: $description"
        fi
    fi
}

deny_matches \
    "public unsafe function declaration found" \
    -n --type rust '^[[:space:]]*pub(\([^)]*\))?[^{;]*\bunsafe\b[^{;]*\bfn\b' \
    crates

deny_matches \
    "compile-time CARGO_MANIFEST_DIR outside fathomable-testing::repo_file" \
    -n --type rust -g '!crates/fathomable-testing/src/lib.rs' \
    'env!\("CARGO_MANIFEST_DIR"\)' \
    crates

log "checking: public glob re-exports and inline test modules"
scripts/check-rust-source-policy.py

log "checking: fathomable-core terminal and MCP dependencies"
: >"$matches"
core_deps_script='
import json, sys
forbidden = {"ratatui", "crossterm", "rmcp"}
metadata = json.load(sys.stdin)
for package in metadata["packages"]:
    if package["name"] != "fathomable-core":
        continue
    for dependency in package["dependencies"]:
        if dependency["name"] in forbidden:
            print("fathomable-core depends on " + dependency["name"])
'
if ! cargo metadata --format-version 1 --no-deps | python3 -c "$core_deps_script" >"$matches"; then
    fail "cannot inspect fathomable-core dependencies"
fi
if [[ -s "$matches" ]]; then
    fail "fathomable-core must not depend on ratatui, crossterm, or rmcp"
fi
