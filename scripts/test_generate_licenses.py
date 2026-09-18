#!/usr/bin/env python3
"""Test the cargo-about adapter and supplemental notice safeguards."""

import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).with_name("generate_licenses.py")
SPEC = importlib.util.spec_from_file_location("generate_licenses", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
licenses = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = licenses
SPEC.loader.exec_module(licenses)


def inventory(root, *, source_path="LICENSE"):
    package = {
        "name": "example",
        "version": "1.2.3",
        "license": "MIT",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": str(root / "Cargo.toml"),
        "authors": ["Do not turn package authors into copyright claims"],
    }
    return {
        "crates": [{"package": package}],
        "licenses": [{
            "id": "MIT",
            "name": "MIT License",
            "source_path": source_path,
            "text": "Copyright © Holder & Partners\nPermission granted.\n",
            "used_by": [{"crate": package}],
        }],
    }


class LicenseTests(unittest.TestCase):
    def test_cargo_about_is_pinned_locked_offline_and_fail_closed(self):
        calls = [
            subprocess.CompletedProcess([], 0, "cargo-about 0.9.2\n", ""),
            subprocess.CompletedProcess([], 0, '{"crates": [], "licenses": []}', ""),
        ]
        with mock.patch.object(licenses.subprocess, "run", side_effect=calls) as run:
            licenses.cargo_about(Path("."))
        command = run.call_args.args[0]
        for flag in ("--frozen", "--fail", "--format"):
            self.assertIn(flag, command)
        self.assertEqual(command[command.index("--manifest-path") + 1],
                         "crates/fathomable/Cargo.toml")
        self.assertEqual(command[command.index("--format") + 1], "json")

    def test_wrong_cargo_about_version_is_rejected(self):
        with mock.patch.object(licenses, "run", return_value="cargo-about 0.1.0"):
            with self.assertRaisesRegex(licenses.LicenseBundleError, "expected cargo-about"):
                licenses.cargo_about(Path("."))

    def test_cargo_about_failures_reach_the_caller(self):
        failure = subprocess.CompletedProcess([], 1, "", "unaccepted license")
        with mock.patch.object(licenses.subprocess, "run", return_value=failure):
            with self.assertRaisesRegex(licenses.LicenseBundleError, "unaccepted license"):
                licenses.cargo_about(Path("."))

    def test_crate_copyrights_and_separate_notices_survive_without_html_escaping(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "NOTICE").write_text("Additional NOTICE attribution\n")
            text, packages = licenses.crate_notices(
                root, inventory(root), {"package_notices": {}},
            )
        self.assertEqual(set(packages), {"example@1.2.3"})
        self.assertIn("Copyright © Holder & Partners", text)
        self.assertIn("Additional NOTICE attribution", text)
        self.assertIn("https://crates.io/api/v1/crates/example/1.2.3/download", text)
        self.assertNotIn("package authors", text)
        self.assertNotIn(directory, text)

    def test_unreviewed_synthesized_license_is_rejected(self):
        with self.assertRaisesRegex(licenses.LicenseBundleError, "synthesized MIT"):
            licenses.crate_notices(
                Path("."), inventory(Path("."), source_path=None), {"package_notices": {}},
            )

    def test_pinned_notice_replaces_synthesized_text(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "licenses").mkdir()
            text = "Copyright 2026 Actual Holder\nFull upstream terms.\n"
            (root / "licenses/notice.txt").write_text(text)
            manifest = {"package_notices": {"example@1.2.3": [{
                "license": "MIT",
                "file": "notice.txt",
                "source": "https://example.test/upstream",
                "sha256": hashlib.sha256(text.encode()).hexdigest(),
            }]}}
            rendered, _ = licenses.crate_notices(
                root, inventory(root, source_path=None), manifest,
            )
        self.assertIn(text, rendered)
        self.assertNotIn("Holder & Partners", rendered)

    def test_missing_package_license_coverage_is_rejected(self):
        value = inventory(Path("."))
        value["licenses"] = []
        with self.assertRaisesRegex(licenses.LicenseBundleError, "coverage"):
            licenses.crate_notices(Path("."), value, {"package_notices": {}})

    def test_stale_supplemental_owner_is_rejected(self):
        with self.assertRaisesRegex(licenses.LicenseBundleError, "stale package notices"):
            licenses.crate_notices(
                Path("."), inventory(Path(".")), {"package_notices": {"removed@1.0": []}},
            )

    def test_notice_hash_mismatch_is_fatal(self):
        with self.assertRaisesRegex(licenses.LicenseBundleError, "hash mismatch"):
            licenses.verify_digest(b"modified terms", "0" * 64, "upstream notice")

    def test_check_rejects_content_and_whitespace_changes_without_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "licenses.txt"
            path.write_bytes(b"old notice\n")
            with self.assertRaisesRegex(licenses.LicenseBundleError, "stale"):
                licenses.check_output("new notice\n", path)
            self.assertEqual(path.read_bytes(), b"old notice\n")
            with self.assertRaisesRegex(licenses.LicenseBundleError, "stale"):
                licenses.check_output("old notice \n", path)
            licenses.check_output("old notice\n", path)

    def test_lockfile_changes_make_the_complete_bundle_stale(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "licenses").mkdir()
            (root / "licenses/manifest.json").write_text(json.dumps({
                "version": 1, "target": "x86_64-unknown-linux-gnu", "package_notices": {},
            }))
            (root / "about.toml").write_text('targets = ["x86_64-unknown-linux-gnu"]\n')
            (root / "Cargo.toml").write_text(
                '[workspace.package]\nlicense = "MIT"\nversion = "0.1.0"\n',
            )
            (root / "LICENSE").write_text("First-party copyright and license\n")
            lock = root / "Cargo.lock"
            lock.write_text("version = 4\n")
            output = root / "bundle.txt"
            with (
                mock.patch.object(licenses, "cargo_about", return_value=inventory(root)),
                mock.patch.object(licenses, "runtime_notices", return_value="Rust notices"),
                mock.patch.object(licenses, "asset_notices", return_value="Asset notices"),
            ):
                original = licenses.render_bundle(root)
                output.write_text(original)
                lock.write_text("version = 4\n# dependency change\n")
                with self.assertRaisesRegex(licenses.LicenseBundleError, "stale"):
                    licenses.check_output(licenses.render_bundle(root), output)
            self.assertEqual(output.read_text(), original)

    def test_toolchain_mismatch_is_fatal(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.97.0"\n')
            manifest = {"rust_standard_library": {
                "release": "1.97.0", "rustc_commit": "1" * 40,
            }}
            with mock.patch.object(licenses, "run", return_value="release: 1.96.0\ncommit-hash: wrong"):
                with self.assertRaisesRegex(licenses.LicenseBundleError, "toolchain"):
                    licenses.runtime_notices(root, manifest)

    def test_embedded_asset_changes_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".cargo_vcs_info.json").write_text(json.dumps({"git": {"sha1": "revision"}}))
            (root / "theme.dump").write_bytes(b"changed theme")
            packages = {"example@1.2.3": {"manifest_path": str(root / "Cargo.toml")}}
            manifest = {"embedded_assets": [{
                "package": "example@1.2.3", "vcs_revision": "revision",
                "artifacts": [{"path": "theme.dump", "sha256": "0" * 64}],
                "notices": [{"file": "unused"}],
            }]}
            with self.assertRaisesRegex(licenses.LicenseBundleError, "hash mismatch"):
                licenses.asset_notices(root, packages, manifest)

    def test_html_renderer_preserves_legal_text_but_omits_page_chrome(self):
        parser = licenses.PlainTextHTML()
        parser.feed(
            "<html><head><title>duplicate</title></head><body>"
            "<h1>Copyright notices</h1><p>Copyright &copy; Holder</p>"
            "<pre>Permission  granted\n  Terms</pre></body></html>"
        )
        text = parser.text()
        self.assertNotIn("duplicate", text)
        self.assertIn("Copyright © Holder", text)
        self.assertIn("Permission  granted\n  Terms", text)

    def test_committed_bundle_has_no_local_paths_or_generation_time(self):
        text = licenses.OUTPUT.read_text(encoding="utf-8")
        self.assertNotIn(str(SCRIPT.parent.parent.resolve()), text)
        self.assertNotIn(str(Path.home()), text)
        self.assertNotIn("Generated at:", text)
        self.assertNotIn("Generated on:", text)


if __name__ == "__main__":
    unittest.main()
