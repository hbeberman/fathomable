#!/usr/bin/env bash
# @okf-doc: /commit-hooks.md
# Install the pinned cargo tooling the commit gate runs. The system
# packages it needs are listed per distribution in CONTRIBUTING.md; this
# script checks for them first so a missing header fails here, not
# twenty minutes into a `cargo install`.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

missing=0
need() {
    local command=$1
    local why=$2
    if ! command -v "$command" >/dev/null 2>&1; then
        printf 'missing: %s (%s)\n' "$command" "$why" >&2
        missing=1
    fi
}

need rustup "installs stable, release, and nightly toolchains"
need cc "links every Rust binary; gcc or build-essential"
need git "prek checks staged changes and installs Git hooks"
need rg "ripgrep drives the boundary and public API scripts"
need python3 "OKF lint, public API discovery, perf metadata"
need pkg-config "cargo-udeps and cargo-public-api locate OpenSSL and libcurl with it"
if command -v pkg-config >/dev/null 2>&1 && ! pkg-config --exists openssl; then
    printf 'missing: OpenSSL headers (openssl-devel or libssl-dev; cargo-udeps and cargo-public-api link them)\n' >&2
    missing=1
fi
if [[ $missing -eq 1 ]]; then
    printf 'install the packages above (see CONTRIBUTING.md), then rerun\n' >&2
    exit 1
fi

rustup toolchain install stable --profile minimal --component clippy,rustfmt
release=$(python3 scripts/rust-toolchain.py release)
rustup toolchain install "$release" --profile minimal --component clippy,rustfmt
rustup toolchain install nightly --profile minimal

cargo +stable install cargo-public-api --locked --version 0.51.0
cargo +stable install cargo-audit --locked --version 0.22.1
cargo +stable install cargo-about --locked --version 0.9.2 --features cli
cargo +stable install cargo-deny --locked --version 0.19.9
cargo +stable install cargo-nextest --locked --version 0.9.138
cargo +stable install cargo-mutants --locked --version 27.1.0
cargo +nightly install cargo-udeps --locked --version 0.1.61
cargo +stable install lychee --locked --version 0.24.2
cargo +stable install prek --locked --version 0.5.3

# The OKF lint imports PyYAML. Distributions package it (python3-pyyaml,
# python3-yaml); pip is the fallback, and Ubuntu refuses `pip --user`
# under PEP 668, so the failure names the package instead of aborting.
if python3 -c 'import yaml' >/dev/null 2>&1; then
    printf 'PyYAML present; skipping pip\n'
elif ! python3 -m pip install --user --requirement requirements-docs.txt; then
    printf 'pip could not install PyYAML; install python3-pyyaml (Fedora, Azure Linux) or python3-yaml (Ubuntu) instead\n' >&2
    exit 1
fi
