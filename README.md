# fathomable

Rust Workspace Viewer

## Local workflow

```sh
scripts/setup-build-deps.sh   # installs optional cargo tools after approval
just install-commit-hooks     # refreshes the local commit hook

# Run project gates and heavyweight diagnostics.
make gates                    # canonical local gate
make mutants                  # heavyweight mutation testing
PERF_SECONDS=15 PERF_BIN=<binary-name> make perf
scripts/perf-record.sh 15 --bin <binary-name> -- <binary-args>
```

`Makefile` is the command source of truth; `justfile` is a thin compatibility
frontend that calls the matching Make target. `make gates` calls
`scripts/gates.sh`. `just install-commit-hooks` writes a local
`.git/hooks/commit-msg` wrapper without mutating `core.hooksPath`. Do not claim
the full gate passed after running only an individual diagnostic command.

## Documentation

Durable project knowledge lives in the Open Knowledge Format 0.2 bundle under
`docs/`. Start at `docs/index.md`; validate changes with `just docs-check`.
The canonical gate enforces both OKF structure and maintained local links.

## Generated checks

- formatting: `cargo fmt --check`
- linting: `cargo clippy --all-targets --all-features -- -D warnings -F unsafe-code`
- tests: `cargo nextest run --all-targets --all-features`
- doctests: `scripts/test-doctests.sh` (all library targets, default and all features)
- rustdoc: `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --all-features`
- OKF documentation: `python3 scripts/okf-lint.py --repo-root . docs`
- documentation links: `lychee --offline --no-progress docs README.md AGENTS.md .agents/skills/open-knowledge-format/SKILL.md`
- public API scan: `scripts/check-public-api.sh`
- dependency audit: `cargo audit`
- feature powerset: `cargo hack check --feature-powerset --no-dev-deps`
- unused dependencies: `cargo +nightly udeps --all-targets --all-features`
- mutation testing: `cargo mutants --workspace --all-features`
- perf tracing: `PERF_SECONDS=15 PERF_BIN=<binary-name> make perf` after adding a binary target; call `scripts/perf-record.sh` directly for binary arguments
