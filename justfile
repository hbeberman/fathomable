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

features:
    make features

udeps:
    make udeps

mutants:
    make mutants

mutants-file:
    make mutants-file

perf:
    make perf

web-install:
    make web-install

web-dev:
    make web-dev

web-toolchain:
    make web-toolchain

web-check:
    make web-check

web-build:
    make web-build

web-lock:
    make web-lock

install-commit-hooks:
    make install-commit-hooks

build-deps:
    make build-deps

clean:
    make clean
