#!/usr/bin/env python3
"""Verify source-package contents and repeated packaging with real Cargo."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import unittest


SOURCE_ROOT = Path(__file__).resolve().parent.parent


class PackageTests(unittest.TestCase):
    def test_published_packages_include_license_without_redundant_metadata(self):
        workspace = tomllib.loads((SOURCE_ROOT / "Cargo.toml").read_text())["workspace"]
        version = workspace["package"]["version"]
        license_text = (SOURCE_ROOT / "LICENSE").read_bytes()
        with tempfile.TemporaryDirectory(prefix="fathomable license package ") as directory:
            environment = {
                key: value for key, value in os.environ.items()
                if not key.startswith("GIT_") and key not in {
                    "CARGO_BUILD_BUILD_DIR", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                }
            }
            environment.update({
                "CARGO_TARGET_DIR": directory,
                "CARGO_BUILD_BUILD_DIR": directory,
                "CARGO_NET_OFFLINE": "true",
            })
            result = subprocess.run(
                [
                    sys.executable, str(SOURCE_ROOT / "scripts/rust-toolchain.py"),
                    "release", "cargo", "package", "--workspace",
                    "--exclude", "fathomable-testing", "--locked", "--allow-dirty",
                    "--no-verify",
                ],
                cwd=SOURCE_ROOT, env=environment,
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotIn("only one of `license` or `license-file`", result.stderr)
            for name in ("fathomable-core", "fathomable"):
                with self.subTest(package=name):
                    package = f"{name}-{version}"
                    archive = Path(directory) / "package" / f"{package}.crate"
                    with tarfile.open(archive) as bundle:
                        notice = bundle.getmember(f"{package}/LICENSE")
                        self.assertTrue(notice.isfile())
                        self.assertEqual(bundle.extractfile(notice).read(), license_text)
                        manifest = bundle.extractfile(f"{package}/Cargo.toml")
                        self.assertIsNotNone(manifest)
                        metadata = tomllib.loads(manifest.read().decode())["package"]
                    self.assertEqual(metadata["license"], "MIT")
                    self.assertNotIn("license-file", metadata)

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
            app_manifest = tomllib.loads(
                (SOURCE_ROOT / "crates/fathomable/Cargo.toml").read_text()
            )
            excludes = json.dumps(app_manifest["package"].get("exclude", []))
            for folder, name, extra in (
                ("core", "fathomable-core", ""),
                ("app", "fathomable",
                 f'exclude = {excludes}\n'
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
            (root / "app/examples").mkdir()
            (root / "app/examples/seed.rs").write_text("fn main() {}\n")
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
                        self.assertNotIn("fathomable-0.1.0/examples/seed.rs", bundle.getnames())
                        packaged_manifest = bundle.extractfile("fathomable-0.1.0/Cargo.toml")
                        self.assertIsNotNone(packaged_manifest)
                        packaged = tomllib.loads(packaged_manifest.read().decode())
                        self.assertNotIn(
                            "seed", [example["name"] for example in packaged.get("example", [])],
                        )
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
