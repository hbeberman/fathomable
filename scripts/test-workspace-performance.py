#!/usr/bin/env python3
# @okf-doc: /guide.md
"""Exercise bounded workspace startup, watching, input, memory, and shutdown."""

import argparse
from collections.abc import Callable
import errno
import fcntl
import json
import math
import os
from pathlib import Path
import pty
import selectors
import statistics
import struct
import subprocess
import sys
import tempfile
import threading
import time
import termios
import unicodedata


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRATCH_ROOT = REPO_ROOT / ".tmp"

BEGIN_UPDATE = b"\x1b[?2026h"
END_UPDATE = b"\x1b[?2026l"
KEYBOARD_QUERY = b"\x1b[?u"
# Crossterm follows the keyboard query with primary device attributes and
# consumes both responses before starting its input thread.
KEYBOARD_REPLY = b"\x1b[?0u\x1b[?1;2c"
FILE_PICKER = "files > "
WATCH_NOTICE = b"workspace watch coverage is partial"

DIRECTORIES = 2_048
INPUT_SAMPLES = 100
WATCH_CAP = 32
# Workspace discovery owns WATCH_CAP slots. Git metadata, thread state, and
# loaded-file ancestors are separate finite control surfaces.
CONTROL_WATCH_ALLOWANCE = 16
TOTAL_WATCH_LIMIT = WATCH_CAP + CONTROL_WATCH_ALLOWANCE
CAPS = {
    "discovery_entries": 4_096,
    "workspace_watches": WATCH_CAP,
    "retained_paths": 256,
    "comparison_paths": 128,
    "comparison_bytes": 1_048_576,
    "pending_events": 64,
}

FIRST_FRAME_OVERHEAD_LIMIT_MS = 250.0
INPUT_P99_LIMIT_MS = 50.0
QUIT_LIMIT_MS = 100.0
WARM_RSS_DELTA_LIMIT_KIB = 65_536
WARM_RSS_SPREAD_LIMIT_KIB = 8_192


class RegressionFailure(RuntimeError):
    """A synthetic regression or unavailable measurement."""

    def __init__(self, message: str, metrics: dict[str, object] | None = None) -> None:
        super().__init__(message)
        self.metrics = metrics


class TerminalScreen:
    """Minimal ANSI screen state sufficient to read Fathomable picker titles."""

    def __init__(self, width: int, height: int) -> None:
        self.width = width
        self.height = height
        self.cells = [[" "] * width for _ in range(height)]
        self.column = 0
        self.row = 0
        self.saved = (0, 0)

    @staticmethod
    def _parameters(raw: str) -> list[int]:
        raw = raw.lstrip("?><!")
        if not raw:
            return []
        return [int(part) if part.isdigit() else 0 for part in raw.split(";")]

    def _csi(self, parameters: str, command: str) -> None:
        values = self._parameters(parameters)
        amount = values[0] if values and values[0] else 1
        if command in ("H", "f"):
            self.row = max(0, (values[0] if values and values[0] else 1) - 1)
            self.column = max(
                0, (values[1] if len(values) > 1 and values[1] else 1) - 1
            )
        elif command == "A":
            self.row = max(0, self.row - amount)
        elif command == "B":
            self.row = min(self.height - 1, self.row + amount)
        elif command == "C":
            self.column = min(self.width - 1, self.column + amount)
        elif command == "D":
            self.column = max(0, self.column - amount)
        elif command == "E":
            self.row = min(self.height - 1, self.row + amount)
            self.column = 0
        elif command == "F":
            self.row = max(0, self.row - amount)
            self.column = 0
        elif command == "G":
            self.column = min(self.width - 1, amount - 1)
        elif command == "d":
            self.row = min(self.height - 1, amount - 1)
        elif command == "J" and (not values or values[0] in (2, 3)):
            self.cells = [[" "] * self.width for _ in range(self.height)]
        elif command == "K":
            mode = values[0] if values else 0
            if mode == 1:
                start, end = 0, self.column + 1
            elif mode == 2:
                start, end = 0, self.width
            else:
                start, end = self.column, self.width
            self.cells[self.row][start:end] = [" "] * (end - start)
        elif command == "s":
            self.saved = (self.column, self.row)
        elif command == "u":
            self.column, self.row = self.saved

    def _write(self, character: str) -> None:
        if character == "\r":
            self.column = 0
            return
        if character == "\n":
            self.row = min(self.height - 1, self.row + 1)
            return
        if character == "\b":
            self.column = max(0, self.column - 1)
            return
        if character < " " or character == "\x7f":
            return
        width = 0 if unicodedata.combining(character) else (
            2 if unicodedata.east_asian_width(character) in ("F", "W") else 1
        )
        if width == 0:
            return
        if self.column >= self.width:
            self.column = 0
            self.row = min(self.height - 1, self.row + 1)
        self.cells[self.row][self.column] = character
        if width == 2 and self.column + 1 < self.width:
            self.cells[self.row][self.column + 1] = " "
        self.column += width

    def apply(self, payload: bytes) -> None:
        text = payload.decode("utf-8", errors="replace")
        index = 0
        while index < len(text):
            if text[index] != "\x1b":
                self._write(text[index])
                index += 1
                continue
            if index + 1 >= len(text):
                return
            if text[index + 1] == "[":
                end = index + 2
                while end < len(text) and not "@" <= text[end] <= "~":
                    end += 1
                if end >= len(text):
                    return
                body = text[index + 2 : end]
                split = 0
                while split < len(body) and not " " <= body[split] <= "/":
                    split += 1
                self._csi(body[:split], text[end])
                index = end + 1
                continue
            if text[index + 1] == "]":
                bell = text.find("\x07", index + 2)
                terminator = text.find("\x1b\\", index + 2)
                endings = [value for value in (bell, terminator) if value >= 0]
                if not endings:
                    return
                end = min(endings)
                index = end + (2 if text.startswith("\x1b\\", end) else 1)
                continue
            index += 2

    def text(self) -> str:
        return "\n".join("".join(row) for row in self.cells)


def picker_input(screen: str) -> str | None:
    for line in screen.splitlines():
        marker = line.find("files > ")
        if marker < 0:
            continue
        value = line[marker + len("files > ") :]
        separator = value.find("   ")
        if separator >= 0:
            return value[:separator]
    return None


class PtyProcess:
    """One Fathomable process attached to an isolated pseudo-terminal."""

    def __init__(
        self,
        binary: Path,
        workspace: Path,
        config: Path,
        environment: dict[str, str],
    ) -> None:
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
        os.set_blocking(master, False)
        command = [
            str(binary),
            "--config",
            str(config),
            "--discovery-entries",
            str(CAPS["discovery_entries"]),
            "--workspace-watches",
            str(CAPS["workspace_watches"]),
            "--retained-paths",
            str(CAPS["retained_paths"]),
            "--comparison-paths",
            str(CAPS["comparison_paths"]),
            "--comparison-bytes",
            str(CAPS["comparison_bytes"]),
            "--pending-events",
            str(CAPS["pending_events"]),
            str(workspace),
        ]
        self.started_ns = time.perf_counter_ns()
        try:
            self.process = subprocess.Popen(
                command,
                cwd=workspace,
                env=environment,
                stdin=slave,
                stdout=slave,
                stderr=slave,
                close_fds=True,
                start_new_session=True,
            )
        finally:
            os.close(slave)
        self.master = master
        self.selector = selectors.DefaultSelector()
        self.selector.register(master, selectors.EVENT_READ)
        self.frames: list[tuple[int, bytes, str]] = []
        self.screen = TerminalScreen(100, 30)
        self.transcript = bytearray()
        self.parse_buffer = bytearray()
        self.in_frame = False
        self.keyboard_probe_replied = False
        self.closed = False

    def _consume(self, chunk: bytes) -> None:
        self.transcript.extend(chunk)
        if len(self.transcript) > 2_000_000:
            del self.transcript[:-1_000_000]
        if not self.keyboard_probe_replied and KEYBOARD_QUERY in self.transcript:
            os.write(self.master, KEYBOARD_REPLY)
            self.keyboard_probe_replied = True

        self.parse_buffer.extend(chunk)
        while True:
            if not self.in_frame:
                start = self.parse_buffer.find(BEGIN_UPDATE)
                if start < 0:
                    keep = len(BEGIN_UPDATE) - 1
                    if len(self.parse_buffer) > keep:
                        del self.parse_buffer[:-keep]
                    return
                del self.parse_buffer[: start + len(BEGIN_UPDATE)]
                self.in_frame = True
            end = self.parse_buffer.find(END_UPDATE)
            if end < 0:
                return
            payload = bytes(self.parse_buffer[:end])
            del self.parse_buffer[: end + len(END_UPDATE)]
            self.screen.apply(payload)
            self.frames.append((time.perf_counter_ns(), payload, self.screen.text()))
            self.in_frame = False

    def pump(self, timeout: float) -> None:
        if self.closed:
            return
        events = self.selector.select(timeout)
        for _key, _mask in events:
            while True:
                try:
                    chunk = os.read(self.master, 65_536)
                except BlockingIOError:
                    break
                except OSError as error:
                    if error.errno == errno.EIO:
                        return
                    raise
                if not chunk:
                    return
                self._consume(chunk)

    def wait_frame(
        self,
        after: int,
        timeout: float = 5.0,
        screen_matches: Callable[[str], bool] | None = None,
    ) -> tuple[int, int, bytes, str]:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for index in range(after, len(self.frames)):
                timestamp, payload, screen = self.frames[index]
                if screen_matches is None or screen_matches(screen):
                    return index + 1, timestamp, payload, screen
            if self.process.poll() is not None:
                raise RegressionFailure("viewer exited before the expected frame")
            self.pump(min(0.02, deadline - time.monotonic()))
        raise RegressionFailure("timed out waiting for a rendered frame")

    def first_frame_ms(self) -> float:
        _index, timestamp, _payload, _screen = self.wait_frame(0)
        return milliseconds(timestamp - self.started_ns)

    def send(self, data: bytes) -> None:
        try:
            os.write(self.master, data)
        except OSError as error:
            raise RegressionFailure("viewer stopped accepting terminal input") from error

    def send_and_measure(
        self,
        data: bytes,
        screen_matches: Callable[[str], bool],
    ) -> tuple[float, str]:
        before = len(self.frames)
        started = time.perf_counter_ns()
        self.send(data)
        _index, rendered, _payload, screen = self.wait_frame(
            before, timeout=1.0, screen_matches=screen_matches
        )
        return milliseconds(rendered - started), screen

    def wait_exit(self, timeout: float) -> float:
        started = time.perf_counter_ns()
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                self.pump(0)
                return milliseconds(time.perf_counter_ns() - started)
            self.pump(0.005)
        raise RegressionFailure("viewer did not exit promptly")

    def quit(self) -> float:
        self.send_and_measure(b"q", lambda screen: "Quit Fathomable?" in screen)
        self.send(b"\r")
        return self.wait_exit(2.0)

    def contains(self, needle: bytes) -> bool:
        return needle in self.transcript

    def close(self) -> None:
        if self.closed:
            return
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=0.2)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=1.0)
        self.selector.close()
        os.close(self.master)
        self.closed = True

    def __enter__(self) -> "PtyProcess":
        return self

    def __exit__(self, *_error: object) -> None:
        self.close()


def milliseconds(nanoseconds: int) -> float:
    return nanoseconds / 1_000_000


def percentile(values: list[float], proportion: float) -> float:
    if not values:
        raise RegressionFailure("no latency samples were collected")
    ordered = sorted(values)
    return ordered[max(0, math.ceil(proportion * len(ordered)) - 1)]


def rss_kib(pid: int) -> int:
    try:
        lines = Path(f"/proc/{pid}/status").read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise RegressionFailure("cannot read synthetic viewer memory from procfs") from error
    for line in lines:
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    raise RegressionFailure("procfs did not report synthetic viewer resident memory")


def inotify_watches(pid: int) -> int:
    total = 0
    try:
        entries = list(Path(f"/proc/{pid}/fdinfo").iterdir())
    except OSError as error:
        raise RegressionFailure("cannot read synthetic viewer watches from procfs") from error
    for entry in entries:
        try:
            contents = entry.read_text(encoding="utf-8")
        except (FileNotFoundError, PermissionError):
            continue
        total += sum(line.startswith("inotify wd:") for line in contents.splitlines())
    return total


def sample_rss(runner: PtyProcess, count: int = 10) -> list[int]:
    samples = []
    for _ in range(count):
        runner.pump(0.025)
        samples.append(rss_kib(runner.process.pid))
    return samples


def watch_plateau(runner: PtyProcess) -> tuple[int, list[int]]:
    samples: list[int] = []
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline:
        runner.pump(0.025)
        samples.append(inotify_watches(runner.process.pid))
        if len(samples) >= 5 and len(set(samples[-5:])) == 1:
            return samples[-1], samples
    raise RegressionFailure("inotify watch count did not reach a stable plateau")


def write_config(path: Path) -> None:
    path.write_text(
        "watch { toast 0; debounce 1 }\n"
        "layout {\n"
        "    menu-bar #false\n"
        "    sidebar { visible #false; files #false; threads #false }\n"
        "}\n"
        'diff { mode "off" }\n',
        encoding="utf-8",
    )


def make_workspace(root: Path, directories: int) -> Path:
    workspace = root / "workspace"
    workspace.mkdir(parents=True)
    (workspace / "README.md").write_text("# Synthetic workspace\n", encoding="utf-8")
    branches = 64 if directories >= 64 else max(1, directories)
    made = 0
    for branch in range(branches):
        parent = workspace / f"branch-{branch:03d}"
        branch_size = directories // branches + int(branch < directories % branches)
        for depth in range(branch_size):
            parent = parent / f"depth-{made // branches:03d}"
            parent.mkdir(parents=True)
            (parent / "source.rs").write_text("fn synthetic() {}\n", encoding="utf-8")
            (parent / "notes.md").write_text("synthetic\n", encoding="utf-8")
            made += 1

    template = root / "empty-git-template"
    template.mkdir()
    result = subprocess.run(
        ["git", "init", "--quiet", f"--template={template}", str(workspace)],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if result.returncode != 0:
        raise RegressionFailure("cannot initialise the synthetic repository")
    return workspace


def environment(root: Path, run_number: int) -> dict[str, str]:
    run_root = root / f"run-{run_number:02d}"
    home = run_root / "home"
    state = run_root / "state"
    config = run_root / "config"
    cache = run_root / "cache"
    for directory in (home, state, config, cache):
        directory.mkdir(parents=True, mode=0o700)
    result = os.environ.copy()
    result.update(
        {
            "HOME": str(home),
            "XDG_STATE_HOME": str(state),
            "XDG_CONFIG_HOME": str(config),
            "XDG_CACHE_HOME": str(cache),
            "TMPDIR": str(run_root),
            "TERM": "xterm-256color",
            "FATHOMABLE_LOG": "off",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
        }
    )
    return result


def launch(
    binary: Path,
    workspace: Path,
    config: Path,
    root: Path,
    run_number: int,
) -> PtyProcess:
    return PtyProcess(binary, workspace, config, environment(root, run_number))


def exercise_input(runner: PtyProcess, workspace: Path) -> tuple[list[float], int]:
    before = len(runner.frames)
    runner.send(b" ff")
    runner.wait_frame(before, screen_matches=lambda screen: FILE_PICKER in screen)

    stop = threading.Event()
    created = [0]

    def churn() -> None:
        for index in range(INPUT_SAMPLES * 2):
            if stop.wait(0.001):
                return
            directory = workspace / f"pulse-{index:03d}"
            try:
                directory.mkdir()
                (directory / "event.rs").write_text("fn pulse() {}\n", encoding="utf-8")
                created[0] += 1
            except FileExistsError:
                continue

    thread = threading.Thread(target=churn, name="synthetic-churn", daemon=True)
    thread.start()
    latencies: list[float] = []
    expected = ""
    try:
        for index in range(INPUT_SAMPLES):
            if index % 2 == 0:
                expected = chr(ord("a") + (index // 2) % 26)
                key = expected.encode()
            else:
                expected = ""
                key = b"\x7f"
            latency, _screen = runner.send_and_measure(
                key,
                lambda screen, expected=expected: picker_input(screen) == expected,
            )
            latencies.append(latency)
    finally:
        stop.set()
        thread.join(timeout=1.0)
    return latencies, created[0]


def violation(name: str, actual: float, limit: float, unit: str = "ms") -> str | None:
    if actual > limit:
        return f"{name} regression: {actual:.3f}{unit} exceeds {limit:.3f}{unit}"
    return None


def run(binary: Path) -> dict[str, object]:
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RegressionFailure("--bin must name an executable file")
    if not Path("/proc/self/fdinfo").is_dir():
        raise RegressionFailure("the performance harness requires Linux procfs")

    SCRATCH_ROOT.mkdir(mode=0o700, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix="workspace-performance-",
        dir=SCRATCH_ROOT,
    ) as temporary:
        root = Path(temporary)
        config = root / "config.kdl"
        write_config(config)
        small = make_workspace(root / "small", 8)
        moderate = make_workspace(root / "moderate", DIRECTORIES)

        startup_small: list[float] = []
        startup_moderate: list[float] = []
        small_rss: list[int] = []
        keyboard_replies = 0
        run_number = 0

        # Warm executable pages before comparing the two fixture sizes.
        with launch(binary, small, config, root, run_number) as warmup:
            warmup.first_frame_ms()
            keyboard_replies += int(warmup.keyboard_probe_replied)
            warmup.quit()
        run_number += 1

        retained_runner: PtyProcess | None = None
        try:
            for sample in range(3):
                with launch(binary, small, config, root, run_number) as small_run:
                    startup_small.append(small_run.first_frame_ms())
                    keyboard_replies += int(small_run.keyboard_probe_replied)
                    small_rss.append(round(statistics.median(sample_rss(small_run))))
                    small_run.quit()
                run_number += 1

                moderate_run = launch(binary, moderate, config, root, run_number)
                startup_moderate.append(moderate_run.first_frame_ms())
                keyboard_replies += int(moderate_run.keyboard_probe_replied)
                run_number += 1
                if sample == 2:
                    retained_runner = moderate_run
                else:
                    moderate_run.quit()
                    moderate_run.close()

            if retained_runner is None:
                raise RegressionFailure("no moderate synthetic viewer was retained")
            runner = retained_runner
            watch_count, watch_samples = watch_plateau(runner)
            runner.pump(0.1)
            partial_notice = runner.contains(WATCH_NOTICE)

            latencies, churned = exercise_input(runner, moderate)
            input_p99 = percentile(latencies, 0.99)
            input_max = max(latencies)

            runner.pump(0.1)
            moderate_rss_samples = sample_rss(runner)
            moderate_rss = round(statistics.median(moderate_rss_samples[-5:]))
            rss_spread = max(moderate_rss_samples[-5:]) - min(moderate_rss_samples[-5:])
            small_rss_median = round(statistics.median(small_rss))
            rss_delta = max(0, moderate_rss - small_rss_median)

            # A directory event invalidates an active file index. Exiting after
            # this burst exercises worker cancellation and the real PTY path.
            for index in range(32):
                directory = moderate / f"quit-pulse-{index:03d}"
                directory.mkdir(exist_ok=True)
            runner.pump(0.01)
            runner.send_and_measure(
                b"\x1b", lambda screen: picker_input(screen) is None
            )
            runner.send_and_measure(
                b"q", lambda screen: "Quit Fathomable?" in screen
            )
            quit_started = time.perf_counter_ns()
            runner.send(b"\r")
            runner.wait_exit(2.0)
            quit_ms = milliseconds(time.perf_counter_ns() - quit_started)

            small_first = statistics.median(startup_small)
            moderate_first = statistics.median(startup_moderate)
            first_overhead = max(0.0, moderate_first - small_first)

            result: dict[str, object] = {
                "caps": CAPS,
                "fixture_directories": DIRECTORIES,
                "fixture_files": DIRECTORIES * 2 + 1,
                "first_frame_small_median_ms": round(small_first, 3),
                "first_frame_moderate_median_ms": round(moderate_first, 3),
                "first_frame_overhead_ms": round(first_overhead, 3),
                "host_cpu_count": os.cpu_count(),
                "host_load_1m": round(os.getloadavg()[0], 3),
                "inotify_control_watch_allowance": CONTROL_WATCH_ALLOWANCE,
                "inotify_watch_plateau": watch_count,
                "inotify_watch_plateau_above_workspace_cap": max(
                    0, watch_count - WATCH_CAP
                ),
                "inotify_watch_samples": len(watch_samples),
                "inotify_watch_total_limit": TOTAL_WATCH_LIMIT,
                "input_correlations_verified": len(latencies),
                "input_correlation": "exact picker query in reconstructed terminal screen",
                "input_max_ms": round(input_max, 3),
                "input_p99_ms": round(input_p99, 3),
                "input_samples": len(latencies),
                "input_samples_while_scan_churn": len(latencies),
                "keyboard_probe_replies": keyboard_replies,
                "keyboard_probe_mode": (
                    "replied" if keyboard_replies == 7 else "common differential"
                ),
                "partial_watch_notice": partial_notice,
                "quit_to_exit_ms": round(quit_ms, 3),
                "synthetic_churn_directories": churned,
                "worker_cancellation_exercised": churned > 0,
                "thresholds": {
                    "first_frame_overhead_ms": FIRST_FRAME_OVERHEAD_LIMIT_MS,
                    "inotify_watch_total": TOTAL_WATCH_LIMIT,
                    "input_p99_ms": INPUT_P99_LIMIT_MS,
                    "quit_to_exit_ms": QUIT_LIMIT_MS,
                    "warm_rss_delta_kib": WARM_RSS_DELTA_LIMIT_KIB,
                    "warm_rss_plateau_spread_kib": WARM_RSS_SPREAD_LIMIT_KIB,
                },
                "warm_rss_delta_kib": rss_delta,
                "warm_rss_moderate_kib": moderate_rss,
                "warm_rss_plateau_spread_kib": rss_spread,
                "warm_rss_small_median_kib": small_rss_median,
                "warm_rss_scope": "moderate-minus-small process RSS; not a process cap",
            }

            failures = []
            if watch_count <= 0:
                failures.append("synthetic workspace installed no inotify watches")
            if watch_count > TOTAL_WATCH_LIMIT:
                failures.append(
                    f"inotify watch plateau {watch_count} exceeds workspace cap "
                    f"{WATCH_CAP} plus control allowance {CONTROL_WATCH_ALLOWANCE}"
                )
            if not partial_notice:
                failures.append("finite watch coverage ended silently")
            if len(latencies) != INPUT_SAMPLES or churned == 0:
                failures.append("input sampling did not overlap file discovery")
            if keyboard_replies not in (0, 7):
                failures.append("terminal keyboard probes were not handled consistently")
            for message in (
                violation(
                    "first-frame overhead",
                    first_overhead,
                    FIRST_FRAME_OVERHEAD_LIMIT_MS,
                ),
                violation("input p99", input_p99, INPUT_P99_LIMIT_MS),
                violation("quit-to-exit", quit_ms, QUIT_LIMIT_MS),
                violation(
                    "warm RSS delta",
                    float(rss_delta),
                    float(WARM_RSS_DELTA_LIMIT_KIB),
                    "KiB",
                ),
                violation(
                    "warm RSS plateau spread",
                    float(rss_spread),
                    float(WARM_RSS_SPREAD_LIMIT_KIB),
                    "KiB",
                ),
            ):
                if message is not None:
                    failures.append(message)
            if failures:
                raise RegressionFailure("; ".join(failures), result)
            return result
        finally:
            if retained_runner is not None:
                retained_runner.close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run synthetic bounded-workspace release regressions."
    )
    parser.add_argument(
        "--bin",
        required=True,
        type=Path,
        help="existing Fathomable binary to exercise (the harness never builds)",
    )
    return parser.parse_args()


def main() -> int:
    arguments = parse_args()
    result: dict[str, object] = {}
    try:
        result = run(arguments.bin.resolve())
    except RegressionFailure as error:
        if error.metrics is not None:
            result = error.metrics
        if result:
            print(json.dumps(result, indent=2, sort_keys=True), flush=True)
        load = os.getloadavg()[0]
        print(
            f"FAIL: {error}; host_load_1m={load:.3f}; host_cpu_count={os.cpu_count()}",
            file=sys.stderr,
        )
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
