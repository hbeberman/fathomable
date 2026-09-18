#!/usr/bin/env bash
# @okf-doc: /decisions/0088-bundled-licenses.md
set -euo pipefail

cd "$(dirname "$0")/.."
python3 -m unittest scripts/test_generate_licenses.py
python3 scripts/generate_licenses.py --check
