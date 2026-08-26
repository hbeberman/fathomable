from __future__ import annotations

# @okf-doc: /okf.md

import argparse
import re

# Git is an external tool; subprocess is the intended adapter boundary.
import subprocess  # ruff:ignore[S404]
import sys
from pathlib import Path, PurePosixPath
from urllib.parse import urlsplit

import yaml

OKF_VERSION = "0.2"
DATE_HEADING = re.compile(r"^##\s+(\d{4}-\d{2}-\d{2})\s*$")
TAG_VALUE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
COMMENT_PREFIX = re.compile(r"^\s*(?:#|//|;|--|/\*+|\*+|REM(?:\s+|$))\s*", re.IGNORECASE)
DOC_REF_MARKER = "@okf-doc:"
DOC_REF = re.compile(r"^@okf-doc:\s+(?P<slug>/[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*\.md)\s*$")


def split_frontmatter(text: str) -> tuple[str | None, bool]:
    """Return frontmatter text and whether both delimiters were present."""
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return None, False
    for index, line in enumerate(lines[1:], start=1):
        if line.strip() == "---":
            return "\n".join(lines[1:index]), True
    return None, False


def git_root(path: Path) -> Path | None:
    """Return the repository containing ``path``, or ``None`` outside Git."""
    result = subprocess.run(
        ["git", "-C", str(path), "rev-parse", "--show-toplevel"],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        return None
    return Path(result.stdout.decode("utf-8").strip()).resolve()


class Linter:
    """Accumulate OKF conformance errors across one documentation bundle."""

    def __init__(self, *, repo_root: Path | None) -> None:
        """Create a linter for resources rooted in ``repo_root``."""
        self.docs_by_slug: dict[str, Path] = {}
        self.doc_slugs: dict[Path, str] = {}
        self.errors: list[str] = []
        self.resource_refs: dict[Path, set[str]] = {}
        self.tag_definitions: set[str] | None = None
        self.tag_usages: list[tuple[Path, str]] = []
        self.repo_root = repo_root

    def error(self, path: Path, message: str) -> None:
        """Record an error for ``path``."""
        self.errors.append(f"{path}: {message}")

    def lint_bundle(self, root: Path) -> None:
        """Lint every Markdown file below ``root``."""
        if not root.is_dir():
            self.errors.append(f"{root}: not a directory")
            return

        markdown_files = sorted(root.rglob("*.md"))
        for path in markdown_files:
            slug = f"/{path.relative_to(root).as_posix()}"
            resolved = path.resolve()
            self.docs_by_slug[slug] = resolved
            self.doc_slugs[resolved] = slug
        for path in markdown_files:
            self.lint_file(root, path)
        self.lint_tag_usages(root)
        self.lint_resource_backlinks()

    def lint_file(self, root: Path, path: Path) -> None:
        """Validate one Markdown file against OKF bundle rules."""
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            self.error(path, "file is not valid UTF-8")
            return

        frontmatter, has_frontmatter = split_frontmatter(text)
        is_root_index = path.parent == root and path.name == "index.md"
        if path.name == "index.md":
            self.lint_index(
                path,
                frontmatter,
                has_frontmatter=has_frontmatter,
                is_root=is_root_index,
            )
            return
        if path.name == "log.md":
            self.lint_log(path, text)
            return
        if not has_frontmatter:
            self.error(path, "missing YAML frontmatter block delimited by `---`")
            return

        data = self.parse_frontmatter(path, frontmatter)
        if data is None:
            return
        type_value = data.get("type")
        if not isinstance(type_value, str) or not type_value.strip():
            self.error(path, "frontmatter is missing a non-empty `type` field")
        if path.parent == root and path.name == "tags.md":
            self.lint_tag_catalog(path, data)
        self.lint_tags(path, data)
        self.lint_resource(path, data)

    def lint_index(
        self,
        path: Path,
        frontmatter: str | None,
        *,
        has_frontmatter: bool,
        is_root: bool,
    ) -> None:
        """Validate a root or nested bundle index."""
        if not is_root:
            if has_frontmatter:
                self.error(path, "nested index.md must not contain frontmatter")
            return
        if not has_frontmatter:
            return
        data = self.parse_frontmatter(path, frontmatter)
        if data is not None and str(data.get("okf_version")) != OKF_VERSION:
            self.error(path, f"frontmatter `okf_version` must be {OKF_VERSION!r}")

    def parse_frontmatter(self, path: Path, frontmatter: str | None) -> dict[object, object] | None:
        """Parse a frontmatter mapping, recording an error when invalid."""
        try:
            data = yaml.safe_load(frontmatter) if frontmatter and frontmatter.strip() else None
        except yaml.YAMLError as error:
            self.error(path, f"frontmatter is not parseable YAML: {error}")
            return None
        if not isinstance(data, dict):
            self.error(path, "frontmatter must be a YAML mapping")
            return None
        return data

    def lint_log(self, path: Path, text: str) -> None:
        """Validate ISO-8601 date headings in a reserved log file."""
        for line in text.splitlines():
            if line.startswith("## ") and DATE_HEADING.fullmatch(line) is None:
                self.error(path, f"log date heading must be `## YYYY-MM-DD`, got {line!r}")

    def lint_tags(self, path: Path, data: dict[object, object]) -> None:
        """Validate optional discovery tags without imposing a closed vocabulary."""
        if "tags" not in data:
            return
        tags = data["tags"]
        if not isinstance(tags, list):
            self.error(path, "frontmatter `tags` must be a list")
            return
        seen: set[str] = set()
        for tag in tags:
            if not isinstance(tag, str) or TAG_VALUE.fullmatch(tag) is None:
                self.error(path, "frontmatter `tags` entries must be lowercase kebab-case strings")
                continue
            if tag in seen:
                self.error(path, f"frontmatter `tags` contains duplicate tag {tag!r}")
                continue
            seen.add(tag)
            self.tag_usages.append((path, tag))

    def lint_tag_catalog(self, path: Path, data: dict[object, object]) -> None:
        """Validate the root vocabulary of reusable discovery tags."""
        self.tag_definitions = set()
        definitions = data.get("tag_definitions")
        if not isinstance(definitions, dict):
            self.error(path, "frontmatter `tag_definitions` must be a mapping")
            return
        for name, description in definitions.items():
            if not isinstance(name, str) or TAG_VALUE.fullmatch(name) is None:
                self.error(path, "frontmatter `tag_definitions` names must be lowercase kebab-case strings")
                continue
            if not isinstance(description, str) or not description.strip():
                self.error(path, f"frontmatter `tag_definitions.{name}` must be a non-empty string")
                continue
            self.tag_definitions.add(name)

    def lint_tag_usages(self, root: Path) -> None:
        """Require used tags to come from the extensible root vocabulary."""
        if not self.tag_usages:
            return
        if self.tag_definitions is None:
            self.error(
                root,
                "bundle uses tags but is missing tags.md; create the open tag vocabulary, where well-considered "
                "new tags are welcome",
            )
            return
        for path, tag in self.tag_usages:
            if tag not in self.tag_definitions:
                self.error(
                    path,
                    f"frontmatter tag {tag!r} is not defined in tags.md; new tags are welcome when they are "
                    f"well-considered, durable discovery terms, so reuse an existing tag or add {tag!r} with a "
                    "concise description",
                )

    def lint_resource(self, path: Path, data: dict[object, object]) -> None:
        """Validate a concept's declared resource and related local resources."""
        slug = self.doc_slugs[path.resolve()]

        resource_target: Path | None = None
        if "resource" in data:
            resource = data["resource"]
            if not isinstance(resource, str) or not resource.strip():
                self.error(path, "frontmatter `resource` must be a non-empty string")
            else:
                resource_target = self._register_resource_reference(path, resource.strip(), slug)

        if "related_resources" not in data:
            return
        related_raw = data["related_resources"]
        if not isinstance(related_raw, list):
            self.error(path, "frontmatter `related_resources` must be a list of repository-relative paths")
            return
        for item in related_raw:
            if not isinstance(item, str) or not item.strip():
                self.error(path, "frontmatter `related_resources` entries must be non-empty strings")
                continue
            related_resource = item.strip()
            if urlsplit(related_resource).scheme:
                self.error(path, "frontmatter `related_resources` entries must be repository-relative paths")
                continue
            target = self._register_resource_reference(path, related_resource, slug)
            if target is not None and target == resource_target:
                self.error(path, "frontmatter `related_resources` may not duplicate `resource`")

    def _register_resource_reference(self, path: Path, resource: str, slug: str) -> Path | None:
        """Resolve and register a documented resource reference; return the resolved path."""
        if urlsplit(resource).scheme:
            return None
        target = self.local_resource(path, resource)
        if target is None:
            return None
        if target not in self.resource_refs:
            self.resource_refs[target] = set()
        self.resource_refs[target].add(slug)
        return target

    def local_resource(self, path: Path, resource: str) -> Path | None:
        """Resolve a valid repository-relative resource path."""
        if self.repo_root is None:
            self.error(path, f"cannot validate local resource outside a Git worktree: {resource!r}")
            return None
        pure_resource = PurePosixPath(resource)
        if pure_resource.is_absolute() or ".." in pure_resource.parts or pure_resource.as_posix() != resource:
            self.error(path, f"local resource must be a canonical repository-relative path: {resource!r}")
            return None
        target = self.repo_root / resource
        if target.is_symlink() or not target.is_file():
            self.error(path, f"local resource does not exist as a regular file: {resource!r}")
            return None
        target = target.resolve()
        if not target.is_relative_to(self.repo_root):
            self.error(path, f"local resource escapes the repository: {resource!r}")
            return None
        return target

    def lint_resource_backlinks(self) -> None:
        """Require each resource to link back to exactly one canonical concept document."""
        for resource, owners in self.resource_refs.items():
            try:
                text = resource.read_text(encoding="utf-8")
            except UnicodeDecodeError:
                self.error(resource, "local resource is not valid UTF-8")
                continue
            references = self.document_references(resource, text)
            if len(references) > 1:
                self.error(resource, f"resource may contain at most one `{DOC_REF_MARKER}` reference")
            missing = sorted(owners - references)
            if missing:
                details = ", ".join(f"`{DOC_REF_MARKER} {slug}`" for slug in missing)
                self.error(resource, f"resource must contain backlink comments for: {details}")

    def document_references(self, path: Path, text: str) -> set[str]:
        """Return valid document slugs referenced by source comments."""
        references: set[str] = set()
        for line_number, line in enumerate(text.splitlines(), start=1):
            prefix = COMMENT_PREFIX.match(line)
            if prefix is None:
                continue
            comment = line[prefix.end() :].strip()
            if comment.endswith("*/"):
                comment = comment[:-2].rstrip()
            if DOC_REF_MARKER not in comment:
                continue
            match = DOC_REF.fullmatch(comment)
            if match is None:
                self.error(path, f"line {line_number}: malformed `{DOC_REF_MARKER}` comment")
                continue
            slug = match.group("slug")
            if slug not in self.docs_by_slug:
                self.error(path, f"line {line_number}: document slug does not exist: {slug!r}")
                continue
            references.add(slug)
        return references


def main(argv: list[str]) -> int:
    """Run the OKF linter and return its process exit code."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", nargs="?", default="docs")
    parser.add_argument(
        "--repo-root",
        type=Path,
        help="repository root for local resources; defaults to the bundle's Git worktree",
    )
    arguments = parser.parse_args(argv)

    bundle = Path(arguments.bundle).resolve()
    repo_root = arguments.repo_root.resolve() if arguments.repo_root is not None else git_root(bundle)
    if repo_root is not None and not repo_root.is_dir():
        parser.error(f"repository root is not a directory: {repo_root}")
    linter = Linter(repo_root=repo_root)
    linter.lint_bundle(bundle)
    for error in linter.errors:
        print(f"error: {error}", file=sys.stderr)
    if linter.errors:
        print(f"okf-lint: FAIL ({len(linter.errors)} error(s)) in {arguments.bundle}", file=sys.stderr)
        return 1
    print(f"okf-lint: OK in {arguments.bundle}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

