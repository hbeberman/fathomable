set shell := ["bash", "-cu"]

default:
    make help

gates:
    make gates

gates-verbose:
    make gates-verbose

fmt:
    make fmt

fmt-check:
    make fmt-check

clippy:
    make clippy

test:
    make test

doctest:
    make doctest

doc:
    make doc


okf:
    make okf

links:
    make links

docs-check:
    make docs-check
boundaries:
    make boundaries

public-api:
    make public-api

audit:
    make audit

deny:
    make deny

udeps:
    make udeps

mutants:
    make mutants

mutants-file:
    make mutants-file

perf path=".":
    make perf PERF_PATH="{{path}}"

demo *ARGS:
    scripts/demo-repo.sh {{ARGS}}

install-commit-hooks:
    make install-commit-hooks

test-commit-hooks:
    make test-commit-hooks

install:
    make install

build-deps:
    make build-deps

clean:
    make clean
