#!/usr/bin/env python3
"""Apply syntax-aware source policies not represented in public API output."""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PUBLIC_USE = re.compile(r"\bpub\s+use\s+([^;]*);", re.DOTALL)


def mask(source: str, start: int, end: int, output: list[str]) -> None:
    for index in range(start, end):
        if source[index] != "\n":
            output[index] = " "


def quoted_end(source: str, quote: int) -> int:
    index = quote + 1
    while index < len(source):
        if source[index] == "\\":
            index += 2
        elif source[index] == '"':
            return index + 1
        else:
            index += 1
    return len(source)


def raw_string_end(source: str, start: int) -> int | None:
    for prefix in ("br", "cr", "r"):
        if not source.startswith(prefix, start):
            continue
        index = start + len(prefix)
        while index < len(source) and source[index] == "#":
            index += 1
        if index >= len(source) or source[index] != '"':
            continue
        terminator = '"' + source[start + len(prefix) : index]
        end = source.find(terminator, index + 1)
        return len(source) if end == -1 else end + len(terminator)
    return None


def char_end(source: str, quote: int) -> int | None:
    if quote + 2 < len(source) and source[quote + 2] == "'":
        return quote + 3
    if quote + 1 >= len(source) or source[quote + 1] != "\\":
        return None

    index = quote + 2
    while index < len(source) and source[index] not in {"'", "\n"}:
        index += 1
    return index + 1 if index < len(source) and source[index] == "'" else None


def mask_non_code(source: str) -> str:
    output = list(source)
    index = 0
    while index < len(source):
        raw_end = raw_string_end(source, index)
        if raw_end is not None:
            mask(source, index, raw_end, output)
            index = raw_end
        elif source.startswith("//", index):
            end = source.find("\n", index + 2)
            end = len(source) if end == -1 else end
            mask(source, index, end, output)
            index = end
        elif source.startswith("/*", index):
            depth = 1
            end = index + 2
            while end < len(source) and depth > 0:
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            mask(source, index, end, output)
            index = end
        elif source[index] == '"':
            end = quoted_end(source, index)
            mask(source, index, end, output)
            index = end
        elif source[index] in {"b", "c"} and source.startswith('"', index + 1):
            end = quoted_end(source, index + 1)
            mask(source, index, end, output)
            index = end
        elif source[index] == "'":
            end = char_end(source, index)
            if end is None:
                index += 1
            else:
                mask(source, index, end, output)
                index = end
        elif source[index] == "b" and source.startswith("'", index + 1):
            end = char_end(source, index + 1)
            if end is None:
                index += 1
            else:
                mask(source, index, end, output)
                index = end
        else:
            index += 1
    return "".join(output)


def main() -> int:
    errors: list[str] = []
    for path in sorted((REPO_ROOT / "crates").glob("**/*.rs")):
        source = path.read_text(encoding="utf-8")
        masked = mask_non_code(source)
        for match in PUBLIC_USE.finditer(masked):
            if "*" not in match.group(1):
                continue
            line = masked.count("\n", 0, match.start()) + 1
            errors.append(f"{path.relative_to(REPO_ROOT)}:{line}: public glob re-export")

    for error in errors:
        print(f"Rust source policy failed: {error}", file=sys.stderr)
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
