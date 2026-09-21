#!/usr/bin/env python3
# @okf-doc: /commit-hooks.md
"""Verify and scan fresh source packages without reusing Cargo's temporary registry."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parent.parent
CARGO = [sys.executable, str(ROOT / "scripts/rust-toolchain.py"), "release", "cargo"]


def package() -> None:
    metadata = json.loads(subprocess.check_output(
        [*CARGO, "metadata", "--no-deps", "--locked", "--format-version=1"], cwd=ROOT,
    ))
    target = Path(metadata["target_directory"])
    archives = [
        f'{item["name"]}-{item["version"]}.crate'
        for item in metadata["packages"]
        if item["id"] in metadata["workspace_members"] and item["publish"] != []
    ]
    target.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="package-verify-", dir=target) as directory:
        environment = os.environ.copy()
        environment.update({
            "CARGO_TARGET_DIR": directory,
            "CARGO_BUILD_BUILD_DIR": directory,
        })
        subprocess.run(
            [*CARGO, "package", "--workspace", "--exclude", "fathomable-testing", "--locked"],
            cwd=ROOT, env=environment, check=True,
        )
        subprocess.run(
            [sys.executable, str(ROOT / "scripts/betterleaks.py"), "artifacts", "--packages"],
            cwd=ROOT, env=environment, check=True,
        )
        output = target / "package"
        output.mkdir(exist_ok=True)
        for name in archives:
            os.replace(Path(directory) / "package" / name, output / name)
            print(f"Verified package: {output / name}", flush=True)


def main() -> int:
    try:
        package()
    except subprocess.CalledProcessError as error:
        print(f"Package preflight failed: command exited with {error.returncode}", file=sys.stderr)
        return error.returncode
    except (OSError, ValueError, KeyError) as error:
        print(f"Package preflight failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
