#!/usr/bin/env python3
# @okf-doc: /decisions/0001-dependency-policy.md
"""Print a repository toolchain version, or run a command with that toolchain."""

import argparse
import json
import os
from pathlib import Path
import re
import sys
import tomllib


ROOT = Path(__file__).resolve().parent.parent


def channel(root: Path, role: str) -> str:
    if role == "msrv":
        manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
        version = manifest["workspace"]["package"]["rust-version"]
    else:
        manifest = json.loads((root / "licenses/manifest.json").read_text(encoding="utf-8"))
        version = manifest["rust_standard_library"]["release"]
    if not isinstance(version, str) or not re.fullmatch(r"[0-9]+\.[0-9]+(?:\.[0-9]+)?", version):
        raise ValueError(f"{role} must be a numeric Rust release, got {version!r}")
    return version if version.count(".") == 2 else version + ".0"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("role", choices=("msrv", "release"))
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    try:
        version = channel(ROOT, args.role)
        if args.command:
            os.execvp("rustup", ["rustup", "run", version, *args.command])
        print(version)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Rust toolchain selection failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
