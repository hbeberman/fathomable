#!/usr/bin/env python3
# @okf-doc: /secret-scanning.md
"""Pinned, offline secret scans. Scanner diagnostics never reach public logs."""

import argparse
from collections import Counter
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import urllib.request


ROOT = Path(__file__).resolve().parent.parent
VERSION = "1.8.1"
# Reviewed against the v1.8.1 release checksums.txt, not downloaded trust on use.
CHECKSUMS = {
    "x86_64": ("x64", "efa407244e1ea8e35f582b8a42becdeac08bdead04f68eb752adda722d583c2a"),
    "aarch64": ("arm64", "bbb578b12a2f65d7082ab436abf37724232bc71d8a078e3c41336574420f1b48"),
}
TOOL = ROOT / ".tmp" / "tools" / f"betterleaks-{VERSION}" / "betterleaks"
POLICY_PATHS = [
    "scripts/betterleaks.py", "scripts/betterleaks.toml",
    ".github/workflows/secrets.yml",
]


class ScanError(Exception):
    pass


def run(command, *, cwd, env=None, timeout=600, content=None):
    return subprocess.run(
        command, cwd=cwd, env=env, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, timeout=timeout, check=False, input=content,
    )


def git(repo, *args):
    result = run([
        "git", "-c", "core.quotePath=false", "-c", "core.pager=cat", *args,
    ], cwd=repo, env=os.environ | {"GIT_NO_REPLACE_OBJECTS": "1"})
    if result.returncode:
        raise ScanError("Git scope resolution failed; diagnostics withheld.")
    return result.stdout


def install():
    if platform.system() != "Linux" or platform.machine() not in CHECKSUMS:
        raise ScanError("The pinned installer supports Linux x86_64 and aarch64.")
    arch, digest = CHECKSUMS[platform.machine()]
    url = (
        f"https://github.com/betterleaks/betterleaks/releases/download/v{VERSION}/"
        f"betterleaks_{VERSION}_linux_{arch}.tar.gz"
    )
    with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != digest:
        raise ScanError("Betterleaks archive checksum mismatch; not installed.")
    TOOL.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        # Extract only these regular files, never archive-supplied paths or links.
        for name in ("betterleaks", "LICENSE"):
            member = archive.getmember(name)
            if not member.isfile():
                raise ScanError("Unexpected Betterleaks archive member type.")
            with archive.extractfile(member) as source:
                payload = source.read()
            destination = TOOL.parent / name
            with tempfile.NamedTemporaryFile(dir=TOOL.parent, delete=False) as output:
                pending = Path(output.name)
                try:
                    output.write(payload)
                    output.flush()
                    pending.chmod(0o700 if name == "betterleaks" else 0o600)
                    pending.replace(destination)
                finally:
                    pending.unlink(missing_ok=True)
    print(f"Installed checksum-verified Betterleaks {VERSION} under .tmp/tools/.")


def scan(repo, arguments, content=None):
    if not TOOL.is_file():
        raise ScanError("Betterleaks is missing; run python3 scripts/betterleaks.py install.")
    environment = {
        key: value for key, value in os.environ.items()
        if not key.startswith(("BETTERLEAKS_", "GITLEAKS_"))
    }
    version = run([str(TOOL), "version"], cwd=repo, env=environment, timeout=10)
    if version.returncode or version.stdout.decode().strip().removeprefix("v") != VERSION:
        raise ScanError("Betterleaks version does not match the reviewed pin.")
    scratch = ROOT / ".tmp"
    scratch.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="secret-scan-", dir=scratch) as directory:
        environment["TMPDIR"] = directory
        ignore = Path(directory) / "empty-ignore"
        ignore.touch(mode=0o600)
        if arguments[0] == "git":
            # v1.8.1 also loads ignores from the source directory even with -i.
            # Bind Git to the original repository but give the scanner an empty
            # source directory. Preserve alternate-index/commit --only semantics.
            environment["GIT_DIR"] = git(repo, "rev-parse", "--absolute-git-dir").decode().strip()
            environment["GIT_WORK_TREE"] = directory
            if environment.get("GIT_INDEX_FILE"):
                environment["GIT_INDEX_FILE"] = str((repo / environment["GIT_INDEX_FILE"]).absolute())
            arguments = [arguments[0], directory, *arguments[2:]]
        result = run([
            str(TOOL), *arguments,
            "--config", str(ROOT / "scripts/betterleaks.toml"),
            "--gitleaks-ignore-path", str(ignore),
            "--ignore-gitleaks-allow", "--redact=100", "--validation=false",
            "--no-banner", "--no-color", "--log-level=warn", "--exit-code=1",
            "--max-archive-depth=8", "--max-decode-depth=5", "--timeout=540",
            "--report-format=json", "--report-path=-",
        ], cwd=directory, env=environment, content=content)
    # Redaction alone does not sanitize arbitrary Git errors, paths or context.
    # Keep the report in memory and emit only counts and validated rule IDs.
    # Upstream reports archive failures/depth limits as warnings, sometimes
    # with exit 0. Only its ordinary findings summary is an acceptable warning.
    diagnostics = result.stderr.decode(errors="replace").splitlines()
    unexpected = [line for line in diagnostics if not re.fullmatch(
        r"\d{1,2}:\d{2}[AP]M WRN leaks found: \d+", line
    )]
    if result.returncode not in (0, 1) or unexpected:
        raise ScanError("Secret scan failed; scanner diagnostics withheld (not a clean scan).")
    try:
        findings = json.loads(result.stdout)
        if findings is None:
            findings = []  # Upstream emits JSON null for an empty result.
        if not isinstance(findings, list):
            raise ValueError
        rules = Counter(item["RuleID"] for item in findings)
        if any(not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,100}", rule) for rule in rules):
            raise ValueError
    except (ValueError, TypeError, KeyError):
        raise ScanError("Invalid scanner report; diagnostics withheld.") from None
    if findings:
        counts = ", ".join(f"{rule}: {count}" for rule, count in sorted(rules.items()))
        raise ScanError(f"Secret scan rejected {len(findings)} finding(s) ({counts}).")
    if result.returncode:
        raise ScanError("Secret scan failed without findings; diagnostics withheld.")
    print("Betterleaks: no findings in the selected scope.")


def tracked(repo):
    """Prek hides unstaged tracked edits, but --all-files intentionally does not."""
    if git(repo, "ls-files", "--unmerged", "-z"):
        raise ScanError("Resolve unmerged entries before scanning.")
    scratch = ROOT / ".tmp"
    scratch.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="secret-tree-", dir=scratch) as directory:
        snapshot = Path(directory)
        for raw in git(repo, "ls-files", "--cached", "-z").split(b"\0"):
            if not raw:
                continue
            relative = Path(os.fsdecode(raw))
            if relative.is_absolute() or ".." in relative.parts:
                raise ScanError("Unsafe tracked path.")
            source = repo / relative
            if not source.parent.resolve().is_relative_to(repo):
                raise ScanError("Tracked parent escapes the checkout.")
            try:
                mode = source.lstat().st_mode
            except FileNotFoundError:
                continue  # Current-checkout deletion under --all-files.
            # Do not put repository-controlled ignore files at the scan root.
            destination = snapshot / "tree" / relative
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            if stat.S_ISLNK(mode):
                destination.write_bytes(os.fsencode(os.readlink(source)))
            elif stat.S_ISREG(mode):
                shutil.copyfile(source, destination)
            else:
                raise ScanError("Tracked non-file/submodule requires a separate scan.")
        scan(repo, ["dir", str(snapshot)])


def revision(repo, value):
    if not re.fullmatch(r"[0-9a-f]{40}", value):
        raise ScanError("History boundaries must be full commit IDs.")
    return git(repo, "rev-parse", "--verify", f"{value}^{{commit}}").decode().strip()


def history(repo, base=None, head=None):
    if git(repo, "rev-parse", "--is-shallow-repository").strip() != b"false":
        raise ScanError("History scanning requires a full fetch (fetch-depth: 0).")
    if head is None:
        git(repo, "rev-parse", "--verify", "HEAD^{commit}")
        selection = "--all"
    else:
        head = revision(repo, head)
        selection = head if base is None else f"{revision(repo, base)}..{head}"
    # Separate merge diffs include merge-resolution additions, not only parents.
    options = f"--full-history --diff-merges=separate --no-ext-diff --no-textconv {selection}"
    scan(repo, ["git", str(repo), f"--log-opts={options}"])


def ci(repo, event_path):
    event = json.loads(event_path.read_text())
    kind = os.environ.get("GITHUB_EVENT_NAME")
    if kind in ("schedule", "workflow_dispatch"):
        return history(repo)
    if kind == "pull_request":
        base = revision(repo, event["pull_request"]["base"]["sha"])
        head = revision(repo, event["pull_request"]["head"]["sha"])
    elif kind == "push":
        if event.get("deleted") is True and event["after"] == "0" * 40:
            print("Deleted ref: no introduced commits.")
            return
        head = revision(repo, event["after"])
        base = event["before"]
        if base == "0" * 40:
            print("New ref: scanning all fetched history.")
            return history(repo)
        try:
            base = revision(repo, base)
        except ScanError:
            print("Previous push boundary unavailable: scanning all fetched history.")
            return history(repo)
    else:
        raise ScanError("Unsupported CI event.")
    changes = git(
        repo, "log", "--format=", "--name-only", "--diff-merges=separate",
        f"{base}..{head}", "--", *POLICY_PATHS,
    )
    if changes.strip():
        print("Scanner/policy update: scanning all fetched history.")
        history(repo)
    else:
        history(repo, base, head)


def artifacts(repo, paths, packages=False, binary=False):
    if packages or binary:
        result = run(["cargo", "metadata", "--format-version=1", "--no-deps", "--locked"], cwd=repo)
        if result.returncode:
            raise ScanError("Cannot resolve exact Cargo artifact paths.")
        metadata = json.loads(result.stdout)
        target = Path(metadata["target_directory"])
        if binary:
            paths = [target / "x86_64-unknown-linux-gnu/release/fathomable"]
        else:
            paths = [
                target / "package" / f'{package["name"]}-{package["version"]}.crate'
                for package in metadata["packages"]
                if package["id"] in metadata["workspace_members"] and package["publish"] != []
            ]
    if not paths:
        raise ScanError("Select at least one exact distribution artifact.")
    resolved = []
    for path in paths:
        path = (repo / path).absolute()
        if path.is_symlink() or not path.is_file() or not path.stat().st_size:
            raise ScanError("Distribution artifact missing, empty, symlinked or not a file.")
        resolved.append(path)
    scratch = ROOT / ".tmp"
    scratch.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="secret-artifacts-", dir=scratch) as directory:
        copies = []
        for index, path in enumerate(resolved):
            # v1.8.1 identifies archives by extension; .crate is gzip tar but
            # not recognized. Copy exact bytes under the recognized suffix.
            suffix = ".tar.gz" if path.suffix == ".crate" else "".join(path.suffixes)
            destination = Path(directory) / f"artifact-{index}{suffix}"
            shutil.copyfile(path, destination)
            copies.append(str(destination))
        scan(repo, ["dir", *copies])
        # Betterleaks skips ELF/other application binaries in dir mode. GNU
        # strings scans the exact bytes, including non-loaded data sections.
        for path in copies:
            strings = run(["strings", "--all", "--bytes=4", path], cwd=repo)
            if strings.returncode:
                raise ScanError("Cannot inspect artifact strings.")
            scan(repo, ["stdin"], content=strings.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scope", choices=("install", "tracked", "staged", "history", "range", "ci", "artifacts"))
    parser.add_argument("paths", nargs="*", type=Path)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--base")
    parser.add_argument("--head")
    parser.add_argument("--event", type=Path)
    outputs = parser.add_mutually_exclusive_group()
    outputs.add_argument("--packages", action="store_true")
    outputs.add_argument("--release-binary", action="store_true")
    args = parser.parse_args()
    repo = args.repo.resolve()
    try:
        if args.scope == "install":
            install()
        elif args.scope == "tracked":
            tracked(repo)
        elif args.scope == "staged":
            scan(repo, ["git", str(repo), "--pre-commit", "--staged"])
        elif args.scope == "history":
            history(repo)
        elif args.scope == "range":
            if not args.base or not args.head:
                raise ScanError("Range scans require --base and --head.")
            history(repo, args.base, args.head)
        elif args.scope == "ci":
            if args.event is None:
                raise ScanError("CI scans require an event payload.")
            ci(repo, args.event)
        else:
            artifacts(repo, args.paths, args.packages, args.release_binary)
    except (ScanError, OSError, ValueError, TypeError, KeyError, subprocess.TimeoutExpired, tarfile.TarError) as error:
        message = str(error) if isinstance(error, ScanError) else "Secret scan/setup error; diagnostics withheld."
        print(message, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
