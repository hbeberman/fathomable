#!/usr/bin/env python3
"""Verify repeated packaging with real Cargo and a dependency-free workspace."""

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import unittest


SOURCE_ROOT = Path(__file__).resolve().parent.parent


class PackageTests(unittest.TestCase):
    def test_repackaging_uses_current_core_at_the_same_version(self):
        with tempfile.TemporaryDirectory(prefix="fathomable package ") as directory:
            root = Path(directory)
            scripts = root / "scripts"
            scripts.mkdir()
            shutil.copy2(SOURCE_ROOT / "justfile", root / "justfile")
            shutil.copy2(
                SOURCE_ROOT / "scripts/rust-toolchain.py",
                scripts / "rust-toolchain.py",
            )
            shutil.copy2(SOURCE_ROOT / "scripts/package.py", scripts / "package.py")
            (root / "licenses").mkdir()
            shutil.copy2(
                SOURCE_ROOT / "licenses/manifest.json",
                root / "licenses/manifest.json",
            )
            license_check = scripts / "check-licenses.sh"
            license_check.write_text("#!/bin/sh\nexit 0\n")
            license_check.chmod(0o700)
            (scripts / "betterleaks.py").write_text(
                "import os, sys\nassert sys.argv[1:] == ['artifacts', '--packages']\n"
                "raise SystemExit(int(os.environ.get('PACKAGE_TEST_SCAN_EXIT', '0')))\n"
            )
            (root / "Cargo.toml").write_text(
                '[workspace]\nmembers = ["core", "app", "testing"]\nresolver = "3"\n'
            )
            for folder, name, extra in (
                ("core", "fathomable-core", ""),
                ("app", "fathomable",
                 '[dependencies]\nfathomable-core = { path = "../core", version = "0.1.0" }\n'),
                ("testing", "fathomable-testing", "publish = false\n"),
            ):
                crate = root / folder
                (crate / "src").mkdir(parents=True)
                (crate / "Cargo.toml").write_text(
                    f'[package]\nname = "{name}"\nversion = "0.1.0"\n'
                    'edition = "2024"\nlicense = "MIT"\n' + extra
                )
            (root / "testing/src/lib.rs").write_text("")
            environment = {
                key: value for key, value in os.environ.items()
                if not key.startswith("GIT_") and key not in {
                    "BASH_ENV", "ENV", "CDPATH", "CARGO_BUILD_BUILD_DIR",
                    "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                }
            }
            target = root / "target with spaces"
            build = root / "normal build cache"
            build.mkdir()
            marker = build / "keep"
            marker.write_text("existing cache\n")
            environment.update({
                "CARGO_TARGET_DIR": str(target),
                "CARGO_BUILD_BUILD_DIR": str(build),
                "CARGO_NET_OFFLINE": "true",
            })
            for api in ("old_api", "new_api"):
                with self.subTest(api=api):
                    (root / "core/src/lib.rs").write_text(f"pub fn {api}() {{}}\n")
                    (root / "app/src/main.rs").write_text(
                        f"fn main() {{ fathomable_core::{api}(); }}\n"
                    )
                    result = subprocess.run(
                        ["just", "package"], cwd=root, env=environment,
                        capture_output=True, text=True, check=False,
                    )
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    archive = target / "package/fathomable-core-0.1.0.crate"
                    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
                    with tarfile.open(target / "package/fathomable-0.1.0.crate") as bundle:
                        lock = bundle.extractfile("fathomable-0.1.0/Cargo.lock")
                        self.assertIsNotNone(lock)
                        packages = tomllib.loads(lock.read().decode())["package"]
                    core = next(p for p in packages if p["name"] == "fathomable-core")
                    self.assertEqual(core["checksum"], checksum)
                    self.assertEqual(list(build.iterdir()), [marker])
                    self.assertEqual(list(target.glob("package-verify-*")), [])

            previous = {
                path.name: path.read_bytes() for path in (target / "package").glob("*.crate")
            }
            for source, scan_exit in (
                ("pub fn new_api() {} pub fn unscanned() {}\n", "23"),
                ("invalid Rust\n", "0"),
            ):
                with self.subTest(source=source):
                    (root / "core/src/lib.rs").write_text(source)
                    environment["PACKAGE_TEST_SCAN_EXIT"] = scan_exit
                    result = subprocess.run(
                        ["just", "package"], cwd=root, env=environment,
                        capture_output=True, text=True, check=False,
                    )
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("Package preflight failed:", result.stderr)
                    self.assertEqual(
                        {path.name: path.read_bytes()
                         for path in (target / "package").glob("*.crate")},
                        previous,
                    )
                    self.assertEqual(list(target.glob("package-verify-*")), [])


if __name__ == "__main__":
    unittest.main()
