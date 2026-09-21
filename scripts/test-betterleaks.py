#!/usr/bin/env python3
"""Offline synthetic fixtures for the real pinned scanner and native prek."""

import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("secret_scan", ROOT / "scripts/betterleaks.py")
scanner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(scanner)


class SecretScanning(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not scanner.TOOL.is_file():
            raise RuntimeError("Run python3 scripts/betterleaks.py install first.")

    def setUp(self):
        (ROOT / ".tmp").mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix="secret-tests-", dir=ROOT / ".tmp")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.env = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("GIT_", "PREK_", "BETTERLEAKS_", "GITLEAKS_"))
        }
        self.env.update({
            "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_TEMPLATE_DIR": str(self.repo / "templates"),
            "PREK_HOME": str(self.repo / ".tmp/prek"),
            "TMPDIR": str(self.repo / ".tmp"),
        })
        (self.repo / "templates").mkdir()
        (self.repo / "scripts").mkdir()
        for name in ("betterleaks.py", "betterleaks.toml"):
            shutil.copy2(ROOT / "scripts" / name, self.repo / "scripts" / name)
        (self.repo / "scripts/configs").mkdir()
        shutil.copy2(ROOT / "scripts/configs/prek.toml", self.repo / "scripts/configs/prek.toml")
        tool = self.repo / scanner.TOOL.relative_to(ROOT)
        tool.parent.mkdir(parents=True)
        tool.symlink_to(scanner.TOOL)
        self.secret = "ghp_" + "A1b2C3d4E5f6G7h8J9k0LmNoPqRsTuVwXyZa"
        self.write(".gitignore", ".tmp/\nignored.txt\ntemplates/\n")
        self.write("payload.txt", "clean\n")
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Synthetic Scanner Test")
        self.git("config", "user.email", "scanner@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.commit()
        self.base = self.git("rev-parse", "HEAD").stdout.strip()

    def command(self, *command, expected=0, env=None):
        result = subprocess.run(
            command, cwd=self.repo, env=env or self.env,
            capture_output=True, text=True, timeout=45, check=False,
        )
        self.assertNotIn(self.secret, result.stdout + result.stderr)
        if expected == 0:
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        else:
            self.assertNotEqual(result.returncode, 0, result.stderr + result.stdout)
        return result

    def git(self, *args):
        return self.command("git", *args)

    def commit(self):
        self.git("add", ".")
        self.git("commit", "-qm", "test: synthetic scan fixture")

    def write(self, path, content):
        (self.repo / path).write_text(content)

    def scan(self, *args, **kwargs):
        return self.command(sys.executable, "scripts/betterleaks.py", *args, **kwargs)

    def test_tracked_excludes_ignored_untracked_and_symlink_targets(self):
        self.write("ignored.txt", self.secret)
        self.write("untracked.txt", self.secret)
        (self.repo / "link").symlink_to("ignored.txt")
        self.git("add", "link")
        self.scan("tracked")
        self.scan("staged")

    def test_staged_and_all_files_scopes_use_the_right_bytes(self):
        self.write("payload.txt", self.secret)
        self.git("add", "payload.txt")
        self.write("payload.txt", "clean unstaged replacement\n")
        self.scan("staged", expected=1)
        self.scan("tracked")
        self.command("prek", "run", "--config", "scripts/configs/prek.toml", "secrets", expected=1)
        self.assertEqual((self.repo / "payload.txt").read_text(), "clean unstaged replacement\n")
        self.command("prek", "run", "--config", "scripts/configs/prek.toml", "--all-files", "secrets")
        self.git("add", "payload.txt")
        self.write("payload.txt", self.secret)
        self.command("prek", "run", "--config", "scripts/configs/prek.toml", "secrets")
        self.assertEqual((self.repo / "payload.txt").read_text(), self.secret)
        self.command("prek", "run", "--config", "scripts/configs/prek.toml", "--all-files", "secrets", expected=1)

    def test_alternate_index_is_preserved_by_staged_and_prek_scans(self):
        index = self.repo / ".tmp/alternate-index"
        shutil.copyfile(self.repo / ".git/index", index)
        env = self.env | {"GIT_INDEX_FILE": str(index)}
        self.write("payload.txt", self.secret)
        self.command("git", "add", "payload.txt", env=env)
        self.scan("staged")  # Ordinary index is still clean.
        self.scan("staged", expected=1, env=env)
        self.command("prek", "run", "--config", "scripts/configs/prek.toml", "secrets", expected=1, env=env)
        self.assertEqual((self.repo / "payload.txt").read_text(), self.secret)

    def test_inline_environment_config_and_ignore_cannot_suppress_findings(self):
        self.write("payload.txt", self.secret + " # gitleaks:allow betterleaks:allow\n")
        self.git("add", "payload.txt")
        bypass = '[extend]\nuseDefault = true\n[[allowlists]]\nregexes = ["."]\n'
        self.write(".betterleaks.toml", bypass)
        self.write(".gitleaks.toml", bypass)
        self.write(".betterleaksignore", "payload.txt:github-pat:1\n")
        self.write(".gitleaksignore", "payload.txt:github-pat:1\n")
        self.git("add", ".betterleaksignore", ".gitleaksignore")
        env = self.env | {"BETTERLEAKS_CONFIG_TOML": bypass, "GITLEAKS_CONFIG": ".gitleaks.toml"}
        result = self.scan("tracked", expected=1, env=env)
        self.assertIn("github-pat", result.stderr)
        self.scan("staged", expected=1, env=env)

    def test_history_and_range_find_added_then_deleted_secret(self):
        self.write("payload.txt", self.secret)
        self.commit()
        self.write("payload.txt", "removed\n")
        self.commit()
        head = self.git("rev-parse", "HEAD").stdout.strip()
        self.scan("tracked")
        self.scan("history", expected=1)
        self.scan("range", "--base", self.base, "--head", head, expected=1)
        self.scan("range", "--base", head, "--head", head)
        self.scan("range", "--base", "not-a-ref", "--head", head, expected=1)
        self.scan("range", "--base", "1" * 40, "--head", head, expected=1)

    def test_merge_resolution_additions_are_scanned(self):
        self.git("switch", "-qc", "topic")
        self.write("topic.txt", "topic\n")
        self.commit()
        self.git("switch", "-q", "main")
        self.write("main.txt", "main\n")
        self.commit()
        self.git("merge", "--no-commit", "--no-ff", "topic")
        self.write("resolution.txt", self.secret)
        self.commit()
        head = self.git("rev-parse", "HEAD").stdout.strip()
        self.scan("range", "--base", self.base, "--head", head, expected=1)

    def test_exact_archives_include_ignored_files_and_nested_contents(self):
        path = self.repo / "ignored.txt"
        path.write_text(self.secret)
        bundle = self.repo / "distribution.crate"
        with tarfile.open(bundle, "w:gz") as archive:
            archive.add(path, arcname="package/hidden.txt")
        self.scan("artifacts", str(bundle), expected=1)
        self.scan("artifacts", str(path), expected=1)
        self.scan("artifacts", "missing.crate", expected=1)
        self.scan("artifacts", expected=1)
        self.scan("artifacts", ".", expected=1)
        with tarfile.open(self.repo / "nested.tar.gz", "w:gz") as archive:
            archive.add(bundle, arcname="distribution.tar.gz")
        self.scan("artifacts", "nested.tar.gz", expected=1)

    def test_exact_binary_artifact_strings_are_scanned(self):
        (self.repo / "program").write_bytes(b"\x7fELF" + bytes(64) + self.secret.encode() + b"\0")
        self.scan("artifacts", "program", expected=1)

    def test_corrupt_archive_fails_even_when_scanner_only_warns(self):
        (self.repo / "corrupt.tar.gz").write_bytes(b"not a gzip archive")
        self.scan("artifacts", "corrupt.tar.gz", expected=1)

    def test_missing_tool_and_shallow_history_fail_closed(self):
        tool = self.repo / scanner.TOOL.relative_to(ROOT)
        tool.unlink()
        self.scan("tracked", expected=1)
        tool.symlink_to(scanner.TOOL)
        self.write(".git/shallow", self.base + "\n")
        self.scan("history", expected=1)

    def test_ci_push_and_pr_scan_every_introduced_commit(self):
        self.write("payload.txt", self.secret)
        self.commit()
        self.write("payload.txt", "removed\n")
        self.commit()
        head = self.git("rev-parse", "HEAD").stdout.strip()
        for kind, event in (
            ("push", {"before": self.base, "after": head}),
            ("push", {"before": "0" * 40, "after": head}),
            ("push", {"before": "1" * 40, "after": head}),
            ("pull_request", {"pull_request": {"base": {"sha": self.base}, "head": {"sha": head}}}),
            ("schedule", {}),
            ("workflow_dispatch", {}),
        ):
            with self.subTest(kind=kind, event=event):
                self.write(".tmp/event.json", json.dumps(event))
                self.scan("ci", "--event", ".tmp/event.json", expected=1,
                          env=self.env | {"GITHUB_EVENT_NAME": kind})

    def test_ci_rule_update_scans_history_older_than_range(self):
        self.write("payload.txt", self.secret)
        self.commit()
        self.write("payload.txt", "removed\n")
        self.commit()
        base = self.git("rev-parse", "HEAD").stdout.strip()
        with (self.repo / "scripts/betterleaks.toml").open("a") as output:
            output.write("\n# reviewed rules update\n")
        self.commit()
        head = self.git("rev-parse", "HEAD").stdout.strip()
        self.scan("range", "--base", base, "--head", head)
        self.write(".tmp/event.json", json.dumps({"before": base, "after": head}))
        self.scan("ci", "--event", ".tmp/event.json", expected=1,
                  env=self.env | {"GITHUB_EVENT_NAME": "push"})

    def test_deleted_ref_and_invalid_ci_event_are_distinct(self):
        self.write(".tmp/event.json", json.dumps({"deleted": True, "after": "0" * 40}))
        self.scan("ci", "--event", ".tmp/event.json",
                  env=self.env | {"GITHUB_EVENT_NAME": "push"})
        self.write(".tmp/event.json", json.dumps({"after": "0" * 40}))
        self.scan("ci", "--event", ".tmp/event.json", expected=1,
                  env=self.env | {"GITHUB_EVENT_NAME": "push"})
        self.scan("ci", "--event", ".tmp/event.json", expected=1,
                  env=self.env | {"GITHUB_EVENT_NAME": "unknown"})

    def test_installer_rejects_checksum_mismatch_before_writing(self):
        with patch.object(scanner.urllib.request, "urlopen", return_value=io.BytesIO(b"invalid")):
            with self.assertRaisesRegex(scanner.ScanError, "checksum mismatch"):
                scanner.install()

    def test_errors_and_raw_diagnostics_are_never_printed(self):
        tool = self.repo / scanner.TOOL.relative_to(ROOT)
        tool.unlink()
        tool.write_text(
            "#!" + sys.executable + "\nimport sys\n"
            "if sys.argv[1:] == ['version']:\n    print('1.8.1')\n"
            f"else:\n    print({self.secret!r}, file=sys.stderr)\n    print('null')\n"
        )
        tool.chmod(0o700)
        self.scan("tracked", expected=1)


if __name__ == "__main__":
    unittest.main()
