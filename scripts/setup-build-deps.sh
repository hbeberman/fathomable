#!/usr/bin/env bash
set -euo pipefail

rustup toolchain install stable --profile minimal --component clippy,rustfmt
rustup toolchain install nightly --profile minimal

cargo install cargo-public-api --locked --version 0.51.0
cargo install cargo-audit --locked --version 0.22.1
cargo install cargo-deny --locked --version 0.19.9
cargo install cargo-nextest --locked --version 0.9.138
cargo install cargo-mutants --locked --version 27.1.0
cargo +nightly install cargo-udeps --locked --version 0.1.61
cargo install lychee --locked --version 0.24.2

if ! command -v rg >/dev/null 2>&1; then
    printf '%s\n' "ripgrep (rg) is required for boundary scripts; install it with your OS package manager." >&2
fi
if ! command -v make >/dev/null 2>&1; then
    printf '%s\n' "GNU Make is required for project commands; install it with your OS package manager." >&2
fi
if ! command -v python3 >/dev/null 2>&1; then
    printf '%s\n' "Python 3 is required for public API discovery, OKF linting, and perf metadata parsing." >&2
else
    python3 -m pip install --user --requirement requirements-docs.txt
fi
