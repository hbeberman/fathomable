# @okf-doc: /commit-hooks.md
set shell := ["bash", "-cu"]

default:
    @just --list

# Check the current checkout without hiding unstaged edits.
gates:
    prek run --config prek.toml --all-files

gates-verbose:
    prek run --config prek.toml --all-files --verbose

fmt:
    python3 scripts/rust-toolchain.py release cargo fmt

fmt-check:
    prek run --config prek.toml --all-files fmt

clippy:
    prek run --config prek.toml --all-files clippy

test:
    prek run --config prek.toml --all-files nextest

doctest:
    prek run --config prek.toml --all-files doctest

doc:
    prek run --config prek.toml --all-files rustdoc

okf:
    prek run --config prek.toml --all-files okf

links:
    prek run --config prek.toml --all-files links

docs-check:
    prek run --config prek.toml --all-files okf links

boundaries:
    prek run --config prek.toml --all-files boundaries

public-api:
    prek run --config prek.toml --all-files public-api

audit:
    prek run --config prek.toml --all-files audit

deny:
    prek run --config prek.toml --all-files deny

udeps:
    cargo +nightly udeps --workspace --all-targets --all-features --locked

licenses:
    prek run --config prek.toml --all-files licenses

mutants:
    cargo mutants --workspace --all-features

mutants-file file:
    cargo mutants --file {{quote(file)}} --all-features

perf path="." bin="":
    scripts/perf-record.sh {{if bin == "" { "" } else { "--bin " + quote(bin) }}} -- {{quote(path)}}

demo *ARGS:
    scripts/demo-repo.sh {{ARGS}}

install-commit-hooks:
    scripts/install-commit-hooks.sh

test-commit-hooks:
    prek run --config prek.toml --all-files commit-hooks

install:
    cargo +stable install --path crates/fathomable --locked

package:
    scripts/check-licenses.sh
    python3 scripts/rust-toolchain.py release cargo package --workspace --exclude fathomable-testing --locked

release:
    scripts/check-licenses.sh
    python3 scripts/rust-toolchain.py release cargo build --release --locked --bin fathomable --target x86_64-unknown-linux-gnu

build-deps:
    scripts/setup-build-deps.sh

clean:
    cargo clean
