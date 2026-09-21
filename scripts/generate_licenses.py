#!/usr/bin/env python3
# @okf-doc: /decisions/0088-bundled-licenses.md
"""Combine cargo-about's license report with reviewed runtime and asset notices."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from html.parser import HTMLParser
from pathlib import Path
from typing import Any

CARGO_ABOUT_VERSION = "0.9.2"
ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "crates/fathomable/assets/licenses.txt"
SEPARATOR = "=" * 79
NOTICE_NAME = re.compile(r"^(?:notice|copyright)(?:[-_.].*)?$", re.IGNORECASE)
FIRST_PARTY_MANIFESTS = {
    "fathomable": Path("crates/fathomable/Cargo.toml"),
    "fathomable-core": Path("crates/fathomable-core/Cargo.toml"),
    "fathomable-testing": Path("crates/fathomable-testing/Cargo.toml"),
}


class LicenseBundleError(RuntimeError):
    """A license report or supplemental notice is missing or inconsistent."""


def normalize_text(text: str) -> str:
    """Normalize line endings and trailing horizontal whitespace."""
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    return "\n".join(line.rstrip(" \t") for line in text.split("\n")).rstrip("\n") + "\n"


class PlainTextHTML(HTMLParser):
    """Render Rust's upstream legal HTML without page chrome."""

    BLOCKS = {"body", "details", "div", "h1", "h2", "h3", "h4",
              "li", "p", "pre", "summary", "ul"}

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.lines: list[str] = []
        self.current = ""
        self.ignored = 0
        self.in_pre = False

    def flush(self, *, blank: bool = False) -> None:
        if self.current.strip():
            self.lines.append(self.current.strip())
        self.current = ""
        if blank and self.lines and self.lines[-1] != "":
            self.lines.append("")

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag in {"head", "script", "style"}:
            self.ignored += 1
            return
        if self.ignored:
            return
        if tag in self.BLOCKS:
            self.flush()
        if tag == "li":
            self.current = "- "
        elif tag == "pre":
            self.in_pre = True
        elif tag == "br":
            self.flush()

    def handle_endtag(self, tag: str) -> None:
        if self.ignored:
            if tag in {"head", "script", "style"}:
                self.ignored -= 1
            return
        if tag == "pre":
            self.in_pre = False
        if tag in self.BLOCKS:
            self.flush(blank=True)

    def handle_data(self, data: str) -> None:
        if self.ignored:
            return
        if self.in_pre:
            self.flush()
            self.lines.extend(normalize_text(data).rstrip("\n").split("\n"))
            return
        value = " ".join(data.split())
        if value:
            if self.current and not self.current.endswith(" "):
                self.current += " "
            self.current += value

    def text(self) -> str:
        self.flush()
        collapsed: list[str] = []
        for line in self.lines:
            if line or not collapsed or collapsed[-1]:
                collapsed.append(line)
        return normalize_text("\n".join(collapsed))


def run(command: list[str], root: Path) -> str:
    result = subprocess.run(
        [sys.executable, str(root / "scripts/rust-toolchain.py"), "release", *command],
        cwd=root, capture_output=True, text=True, encoding="utf-8", check=False,
    )
    if result.returncode:
        raise LicenseBundleError(
            f"{' '.join(command)} failed:\n{result.stderr.strip() or result.stdout.strip()}"
        )
    if result.stderr:
        print(result.stderr, file=sys.stderr, end="")
    return result.stdout


def cargo_about(root: Path) -> dict[str, Any]:
    version = run(["cargo", "about", "--version"], root).strip()
    if version != f"cargo-about {CARGO_ABOUT_VERSION}":
        raise LicenseBundleError(
            f"expected cargo-about {CARGO_ABOUT_VERSION}, got {version!r}; "
            "run scripts/setup-build-deps.sh"
        )
    return json.loads(run([
        "cargo", "about", "generate", "--frozen", "--fail", "--threshold", "0.9",
        "--manifest-path", "crates/fathomable/Cargo.toml",
        "--config", "scripts/configs/about.toml", "--format", "json",
    ], root))


def package_key(package: dict[str, Any]) -> str:
    return f"{package['name']}@{package['version']}"


def is_first_party(root: Path, package: dict[str, Any]) -> bool:
    """Recognize an exact workspace package, not an arbitrary path dependency."""
    if package["source"] is not None:
        return False
    relative = FIRST_PARTY_MANIFESTS.get(package["name"])
    if relative is None:
        return False
    return Path(package["manifest_path"]).resolve() == (root / relative).resolve()


def source_url(package: dict[str, Any]) -> str:
    if package["source"] != "registry+https://github.com/rust-lang/crates.io-index":
        raise LicenseBundleError(f"unsupported source for {package_key(package)}")
    return f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download"


def verify_digest(data: bytes, expected: str, label: str) -> None:
    actual = hashlib.sha256(data).hexdigest()
    if actual != expected:
        raise LicenseBundleError(
            f"hash mismatch for {label}: expected {expected}, got {actual}"
        )


def pinned_notice(root: Path, entry: dict[str, Any]) -> str:
    path = root / "licenses" / entry["file"]
    raw = path.read_bytes()
    text = normalize_text(raw.decode("utf-8"))
    if entry.get("format") == "html":
        verify_digest(raw, entry["raw_sha256"], entry["file"])
        parser = PlainTextHTML()
        parser.feed(text)
        parser.close()
        return parser.text()
    verify_digest(text.encode("utf-8"), entry["sha256"], entry["file"])
    return text


def section(title: str, text: str) -> str:
    return f"{SEPARATOR}\n{title}\n{SEPARATOR}\n\n{text.rstrip()}\n"


def crate_notices(
    root: Path, inventory: dict[str, Any], manifest: dict[str, Any],
) -> tuple[str, dict[str, dict[str, Any]]]:
    packages = {
        package_key(item["package"]): item["package"]
        for item in inventory["crates"]
        if not is_first_party(root, item["package"])
    }
    if not packages:
        raise LicenseBundleError("cargo-about returned no dependency packages")
    supplemental = manifest["package_notices"]
    stale = set(supplemental) - packages.keys()
    if stale:
        raise LicenseBundleError(f"stale package notices: {', '.join(sorted(stale))}")

    records = []
    for key, package in sorted(packages.items()):
        source = source_url(package)
        records.append(f"{key}\nDeclared license: {package['license']}\nExact source: {source}")
        if "MPL-2.0" in package["license"]:
            records.append(f"MPL-2.0 source availability: {source}")
        records.append("")
    output = [section("DEPENDENCY PACKAGES", "\n".join(records))]
    covered = set()
    for license in sorted(inventory["licenses"], key=lambda item: (item["id"], item["text"])):
        users = sorted(
            key
            for user in license["used_by"]
            if (key := package_key(user["crate"])) in packages
        )
        if not users:
            continue
        covered.update(users)
        # Offline cargo-about can synthesize generic SPDX text without copyrights.
        # Only reviewed replacements may stand in for missing upstream files.
        if not license["source_path"]:
            for key in users:
                if not any(
                    notice.get("license") == license["id"]
                    for notice in supplemental.get(key, [])
                ):
                    raise LicenseBundleError(
                        f"cargo-about synthesized {license['id']} for {key}; "
                        "add a verified scripts/configs/about.toml clarification "
                        "or pinned package notice"
                    )
            continue
        output.append(section(
            f"{license['name']} ({license['id']})\nUsed by: {', '.join(users)}",
            normalize_text(license["text"]),
        ))
    if covered != packages.keys():
        raise LicenseBundleError("cargo-about license coverage does not match its package inventory")

    for key, package in sorted(packages.items()):
        for entry in supplemental.get(key, []):
            output.append(section(
                f"{key}: {entry['file']}\nSource: {entry['source']}",
                pinned_notice(root, entry),
            ))
        # NOTICE and COPYRIGHT files are not license expressions; cargo-about
        # does not automatically include these separate attribution documents.
        crate_root = Path(package["manifest_path"]).parent
        for path in sorted(crate_root.iterdir()):
            if path.is_file() and NOTICE_NAME.fullmatch(path.name):
                output.append(section(
                    f"{key}: {path.name}\nSource: {source_url(package)}",
                    normalize_text(path.read_text(encoding="utf-8")),
                ))
    return "\n".join(output), packages


def runtime_notices(root: Path, manifest: dict[str, Any]) -> str:
    runtime = manifest["rust_standard_library"]
    fields = dict(
        line.split(":", 1) for line in run(["rustc", "--version", "--verbose"], root).splitlines()
        if ":" in line
    )
    if (fields["release"].strip(), fields["commit-hash"].strip()) != (
        runtime["release"], runtime["rustc_commit"],
    ):
        raise LicenseBundleError("release Rust toolchain does not match attribution manifest")
    if not runtime["documents"]:
        raise LicenseBundleError("Rust runtime inventory has no documents")
    output = [section(
        f"RUST STANDARD LIBRARY {runtime['release']}",
        f"Source: {runtime['source']}\nrustc commit: {runtime['rustc_commit']}",
    )]
    for entry in runtime["documents"]:
        output.append(section(
            f"{entry['name']}\nSource: {entry['source']}", pinned_notice(root, entry),
        ))
    return "\n".join(output)


def asset_notices(
    root: Path, packages: dict[str, dict[str, Any]], manifest: dict[str, Any],
) -> str:
    output = []
    for asset in manifest["embedded_assets"]:
        key = asset["package"]
        if key not in packages:
            raise LicenseBundleError(f"embedded asset owner is outside supported graph: {key}")
        crate_root = Path(packages[key]["manifest_path"]).parent
        revision = json.loads((crate_root / ".cargo_vcs_info.json").read_text())["git"]["sha1"]
        if revision != asset["vcs_revision"]:
            raise LicenseBundleError(f"embedded asset VCS mismatch for {key}")
        if not asset["artifacts"] or not asset["notices"]:
            raise LicenseBundleError(f"embedded asset entry for {key} needs artifacts and notices")
        artifacts = []
        for artifact in asset["artifacts"]:
            verify_digest(
                (crate_root / artifact["path"]).read_bytes(),
                artifact["sha256"], f"{key}: {artifact['path']}",
            )
            artifacts.append(f"{artifact['path']} SHA-256: {artifact['sha256']}")
        output.append(section(
            f"{key}: EMBEDDED THIRD-PARTY DATA",
            f"{asset['description']}\nUpstream revision: {asset['revision']}\n"
            + "\n".join(artifacts),
        ))
        for entry in asset["notices"]:
            output.append(section(
                f"{entry['name']} ({entry['license']})\nSource: {entry['source']}",
                pinned_notice(root, entry),
            ))
    return "\n".join(output)


def render_bundle(root: Path) -> str:
    manifest = json.loads((root / "licenses/manifest.json").read_text(encoding="utf-8"))
    config = tomllib.loads((root / "scripts/configs/about.toml").read_text(encoding="utf-8"))
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]
    if manifest["version"] != 1 or config["targets"] != [manifest["target"]]:
        raise LicenseBundleError(
            "supplemental inventory version/target does not match scripts/configs/about.toml"
        )
    if workspace["license"] != "MIT":
        raise LicenseBundleError("Fathomable's declared license is no longer MIT")
    crates, packages = crate_notices(root, cargo_about(root), manifest)
    header = [
        "Fathomable licenses and third-party notices",
        f"Generated by cargo-about {CARGO_ABOUT_VERSION} and scripts/generate_licenses.py.",
        "Do not edit this file; refresh it with the contributor tooling.",
        f"Target: {manifest['target']}; normal/build dependencies, excluding dev dependencies.",
        f"External package versions: {len(packages)}",
        "The Rust runtime inventory and embedded data notices are included separately.",
        "The Rust runtime notices describe the recorded release compiler; source builds",
        "using another compiler require runtime-notice review before redistribution.",
        "This is not a byte-for-byte software bill of materials. System linker/startup",
        "objects and dynamic OS libraries are outside scope; review native inputs",
        "and applicable obligations before distributing binaries for another target.",
        "",
    ]
    for path in ("Cargo.lock", "scripts/configs/about.toml", "licenses/manifest.json"):
        header.append(f"{path} SHA-256: {hashlib.sha256((root / path).read_bytes()).hexdigest()}")
    result = "\n\n".join([
        "\n".join(header),
        section(
            f"Fathomable {workspace['version']} (MIT)\nSource: https://github.com/hbeberman/fathomable",
            normalize_text((root / "LICENSE").read_text(encoding="utf-8")),
        ),
        crates,
        runtime_notices(root, manifest),
        asset_notices(root, packages, manifest),
    ])
    if str(root.resolve()) in result:
        raise LicenseBundleError("generated bundle leaked the local checkout path")
    return normalize_text(result)


def check_output(expected: str, path: Path) -> None:
    if not path.is_file() or path.read_bytes() != expected.encode("utf-8"):
        raise LicenseBundleError(
            f"{path} is missing or stale; run scripts/generate_licenses.py and commit the result"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="reject missing or stale committed notices")
    args = parser.parse_args()
    try:
        rendered = render_bundle(ROOT)
        if args.check:
            check_output(rendered, OUTPUT)
            print(f"{OUTPUT.relative_to(ROOT)} is current")
        else:
            OUTPUT.parent.mkdir(parents=True, exist_ok=True)
            OUTPUT.write_text(rendered, encoding="utf-8", newline="\n")
            print(f"wrote {OUTPUT.relative_to(ROOT)}")
    except (LicenseBundleError, OSError, UnicodeError, json.JSONDecodeError,
            tomllib.TOMLDecodeError) as error:
        print(f"license generation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
