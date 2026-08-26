#!/usr/bin/env bash
set -euo pipefail

verbose=0
for arg in "$@"; do
    case "$arg" in
        -v|--verbose)
            verbose=1
            ;;
        -h|--help)
            cat <<'USAGE'
Usage: check-public-api.sh [-v|--verbose]

Runs cargo-public-api for library crates, then applies repo API tripwires.

API-surface tripwires inspect exported signatures:
  - common third-party implementation types leaked through public APIs
  - smart pointer, interior mutability, or channel wrappers in public APIs
  - Deref/DerefMut impls that need public-surface review
  - public get_* functions that should follow Rust naming conventions
  - bare bool parameters in public functions

Requires ripgrep, Python 3, nightly Rust, and cargo-public-api.
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
    printf 'public API check failed: ripgrep (rg) is required\n' >&2
    exit 1
}
command -v python3 >/dev/null 2>&1 || {
    printf 'public API check failed: python3 is required\n' >&2
    exit 1
}

log=$(mktemp)
api_output=$(mktemp)
matches=$(mktemp)
metadata=$(mktemp)
packages_file=$(mktemp)
trap 'rm -f "$log" "$api_output" "$matches" "$metadata" "$packages_file"' EXIT

fail() {
    if [[ -s "$log" ]]; then
        cat "$log" >&2
    fi
    if [[ -s "$matches" ]]; then
        printf -- '----- tripwire matches -----\n' >&2
        cat "$matches" >&2
    fi
    if [[ -s "$api_output" ]]; then
        printf -- '----- captured public API surface -----\n' >&2
        cat "$api_output" >&2
    fi
    printf 'public API check failed: %s\n' "$1" >&2
    exit 1
}

if ! rustup toolchain list 2>/dev/null | rg -q '^nightly'; then
    fail "nightly Rust is required for cargo-public-api"
fi

if ! cargo +nightly public-api --version >/dev/null 2>>"$log"; then
    fail "cargo-public-api is required; run scripts/setup-build-deps.sh"
fi

if ! cargo metadata --format-version 1 --no-deps >"$metadata" 2>>"$log"; then
    fail "cargo metadata failed"
fi

if ! python3 - "$metadata" >"$packages_file" 2>>"$log" <<'PY'
import json
import sys

LIBRARY_KINDS = {
    "cdylib",
    "dylib",
    "lib",
    "proc-macro",
    "rlib",
    "staticlib",
}

with open(sys.argv[1], encoding="utf-8") as handle:
    metadata = json.load(handle)

packages = set()
for package in metadata.get("packages", []):
    targets = package.get("targets", [])
    if any(LIBRARY_KINDS.intersection(target.get("kind", [])) for target in targets):
        packages.add(package["name"])

for package in sorted(packages):
    print(package)
PY
then
    fail "cannot discover library targets from cargo metadata"
fi

mapfile -t packages <"$packages_file"

if [[ ${#packages[@]} -eq 0 ]]; then
    if [[ $verbose -eq 1 ]]; then
        printf 'no library crates found; public API check skipped\n'
    fi
    exit 0
fi

: >"$api_output"
for package in "${packages[@]}"; do
    printf '## %s\n' "$package" >>"$api_output"
    if ! cargo +nightly public-api -p "$package" --all-features --color never -sss \
        >>"$api_output" 2>>"$log"; then
        fail "cargo public-api failed for $package"
    fi
done

tripwire() {
    local source=$1
    local description=$2
    shift 2

    : >"$matches"
    if rg --color never "$@" >"$matches" 2>>"$log"; then
        fail "$source tripwire: $description"
    else
        status=$?
        if [[ $status -ne 1 ]]; then
            fail "ripgrep failed during $source tripwire: $description"
        fi
    fi
}

api_tripwire() {
    local description=$1
    local pattern=$2

    tripwire "public API" "$description" -n "$pattern" "$api_output"
}

api_tripwire "common third-party implementation type leaked through public API" \
    '(^|[^[:alnum:]_:])(anyhow|eyre|miette|tokio|serde_json|serde_yaml|reqwest|hyper|axum|clap|uuid|time|chrono|regex)::'

api_tripwire "smart pointer or channel wrapper leaked through public API" \
    '\b(std::sync::(Arc|Mutex|RwLock)|std::rc::Rc|std::cell::RefCell|tokio::sync::)'

api_tripwire "public-surface review required for Deref/DerefMut impl" \
    '^impl\b.*\bDeref(Mut)?\b.*\bfor\b'

api_tripwire "getter naming must follow Rust conventions" \
    '^pub\b.*\bfn\s+[^[:space:](]*::get_[a-zA-Z0-9_]*\s*\('

api_tripwire "bare bool parameter found in public function" \
    '^pub\b.*\bfn\s+[^()]*(\([^)]*:\s*bool\b)'

if [[ $verbose -eq 1 ]]; then
    cat "$api_output"
fi
