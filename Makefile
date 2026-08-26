SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c
.DEFAULT_GOAL := help

PERF_BIN ?=
PERF_PATH ?= .

export FILE
export PERF_BIN
export PERF_PATH

.PHONY: help gates gates-verbose fmt fmt-check clippy test doctest doc okf links \
	docs-check boundaries public-api audit deny features udeps mutants mutants-file \
	perf install-commit-hooks build-deps clean web-install web-dev web-check \
	web-build web-lock web-toolchain install

help:
	@printf '%s\n' \
		'gates             Run the canonical local gate set' \
		'gates-verbose     Run gates with full command output' \
		'fmt               Format the workspace in place' \
		'fmt-check         Check formatting without writing' \
		'clippy            Run Clippy with warnings denied' \
		'test              Run the full test suite' \
		'doctest           Run documentation tests with default and all features' \
		'doc               Build rustdoc with warnings denied' \
		'okf               Validate the Open Knowledge Format documentation bundle' \
		'links             Check maintained local documentation links and anchors' \
		'docs-check        Run all documentation bundle checks' \
		'boundaries        Run source boundary checks' \
		'public-api        Check public API shape' \
		'audit             Audit dependencies' \
		'deny              Check licenses, sources, and bans with cargo-deny' \
		'features          Check feature combinations' \
		'udeps             Check unused dependencies' \
		'mutants           Run mutation testing' \
		'mutants-file      Mutate FILE=<path>' \
		'perf              Profile fathomable [PERF_PATH] until it exits' \
		'web-install       Install locked web dependencies (when configured)' \
		'web-dev           Start Vite (when configured)' \
		'web-toolchain     Verify supported Node and npm versions' \
		'web-check         Run Biome and TypeScript checks (when configured)' \
		'web-build         Build with Vite/Rolldown (when configured)' \
		'web-lock          Reject npm lock drift (when configured)' \
		'install-commit-hooks Install or refresh the local commit hook' \
		'install           Install fathomable into ~/.cargo/bin' \
		'build-deps        Install optional Cargo tooling' \
		'clean             Remove build artifacts'

gates:
	scripts/gates.sh

gates-verbose:
	FATHOMABLE_HOOK_VERBOSE=1 scripts/gates.sh

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings -F unsafe-code

test:
	cargo nextest run --all-targets --all-features

doctest:
	scripts/test-doctests.sh

doc:
	RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --all-features

okf:
	python3 scripts/okf-lint.py --repo-root . docs

links:
	lychee --offline --no-progress docs README.md AGENTS.md .agents/skills/open-knowledge-format/SKILL.md

docs-check: okf links

boundaries:
	scripts/check-boundaries.sh

public-api:
	scripts/check-public-api.sh

audit:
	cargo audit

deny:
	cargo deny check

features:
	cargo hack check --feature-powerset --no-dev-deps

udeps:
	cargo +nightly udeps --all-targets --all-features

mutants:
	cargo mutants --workspace --all-features

mutants-file:
	@test -n "$${FILE}" || { echo 'FILE is required' >&2; exit 2; }
	cargo mutants --file "$${FILE}" --all-features

perf:
	@args=(); \
	if [[ -n "$${PERF_BIN}" ]]; then args+=(--bin "$${PERF_BIN}"); fi; \
	args+=(-- "$${PERF_PATH}"); \
	scripts/perf-record.sh "$${args[@]}"

web-install:
	@test -f web/package-lock.json || { echo 'web stack is not configured' >&2; exit 2; }
	scripts/check-node-toolchain.sh
	npm --prefix web ci --ignore-scripts --no-audit --no-fund

web-dev:
	@test -f web/package.json || { echo 'web stack is not configured' >&2; exit 2; }
	scripts/check-node-toolchain.sh
	npm --prefix web run dev

web-toolchain:
	@test -f web/package.json || { echo 'web stack is not configured' >&2; exit 2; }
	scripts/check-node-toolchain.sh

web-check:
	@test -f web/package.json || { echo 'web stack is not configured' >&2; exit 2; }
	scripts/check-node-toolchain.sh
	npm --prefix web run check

web-build:
	@test -f web/package.json || { echo 'web stack is not configured' >&2; exit 2; }
	scripts/check-node-toolchain.sh
	npm --prefix web run build

web-lock:
	@test -f web/package-lock.json || { echo 'web stack is not configured' >&2; exit 2; }
	scripts/check-node-toolchain.sh
	npm --prefix web ci --dry-run --ignore-scripts

install-commit-hooks:
	scripts/install-commit-hooks.sh

install:
	cargo install --path crates/fathomable --locked

build-deps:
	scripts/setup-build-deps.sh

clean:
	cargo clean
