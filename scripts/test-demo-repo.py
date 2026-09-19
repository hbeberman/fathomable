#!/usr/bin/env python3
"""Exercise demo argument handling and mandatory isolation without an agent host."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SOURCE_ROOT = Path(__file__).resolve().parent.parent
CARGO = """#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

Path(os.environ["DEMO_TEST_LOG"]).write_text(json.dumps({
    "args": sys.argv[1:],
    "state": os.environ["XDG_STATE_HOME"],
    "config": os.environ["XDG_CONFIG_HOME"],
    "seed": json.loads(Path(sys.argv[-1]).read_text()),
}))
"""


class DemoTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="fathomable demo ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        scripts = self.root / "scripts"
        scripts.mkdir()
        self.script = scripts / "demo-repo.sh"
        shutil.copy2(SOURCE_ROOT / "scripts/demo-repo.sh", self.script)
        tools = self.root / "tools"
        tools.mkdir()
        cargo = tools / "cargo"
        cargo.write_text(CARGO)
        cargo.chmod(0o700)
        self.log = self.root / "cargo.json"
        self.env = {
            key: value for key, value in os.environ.items()
            if not key.startswith("GIT_") and key not in {"BASH_ENV", "ENV", "CDPATH"}
        }
        self.env.update({
            "PATH": f"{tools}:{self.env['PATH']}",
            "DEMO_TEST_LOG": str(self.log),
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_TEMPLATE_DIR": str(self.root / "empty-template"),
            "XDG_STATE_HOME": str(self.root / "real-state"),
            "XDG_CONFIG_HOME": str(self.root / "real-config"),
        })
        Path(self.env["GIT_TEMPLATE_DIR"]).mkdir()

    def run_demo(self, *args):
        return subprocess.run(
            ["bash", str(self.script), *args], cwd=self.root, env=self.env,
            capture_output=True, text=True, check=False,
        )

    def test_invalid_arguments_fail_before_creating_anything(self):
        for args in (("--isolated",), ("--unknown",), ("first", "second")):
            with self.subTest(args=args):
                before = sorted(self.root.rglob("*"))
                result = self.run_demo(*args)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("always isolated", result.stderr)
                self.assertEqual(sorted(self.root.rglob("*")), before)

    def test_default_and_explicit_spaced_paths_always_isolate_state(self):
        for args in ((), ("demo with spaces",)):
            with self.subTest(args=args):
                result = self.run_demo(*args)
                self.assertEqual(result.returncode, 0, result.stderr)
                event = json.loads(self.log.read_text())
                state = Path(event["state"])
                demo = state.parent.parent
                self.assertTrue(demo.is_relative_to(self.root))
                self.assertEqual(state, demo / ".xdg/state")
                self.assertEqual(Path(event["config"]), demo / ".xdg/config")
                self.assertTrue((demo / ".git").is_dir())
                self.assertEqual(len(event["seed"]["threads"]), 5)
                self.assertIn("--example", event["args"])
                self.assertIn("seed", event["args"])
                self.assertFalse((demo / ".xdg/seed.json").exists())
                self.assertFalse(Path(self.env["XDG_STATE_HOME"]).exists())
                self.assertFalse(Path(self.env["XDG_CONFIG_HOME"]).exists())

    def test_nonempty_directory_is_not_modified(self):
        demo = self.root / "occupied"
        demo.mkdir()
        existing = demo / "keep.txt"
        existing.write_text("keep\n")
        result = self.run_demo(str(demo))
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("non-empty", result.stderr)
        self.assertEqual(list(demo.iterdir()), [existing])
        self.assertEqual(existing.read_text(), "keep\n")
        self.assertFalse(self.log.exists())


if __name__ == "__main__":
    unittest.main()
