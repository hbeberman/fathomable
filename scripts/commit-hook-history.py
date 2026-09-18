#!/usr/bin/env python3
# @okf-doc: /commit-hooks.md
"""Keep timestamped native prek traces and console output for Git hook attempts."""

# ponytail: retain native traces instead of maintaining a report parser.
import datetime
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time


def warn(message):
    print(f"commit-hook-history: {message}", file=sys.stderr)


def observe(command):
    started = time.monotonic()
    triggered = datetime.datetime.now(datetime.timezone.utc)
    directory = Path.cwd() / ".tmp/commit-hook-history"
    try:
        directory.mkdir(parents=True, exist_ok=True)
        fd, name = tempfile.mkstemp(
            prefix=triggered.strftime("%Y%m%dT%H%M%S.%fZ-"), suffix=".prek.log", dir=directory,
        )
        os.close(fd)
        trace = Path(name)
        output = trace.with_name(trace.name.removesuffix(".prek.log") + ".output.log")
        fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        log = os.fdopen(fd, "wb")
    except OSError as error:
        warn(f"cannot create logs: {error}; continuing with native prek")
        os.execvp(command[0], command)

    failed_write = False

    def record(data):
        nonlocal failed_write
        if not failed_write:
            try:
                log.write(data)
                log.flush()
            except OSError as error:
                failed_write = True
                warn(f"cannot write {output}: {error}; preserving prek's result")

    record((
        f"triggered_at={triggered.isoformat()}\n"
        f"worktree={Path.cwd()}\nobserver_pid={os.getpid()}\n\n"
    ).encode())
    native = [command[0], "--log-file", str(trace), *command[1:]]
    if sys.stdout.isatty() and "PREK_COLOR" not in os.environ:
        native.insert(1, "--color=always")
    process = None
    interrupted = None
    handlers = {}
    exit_code = 1

    def send_signal(signum):
        if process is not None:
            try:
                os.killpg(process.pid, signum)
            except ProcessLookupError:
                pass  # The child already exited.

    def forward(signum, _frame):
        nonlocal interrupted
        interrupted = signum
        send_signal(signum)

    try:
        for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            handlers[signum] = signal.signal(signum, forward)
        process = subprocess.Popen(
            native, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True,
        )
        if interrupted is not None:
            send_signal(interrupted)
        with process.stdout:
            while chunk := process.stdout.read1(8192):
                record(chunk)
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()
        code = process.wait()
        exit_code = code if code >= 0 else 128 - code
    except OSError as error:
        warn(str(error))
        record(f"\nobserver_error={error}\n".encode())
        if process is None:
            exit_code = 127
        else:
            send_signal(signal.SIGTERM)
            code = process.wait()
            exit_code = code if code > 0 else 1
    finally:
        reason = "passed" if exit_code == 0 else "failed"
        if interrupted is not None:
            reason = signal.Signals(interrupted).name
        record((
            f"\nfinished_at={datetime.datetime.now(datetime.timezone.utc).isoformat()}\n"
            f"duration_seconds={time.monotonic() - started:.6f}\n"
            f"exit_code={exit_code}\nexit_reason={reason}\n"
        ).encode())
        try:
            log.close()
        except OSError as error:
            warn(f"cannot close {output}: {error}")
        for signum, handler in handlers.items():
            signal.signal(signum, handler)
    return exit_code


def instrument_shim(path):
    content = path.read_text()
    prefix = 'exec "$PREK" hook-impl '
    if content.count(prefix) != 1:
        raise ValueError("unrecognized native prek shim; refusing to replace the installed hook")
    path.write_text(content.replace(
        prefix, 'exec python3 scripts/commit-hook-history.py -- "$PREK" hook-impl ',
    ))


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--instrument-shim":
        try:
            instrument_shim(Path(sys.argv[2]))
        except (OSError, ValueError) as error:
            warn(str(error))
            sys.exit(1)
    elif len(sys.argv) >= 3 and sys.argv[1] == "--":
        sys.exit(observe(sys.argv[2:]))
    else:
        sys.exit("usage: commit-hook-history.py --instrument-shim PATH | -- PREK ARGS...")
