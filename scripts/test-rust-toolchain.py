#!/usr/bin/env python3
"""Test toolchain roles and setup without installing tools or changing rustup state."""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SOURCE_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = SOURCE_ROOT / "scripts/rust-toolchain.py"
SPEC = importlib.util.spec_from_file_location("rust_toolchain", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
toolchains = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(toolchains)

TOOL = """#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

with open(os.environ["TOOLCHAIN_TEST_LOG"], "a") as output:
    output.write(json.dumps([Path(sys.argv[0]).name, *sys.argv[1:]]) + "\\n")
raise SystemExit(int(os.environ.get("TOOLCHAIN_TEST_EXIT", "0")))
"""


class ToolchainTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="fathomable toolchains ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "checkout"
        (self.repo / "scripts").mkdir(parents=True)
        (self.repo / "licenses").mkdir()
        self.cargo_manifest = self.repo / "Cargo.toml"
        self.cargo_manifest.write_text('[workspace.package]\nrust-version = "1.97"\n')
        self.release_manifest = self.repo / "licenses/manifest.json"
        self.release_manifest.write_text(json.dumps({
            "rust_standard_library": {"release": "1.99.1"},
        }))
        (self.repo / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "stable"\n')
        for name in ("rust-toolchain.py", "setup-build-deps.sh"):
            shutil.copy2(SOURCE_ROOT / "scripts" / name, self.repo / "scripts" / name)
        tools = self.root / "tools"
        tools.mkdir()
        for name in ("rustup", "cargo", "cc", "git", "rg", "pkg-config"):
            path = tools / name
            path.write_text(TOOL)
            path.chmod(0o700)
        python = tools / "python3"
        python.write_text(
            f"#!{sys.executable}\nimport os, sys\n"
            "if sys.argv[1:] == ['-c', 'import yaml']:\n    raise SystemExit(0)\n"
            f"os.execv({sys.executable!r}, [{sys.executable!r}, *sys.argv[1:]])\n"
        )
        python.chmod(0o700)
        self.log = self.root / "calls.jsonl"
        self.env = os.environ.copy()
        self.env.pop("BASH_ENV", None)
        self.env.update({
            "PATH": f"{tools}:{self.env['PATH']}",
            "RUSTUP_TOOLCHAIN": "1.90.0",
            "TOOLCHAIN_TEST_LOG": str(self.log),
        })

    def run_script(self, name, *args):
        return subprocess.run(
            ["bash" if name.endswith(".sh") else sys.executable,
             str(self.repo / "scripts" / name), *args],
            cwd=self.root, env=self.env, capture_output=True, text=True, check=False,
        )

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_floor_and_release_are_independent_of_development_channel(self):
        for role, expected in (("msrv", "1.97.0"), ("release", "1.99.1")):
            with self.subTest(role=role):
                result = self.run_script("rust-toolchain.py", role)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), expected)
        self.cargo_manifest.write_text('[workspace.package]\nrust-version = "1.98.2"\n')
        self.assertEqual(toolchains.channel(self.repo, "msrv"), "1.98.2")
        self.assertEqual(toolchains.channel(self.repo, "release"), "1.99.1")
        self.assertFalse(self.log.exists())

    def test_non_numeric_and_missing_versions_fail_without_running_commands(self):
        for version in ("stable", "nightly", "1.99.1 --other", "", None):
            with self.subTest(version=version):
                self.release_manifest.write_text(json.dumps({
                    "rust_standard_library": {"release": version},
                }))
                result = self.run_script("rust-toolchain.py", "release", "cargo", "build")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("numeric Rust release", result.stderr)
                self.assertFalse(self.log.exists())
        self.release_manifest.unlink()
        result = self.run_script("rust-toolchain.py", "release")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("selection failed", result.stderr)

    def test_command_arguments_and_exit_status_are_preserved(self):
        self.env["TOOLCHAIN_TEST_EXIT"] = "23"
        result = self.run_script(
            "rust-toolchain.py", "release", "cargo", "test", "--", "path with spaces", "",
        )
        self.assertEqual(result.returncode, 23, result.stderr)
        self.assertEqual(self.calls(), [[
            "rustup", "run", "1.99.1", "cargo", "test", "--", "path with spaces", "",
        ]])

    def test_setup_uses_checkout_roles_from_another_directory(self):
        result = self.run_script("setup-build-deps.sh")
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        self.assertEqual([call[:4] for call in calls if call[0] == "rustup"], [
            ["rustup", "toolchain", "install", "stable"],
            ["rustup", "toolchain", "install", "1.99.1"],
            ["rustup", "toolchain", "install", "nightly"],
        ])
        installs = [call for call in calls if call[0] == "cargo"]
        self.assertTrue(installs)
        for call in installs:
            self.assertEqual(call[1], "+nightly" if "cargo-udeps" in call else "+stable")
        self.assertNotIn("1.97", self.log.read_text())
        self.assertFalse((self.repo / ".git/hooks").exists())


if __name__ == "__main__":
    unittest.main()
