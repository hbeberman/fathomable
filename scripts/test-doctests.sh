#!/usr/bin/env bash
set -euo pipefail

command -v python3 >/dev/null 2>&1 || {
    printf 'doctest check failed: python3 is required\n' >&2
    exit 1
}

metadata=$(mktemp)
packages_file=$(mktemp)
cleanup() {
    rm -f "$metadata" "$packages_file"
}
trap cleanup EXIT

cargo metadata --format-version 1 --no-deps >"$metadata"
python3 - "$metadata" >"$packages_file" <<'PY'
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

mapfile -t packages <"$packages_file"
if [[ ${#packages[@]} -eq 0 ]]; then
    printf 'No library targets; doctests skipped.\n'
    exit 0
fi

for package in "${packages[@]}"; do
    cargo test --doc --package "$package" --all-features
done
