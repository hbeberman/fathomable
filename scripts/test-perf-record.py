#!/usr/bin/env python3
"""Test the profiling helper with local fake Cargo and perf commands."""

import errno
import json
import os
from pathlib import Path
import pty
import stat
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRATCH_ROOT = REPO_ROOT / ".tmp"

CARGO = r"""#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

event = {
    "args": sys.argv[1:],
    "target_dir": os.environ.get("CARGO_TARGET_DIR"),
    "rustflags": os.environ.get("RUSTFLAGS"),
    "encoded_rustflags": os.environ.get("CARGO_ENCODED_RUSTFLAGS"),
}
with open(os.environ["MOCK_CARGO_LOG"], "a", encoding="utf-8") as output:
    output.write(json.dumps(event) + "\n")

if sys.argv[1] == "metadata":
    print(json.dumps({
        "target_directory": os.environ["MOCK_TARGET_ROOT"],
        "workspace_members": ["fixture 0.1.0"],
        "packages": [{
            "id": "fixture 0.1.0",
            "name": "fixture",
            "targets": [{"kind": ["bin"], "name": "fathomable"}],
        }],
    }))
elif sys.argv[1] == "build":
    executable = Path(os.environ["CARGO_TARGET_DIR"]) / "release" / "fathomable"
    executable.parent.mkdir(parents=True, exist_ok=True)
    executable.write_text(
        "#!/usr/bin/env python3\n"
        "import json, os, sys\n"
        "with open(os.environ['MOCK_EXEC_LOG'], 'w', encoding='utf-8') as output:\n"
        "    json.dump(sys.argv[1:], output)\n",
        encoding="utf-8",
    )
    executable.chmod(0o700)
    print(json.dumps({
        "reason": "compiler-artifact",
        "target": {"name": "fathomable"},
        "executable": str(executable),
    }))
else:
    raise SystemExit(f"unexpected cargo command: {sys.argv[1:]}")
"""

PERF = r"""#!/usr/bin/env python3
import json
import os
from pathlib import Path
import subprocess
import sys

with open(os.environ["MOCK_PERF_LOG"], "a", encoding="utf-8") as output:
    output.write(json.dumps(sys.argv[1:]) + "\n")

command = sys.argv[1]
if command == "record":
    Path(sys.argv[sys.argv.index("-o") + 1]).write_bytes(b"perf-data")
    separator = sys.argv.index("--")
    raise SystemExit(subprocess.run(sys.argv[separator + 1:], check=False).returncode)
if command == "report":
    print("mock perf report")
elif command == "script":
    print("mock 1 1.0: cycles:\n\t1 fixture (/fixture)")
else:
    raise SystemExit(f"unexpected perf command: {sys.argv[1:]}")
"""

SCRIPT = r"""#!/usr/bin/env python3
from pathlib import Path
import subprocess
import sys

Path(sys.argv[sys.argv.index("-O") + 1]).write_text("", encoding="utf-8")
separator = sys.argv.index("--")
raise SystemExit(subprocess.run(sys.argv[separator + 1:], check=False).returncode)
"""


def write_executable(path: Path, contents: str) -> None:
    path.write_text(contents, encoding="utf-8")
    path.chmod(0o700)


def run_in_pty(arguments: list[str], environment: dict[str, str]) -> tuple[int, str]:
    pid, descriptor = pty.fork()
    if pid == 0:
        os.chdir(REPO_ROOT)
        os.execvpe(arguments[0], arguments, environment)

    output = bytearray()
    while True:
        try:
            chunk = os.read(descriptor, 4096)
        except OSError as error:
            if error.errno == errno.EIO:
                break
            raise
        if not chunk:
            break
        output.extend(chunk)
    os.close(descriptor)
    _, status = os.waitpid(pid, 0)
    return os.waitstatus_to_exitcode(status), output.decode(errors="replace")


class PerfRecordTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        os.umask(0o077)
        SCRATCH_ROOT.mkdir(mode=0o700, exist_ok=True)

    def run_helper(
        self,
        arguments: list[str],
        expected_args: list[str],
        *,
        rustflags: str | None = None,
        encoded_rustflags: str | None = None,
    ) -> list[dict[str, object]]:
        with tempfile.TemporaryDirectory(dir=SCRATCH_ROOT) as directory:
            root = Path(directory)
            tools = root / "tools"
            tools.mkdir()
            write_executable(tools / "cargo", CARGO)
            write_executable(tools / "perf", PERF)
            write_executable(tools / "script", SCRIPT)

            target = root / "target with spaces"
            cargo_log = root / "cargo.jsonl"
            perf_log = root / "perf.jsonl"
            exec_log = root / "exec.json"
            environment = os.environ.copy()
            environment.update({
                "PATH": f"{tools}:{environment['PATH']}",
                "CARGO_TARGET_DIR": str(target),
                "MOCK_TARGET_ROOT": str(target),
                "MOCK_CARGO_LOG": str(cargo_log),
                "MOCK_PERF_LOG": str(perf_log),
                "MOCK_EXEC_LOG": str(exec_log),
                "TMPDIR": str(root),
            })
            environment.pop("RUSTFLAGS", None)
            environment.pop("CARGO_ENCODED_RUSTFLAGS", None)
            if rustflags is not None:
                environment["RUSTFLAGS"] = rustflags
            if encoded_rustflags is not None:
                environment["CARGO_ENCODED_RUSTFLAGS"] = encoded_rustflags

            status, output = run_in_pty(arguments, environment)
            self.assertEqual(status, 0, output)

            out_dirs = list((target / "perf").iterdir())
            self.assertEqual(len(out_dirs), 1)
            out_dir = out_dirs[0]
            self.assertEqual(stat.S_IMODE(out_dir.stat().st_mode), 0o700)
            self.assertEqual(json.loads(exec_log.read_text()), expected_args)

            preserved = out_dir / "fathomable"
            built = target / "perf-build" / "release" / "fathomable"
            self.assertTrue(preserved.is_file())
            self.assertEqual(stat.S_IMODE(preserved.stat().st_mode), 0o700)
            self.assertEqual(preserved.read_bytes(), built.read_bytes())
            built.write_text("replaced after capture", encoding="utf-8")
            self.assertNotEqual(preserved.read_bytes(), built.read_bytes())

            cargo_events = [
                json.loads(line) for line in cargo_log.read_text().splitlines()
            ]
            perf_events = [
                json.loads(line) for line in perf_log.read_text().splitlines()
            ]
            record = next(event for event in perf_events if event[0] == "record")
            self.assertEqual(record[record.index("--call-graph") + 1], "fp")
            profiled_path = Path(record[record.index("--") + 1])
            self.assertEqual(profiled_path, preserved)

            return cargo_events

    def test_just_recipe_forwards_spaced_path_and_preserves_rustflags(self) -> None:
        arguments = ["just", "perf", "path with spaces.md", "fathomable"]
        cargo_events = self.run_helper(
            arguments,
            ["path with spaces.md"],
            rustflags="-Copt-level=1",
        )
        build = next(event for event in cargo_events if event["args"][0] == "build")
        self.assertEqual(
            build["args"],
            [
                "build",
                "--release",
                "--bin",
                "fathomable",
                "--message-format=json-render-diagnostics",
            ],
        )
        self.assertEqual(
            build["rustflags"],
            "-Copt-level=1 -Cforce-frame-pointers=yes",
        )
        self.assertTrue(str(build["target_dir"]).endswith("/perf-build"))

    def test_encoded_rustflags_take_precedence_without_discarding_environment(
        self,
    ) -> None:
        encoded = "-Copt-level=1\x1f-Clink-arg=path with spaces"
        arguments = [
            "scripts/perf-record.sh",
            "--bin",
            "fathomable",
            "--",
            "path with spaces.md",
            "--flag-like",
            "",
        ]
        cargo_events = self.run_helper(
            arguments,
            ["path with spaces.md", "--flag-like", ""],
            rustflags="-Cdebuginfo=1",
            encoded_rustflags=encoded,
        )
        build = next(event for event in cargo_events if event["args"][0] == "build")
        self.assertEqual(
            build["encoded_rustflags"],
            f"{encoded}\x1f-Cforce-frame-pointers=yes",
        )
        self.assertEqual(build["rustflags"], "-Cdebuginfo=1")

    def test_explicitly_empty_encoded_rustflags_still_take_precedence(self) -> None:
        arguments = [
            "scripts/perf-record.sh",
            "--bin",
            "fathomable",
            "--",
            ".",
        ]
        cargo_events = self.run_helper(
            arguments,
            ["."],
            rustflags="-Cdebuginfo=1",
            encoded_rustflags="",
        )
        build = next(event for event in cargo_events if event["args"][0] == "build")
        self.assertEqual(
            build["encoded_rustflags"],
            "-Cforce-frame-pointers=yes",
        )
        self.assertEqual(build["rustflags"], "-Cdebuginfo=1")

if __name__ == "__main__":
    unittest.main()
