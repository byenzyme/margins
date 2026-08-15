#!/usr/bin/env python3
"""Audit and deterministically materialize Margins's candidate public surface."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from functools import lru_cache
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Iterable, Sequence


NORMALIZED_MTIME = 315532800  # 1980-01-01T00:00:00Z; portable to zip tools.
GIT_ENVIRONMENT_OVERRIDES = {
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_DIR",
    "GIT_INDEX_FILE",
    "GIT_NAMESPACE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_REPLACE_REF_BASE",
    "GIT_WORK_TREE",
}


class BoundaryError(RuntimeError):
    """A public-boundary invariant was violated."""


@dataclass(frozen=True)
class IndexEntry:
    """One immutable entry from a Git index tree snapshot."""

    path: str
    mode: str
    object_type: str
    object_id: str


@dataclass(frozen=True)
class CandidateFile:
    """Audited bytes and mode used for hashing and materialization."""

    source_path: str
    path: str
    owner: str
    mode: str
    content: bytes

    @property
    def executable(self) -> bool:
        return self.mode == "100755"


@dataclass
class IndexSnapshot:
    """A stable Git tree created from the index at the start of an invocation."""

    repo: Path
    tree_id: str
    entries: dict[str, IndexEntry]
    _contents: dict[str, bytes]

    def read_file(self, relative: str) -> tuple[str, bytes]:
        try:
            entry = self.entries[relative]
        except KeyError as exc:
            raise BoundaryError(
                f"tracked candidate disappeared from index snapshot: {relative}"
            ) from exc
        if entry.mode == "120000":
            raise BoundaryError(f"symlinks are not exportable: {relative}")
        if entry.object_type != "blob" or entry.mode not in {"100644", "100755"}:
            raise BoundaryError(
                f"tracked candidate is not a regular file: {relative} "
                f"({entry.mode} {entry.object_type})"
            )
        if relative not in self._contents:
            self._contents[relative] = _run_git(
                self.repo, "cat-file", "blob", entry.object_id
            )
        return entry.mode, self._contents[relative]


def _run_git(repo: Path, *args: str) -> bytes:
    environment = os.environ.copy()
    for key in list(environment):
        if key in GIT_ENVIRONMENT_OVERRIDES or key.startswith(
            ("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_")
        ):
            environment.pop(key)
    environment["GIT_NO_REPLACE_OBJECTS"] = "1"
    completed = subprocess.run(
        ["git", "-C", str(repo), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        env=environment,
    )
    if completed.returncode:
        detail = completed.stderr.decode("utf-8", "replace").strip()
        raise BoundaryError(f"git {' '.join(args)} failed: {detail}")
    return completed.stdout


def repository_root(start: Path) -> Path:
    root = _run_git(start, "rev-parse", "--show-toplevel").decode().strip()
    return Path(root).resolve()


def snapshot_index(repo: Path) -> IndexSnapshot:
    """Capture the index as one immutable tree, avoiding worktree/index races."""
    tree_id = _run_git(repo, "write-tree").decode("ascii", "strict").strip()
    output = _run_git(repo, "ls-tree", "-rz", "--full-tree", tree_id)
    entries: dict[str, IndexEntry] = {}
    for record in output.split(b"\0"):
        if not record:
            continue
        try:
            metadata, raw_path = record.split(b"\t", 1)
            raw_mode, raw_type, raw_object_id = metadata.split(b" ", 2)
            decoded = raw_path.decode("utf-8")
            mode = raw_mode.decode("ascii")
            object_type = raw_type.decode("ascii")
            object_id = raw_object_id.decode("ascii")
        except (UnicodeDecodeError, ValueError) as exc:
            raise BoundaryError("Git index tree contains an invalid entry") from exc
        path = normalize_relative_path(decoded)
        if path in entries:
            raise BoundaryError(f"Git index tree returned duplicate path: {path}")
        entries[path] = IndexEntry(path, mode, object_type, object_id)
    return IndexSnapshot(repo, tree_id, entries, {})


def _parse_manifest(content: bytes, source: str) -> dict:
    try:
        manifest = json.loads(content.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise BoundaryError(f"cannot read manifest {source}: {exc}") from exc

    if not isinstance(manifest, dict):
        raise BoundaryError("manifest must be a JSON object")

    if manifest.get("schema_version") != 1:
        raise BoundaryError("manifest schema_version must be 1")
    scopes = manifest.get("scopes")
    if not isinstance(scopes, list) or not scopes:
        raise BoundaryError("manifest scopes must be a non-empty list")
    exact_allowlist = manifest.get("exact_allowlist", False)
    if not isinstance(exact_allowlist, bool):
        raise BoundaryError("manifest exact_allowlist must be a boolean")

    names: set[str] = set()
    for scope in scopes:
        if not isinstance(scope, dict):
            raise BoundaryError(f"each manifest scope must be an object: {scope!r}")
        name = scope.get("name")
        includes = scope.get("include")
        minimum = scope.get("minimum_files")
        required = scope.get("required_files", [])
        if not isinstance(name, str) or not name or name in names:
            raise BoundaryError(
                f"scope names must be unique non-empty strings: {name!r}"
            )
        if (
            not isinstance(includes, list)
            or not includes
            or not all(isinstance(item, str) and item for item in includes)
        ):
            raise BoundaryError(f"scope {name!r} must have non-empty include patterns")
        if len(includes) != len(set(includes)):
            raise BoundaryError(f"scope {name!r} include patterns must be unique")
        for include_path in includes:
            normalized = normalize_relative_path(include_path)
            if normalized != include_path:
                raise BoundaryError(
                    f"scope {name!r} include must be a normalized path or pattern: "
                    f"{include_path!r}"
                )
        if isinstance(minimum, bool) or not isinstance(minimum, int) or minimum < 1:
            raise BoundaryError(
                f"scope {name!r} minimum_files must be a positive integer"
            )
        if not isinstance(required, list) or not all(
            isinstance(item, str) and item for item in required
        ):
            raise BoundaryError(
                f"scope {name!r} required_files must be a list of paths"
            )
        if len(required) != len(set(required)):
            raise BoundaryError(f"scope {name!r} required_files must be unique")
        for required_path in required:
            normalized = normalize_relative_path(required_path)
            if normalized != required_path or any(
                char in required_path for char in "*?"
            ):
                raise BoundaryError(
                    f"scope {name!r} required file must be a literal normalized path: "
                    f"{required_path!r}"
                )
            if not any(matches(required_path, pattern) for pattern in includes):
                raise BoundaryError(
                    f"scope {name!r} required file is not included by that scope: "
                    f"{required_path}"
                )
        if exact_allowlist:
            wildcard = next(
                (include for include in includes if any(char in include for char in "*?")),
                None,
            )
            if wildcard is not None:
                raise BoundaryError(
                    f"scope {name!r} exact allowlist cannot contain a glob: {wildcard!r}"
                )
            if set(includes) != set(required):
                raise BoundaryError(
                    f"scope {name!r} exact allowlist must equal required_files"
                )
        names.add(name)

    export_paths = manifest.get("export_paths", {})
    if not isinstance(export_paths, dict) or not all(
        isinstance(source, str)
        and source
        and isinstance(destination, str)
        and destination
        for source, destination in export_paths.items()
    ):
        raise BoundaryError("manifest export_paths must map source paths to paths")
    destinations: set[str] = set()
    required_owners = {
        path: scope["name"]
        for scope in scopes
        for path in scope.get("required_files", [])
    }
    for source, destination in export_paths.items():
        if normalize_relative_path(source) != source or any(
            char in source for char in "*?"
        ):
            raise BoundaryError(
                f"export path source must be a literal normalized path: {source!r}"
            )
        if normalize_relative_path(destination) != destination or any(
            char in destination for char in "*?"
        ):
            raise BoundaryError(
                "export path destination must be a literal normalized path: "
                f"{destination!r}"
            )
        if source not in required_owners:
            raise BoundaryError(
                f"export path source must be an exact required file: {source}"
            )
        if destination in destinations:
            raise BoundaryError(f"duplicate export path destination: {destination}")
        if destination in required_owners and destination != source:
            raise BoundaryError(
                f"export path destination collides with required source: {destination}"
            )
        destinations.add(destination)

    for key in (
        "deny_paths",
        "import_scan_extensions",
        "forbidden_import_regexes",
        "secret_content_regexes",
    ):
        values = manifest.get(key)
        if not isinstance(values, list) or not all(
            isinstance(item, str) for item in values
        ):
            raise BoundaryError(f"manifest {key} must be a list of strings")

    for key in ("forbidden_import_regexes", "secret_content_regexes"):
        for expression in manifest[key]:
            try:
                re.compile(expression)
            except re.error as exc:
                raise BoundaryError(
                    f"invalid regex in {key}: {expression!r}: {exc}"
                ) from exc
    return manifest


def load_manifest(path: Path) -> tuple[dict, bytes]:
    try:
        content = path.read_bytes()
    except OSError as exc:
        raise BoundaryError(f"cannot read manifest {path}: {exc}") from exc
    return _parse_manifest(content, str(path)), content


def normalize_relative_path(raw: str) -> str:
    if "\\" in raw or any(ord(char) < 32 or ord(char) == 127 for char in raw):
        raise BoundaryError(f"unsafe relative path: {raw!r}")
    normalized = raw.replace(os.sep, "/")
    path = PurePosixPath(normalized)
    if (
        path.is_absolute()
        or ".." in path.parts
        or normalized in ("", ".")
        or re.match(r"^[A-Za-z]:($|/)", normalized)
    ):
        raise BoundaryError(f"unsafe relative path: {raw!r}")
    return path.as_posix()


@lru_cache(maxsize=None)
def _glob_regex(pattern: str) -> re.Pattern[str]:
    """Compile a small, cross-platform glob where ``**`` crosses directories."""
    expression = ""
    index = 0
    while index < len(pattern):
        if pattern[index : index + 3] == "**/":
            expression += "(?:.*/)?"
            index += 3
        elif pattern[index : index + 2] == "**":
            expression += ".*"
            index += 2
        elif pattern[index] == "*":
            expression += "[^/]*"
            index += 1
        elif pattern[index] == "?":
            expression += "[^/]"
            index += 1
        else:
            expression += re.escape(pattern[index])
            index += 1
    return re.compile(f"^{expression}$")


def matches(path: str, pattern: str) -> bool:
    """Match repo-relative POSIX paths with deterministic shell-style globs."""
    return _glob_regex(pattern).fullmatch(path) is not None


def matching_scopes(path: str, manifest: dict) -> list[str]:
    return [
        scope["name"]
        for scope in manifest["scopes"]
        if any(matches(path, pattern) for pattern in scope["include"])
    ]


def exported_path(source: str, manifest: dict) -> str:
    """Return the repository-relative destination for an allowlisted source."""
    return manifest.get("export_paths", {}).get(source, source)


def allowlisted_source(exported: str, manifest: dict) -> str:
    """Map a materialized path back to its exact allowlisted source path."""
    for source, destination in manifest.get("export_paths", {}).items():
        if destination == exported:
            return source
    return exported


def denied_patterns(path: str, manifest: dict) -> list[str]:
    folded = path.casefold()
    return [
        pattern
        for pattern in manifest["deny_paths"]
        if matches(folded, pattern.casefold())
    ]


def select_candidate_paths(
    snapshot: IndexSnapshot, manifest: dict
) -> tuple[list[str], dict[str, str]]:
    selected: list[str] = []
    ownership: dict[str, str] = {}
    counts = {scope["name"]: 0 for scope in manifest["scopes"]}

    for path in sorted(snapshot.entries):
        owners = matching_scopes(path, manifest)
        if len(owners) > 1:
            raise BoundaryError(
                f"ambiguous scope ownership for {path}: {', '.join(owners)}"
            )
        if not owners:
            continue
        denied = denied_patterns(path, manifest)
        if denied:
            raise BoundaryError(
                f"allowlist/denylist overlap for {path}: scope {owners[0]!r}, deny {denied[0]!r}"
            )
        selected.append(path)
        ownership[path] = owners[0]
        counts[owners[0]] += 1

    for scope in manifest["scopes"]:
        found = counts[scope["name"]]
        if found < scope["minimum_files"]:
            raise BoundaryError(
                f"scope {scope['name']!r} requires at least {scope['minimum_files']} files; found {found}"
            )
        missing = sorted(set(scope.get("required_files", [])) - set(selected))
        if missing:
            raise BoundaryError(
                f"scope {scope['name']!r} is missing required files: {', '.join(missing)}"
            )
    return selected, ownership


def scan_content(relative: str, content: bytes, manifest: dict) -> None:
    if b"\0" in content:
        raise BoundaryError(
            f"binary file is not permitted in candidate export: {relative}"
        )
    try:
        text = content.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise BoundaryError(f"candidate file must be UTF-8 text: {relative}") from exc

    for expression in manifest["secret_content_regexes"]:
        if re.search(expression, text):
            raise BoundaryError(
                f"credential signature {expression!r} found in {relative}"
            )

    if PurePosixPath(relative).suffix.casefold() in {
        extension.casefold() for extension in manifest["import_scan_extensions"]
    }:
        for expression in manifest["forbidden_import_regexes"]:
            match = re.search(expression, text)
            if match:
                line = text.count("\n", 0, match.start()) + 1
                raise BoundaryError(
                    f"forbidden private-runtime import in {relative}:{line} matching {expression!r}"
                )


def audit_candidate(snapshot: IndexSnapshot, manifest: dict) -> list[CandidateFile]:
    selected, ownership = select_candidate_paths(snapshot, manifest)
    candidates: list[CandidateFile] = []
    destinations: dict[str, str] = {}
    for relative in selected:
        mode, content = snapshot.read_file(relative)
        scan_content(relative, content, manifest)
        destination = exported_path(relative, manifest)
        if denied := denied_patterns(destination, manifest):
            raise BoundaryError(
                f"export path is forbidden: {relative} -> {destination} "
                f"({denied[0]!r})"
            )
        if prior := destinations.get(destination):
            raise BoundaryError(
                f"export path collision: {prior} and {relative} -> {destination}"
            )
        destinations[destination] = relative
        candidates.append(
            CandidateFile(relative, destination, ownership[relative], mode, content)
        )
    return candidates


def content_digest(candidates: Iterable[CandidateFile]) -> str:
    digest = hashlib.sha256()
    for candidate in candidates:
        digest.update(candidate.path.encode("utf-8"))
        digest.update(b"\0")
        digest.update(candidate.mode.encode("ascii"))
        digest.update(b"\0")
        digest.update(hashlib.sha256(candidate.content).hexdigest().encode("ascii"))
        digest.update(b"\n")
    return digest.hexdigest()


def print_plan(
    snapshot: IndexSnapshot,
    manifest_path: Path,
    candidates: Sequence[CandidateFile],
    output: Path | None,
) -> None:
    total_bytes = sum(len(candidate.content) for candidate in candidates)
    print("DRY RUN: no files written")
    print(f"manifest: {manifest_path}")
    print(f"source: {snapshot.repo} (Git index tree {snapshot.tree_id})")
    if output is not None:
        print(f"requested output: {output.resolve()}")
    print(f"files: {len(candidates)}")
    print(f"bytes: {total_bytes}")
    print(f"sha256: {content_digest(candidates)}")
    for candidate in candidates:
        mapping = (
            candidate.path
            if candidate.source_path == candidate.path
            else f"{candidate.source_path} -> {candidate.path}"
        )
        print(f"{candidate.owner}\t{mapping}")


def _tree_files(root: Path) -> list[str]:
    if root.is_symlink() or not root.is_dir():
        raise BoundaryError(f"verification target must be a real directory: {root}")
    found: list[str] = []
    for current, directories, files in os.walk(root, followlinks=False):
        current_path = Path(current)
        for directory in directories:
            candidate = current_path / directory
            if candidate.is_symlink():
                relative = candidate.relative_to(root).as_posix()
                raise BoundaryError(f"symlink directory is not permitted: {relative}")
        for filename in files:
            candidate = current_path / filename
            relative = normalize_relative_path(candidate.relative_to(root).as_posix())
            if candidate.is_symlink() or not candidate.is_file():
                raise BoundaryError(f"non-regular file is not permitted: {relative}")
            found.append(relative)
    return sorted(found)


def verify_tree(root: Path, manifest: dict) -> tuple[list[str], dict[str, str]]:
    paths = _tree_files(root)
    ownership: dict[str, str] = {}
    counts = {scope["name"]: 0 for scope in manifest["scopes"]}
    for relative in paths:
        if (
            relative in manifest.get("export_paths", {})
            and exported_path(relative, manifest) != relative
        ):
            raise BoundaryError(f"file is not allowlisted at export path: {relative}")
        source = allowlisted_source(relative, manifest)
        owners = matching_scopes(source, manifest)
        if not owners:
            raise BoundaryError(f"file is not allowlisted: {relative}")
        if len(owners) > 1:
            raise BoundaryError(
                f"ambiguous scope ownership for {relative}: {', '.join(owners)}"
            )
        denied = denied_patterns(source, manifest) or denied_patterns(relative, manifest)
        if denied:
            raise BoundaryError(
                f"forbidden path in verified tree: {relative} ({denied[0]!r})"
            )
        scan_content(relative, (root / relative).read_bytes(), manifest)
        ownership[relative] = owners[0]
        counts[owners[0]] += 1
    for scope in manifest["scopes"]:
        found = counts[scope["name"]]
        if found < scope["minimum_files"]:
            raise BoundaryError(
                f"verified tree scope {scope['name']!r} requires at least "
                f"{scope['minimum_files']} files; found {found}"
            )
        required = {
            exported_path(path, manifest) for path in scope.get("required_files", [])
        }
        missing = sorted(required - set(paths))
        if missing:
            raise BoundaryError(
                f"verified tree scope {scope['name']!r} is missing required files: "
                f"{', '.join(missing)}"
            )
    return paths, ownership


def test_export(root: Path, manifest: dict) -> None:
    verify_tree(root, manifest)
    command = [
        "cargo",
        "test",
        "-p",
        "margins-cli",
        "--test",
        "command_contract",
        "--locked",
    ]
    env = os.environ.copy()
    with tempfile.TemporaryDirectory(prefix="margins-public-test-target.") as target:
        env["CARGO_TARGET_DIR"] = target
        completed = subprocess.run(command, cwd=root, check=False, env=env)
    if completed.returncode:
        raise BoundaryError(
            "exported public CLI command-contract test failed "
            f"(exit {completed.returncode})"
        )


def materialize(
    source_repo: Path, output: Path, candidates: Sequence[CandidateFile], manifest: dict
) -> None:
    requested_output = output.absolute()
    if requested_output.exists() or requested_output.is_symlink():
        raise BoundaryError(
            f"refusing to replace existing output path: {requested_output}"
        )
    output = requested_output.resolve()
    if output.exists() or output.is_symlink():
        raise BoundaryError(f"refusing to replace existing output path: {output}")
    if not output.parent.is_dir():
        raise BoundaryError(f"output parent does not exist: {output.parent}")
    if output == source_repo or source_repo in output.parents:
        raise BoundaryError("output must be outside the source repository")

    staging = Path(
        tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent)
    )
    try:
        for candidate in candidates:
            destination = staging / candidate.path
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(candidate.content)
            destination.chmod(0o755 if candidate.executable else 0o644)
            os.utime(destination, (NORMALIZED_MTIME, NORMALIZED_MTIME))

        verified, _ = verify_tree(staging, manifest)
        selected = sorted(candidate.path for candidate in candidates)
        if selected != verified:
            raise BoundaryError(
                "materialized tree differs from the audited export plan"
            )
        for directory in sorted(
            (path for path in staging.rglob("*") if path.is_dir()),
            key=lambda path: len(path.parts),
            reverse=True,
        ):
            directory.chmod(0o755)
            os.utime(directory, (NORMALIZED_MTIME, NORMALIZED_MTIME))
        try:
            output.mkdir(mode=0o700)
        except FileExistsError as exc:
            raise BoundaryError(
                f"output appeared during export; refusing to replace it: {output}"
            ) from exc

        moved: list[Path] = []
        try:
            for child in sorted(staging.iterdir(), key=lambda path: path.name):
                child.rename(output / child.name)
                moved.append(child)
            output.chmod(0o755)
            os.utime(output, (NORMALIZED_MTIME, NORMALIZED_MTIME))
            staging.rmdir()
        except Exception:
            for original in reversed(moved):
                destination = output / original.name
                if destination.exists() or destination.is_symlink():
                    destination.rename(original)
            try:
                output.rmdir()
            except OSError:
                pass
            raise
    except Exception:
        shutil.rmtree(staging, ignore_errors=True)
        raise


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo",
        type=Path,
        help="source Git checkout (defaults to this script's checkout)",
    )
    parser.add_argument(
        "--manifest", type=Path, help="manifest path (defaults inside --repo)"
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="audit the candidate selection without listing it",
    )
    parser.add_argument(
        "--verify-tree", type=Path, help="audit an already materialized public tree"
    )
    parser.add_argument(
        "--test-export",
        type=Path,
        help="verify a materialized public tree, then run its margins-cli command-contract test",
    )
    parser.add_argument(
        "--output", type=Path, help="new directory to use for a candidate export"
    )
    parser.add_argument(
        "--execute",
        action="store_true",
        help="materialize --output; otherwise remain a dry run",
    )
    args = parser.parse_args(argv)
    if args.execute and args.output is None:
        parser.error("--execute requires --output")
    modes = sum(
        (
            bool(args.check),
            args.verify_tree is not None,
            args.test_export is not None,
            bool(args.execute),
        )
    )
    if modes > 1:
        parser.error("choose only one of --check, --verify-tree, --test-export, or --execute")
    return args


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        script_repo = Path(__file__).resolve().parent.parent
        repo = repository_root((args.repo or script_repo).resolve())
        snapshot = snapshot_index(repo)
        manifest_path = (args.manifest or repo / "open-source-boundary.json").absolute()
        if args.manifest is None:
            _, manifest_content = snapshot.read_file("open-source-boundary.json")
            manifest = _parse_manifest(
                manifest_content, "open-source-boundary.json in Git index"
            )
        else:
            manifest, manifest_content = load_manifest(manifest_path)

        if args.verify_tree is not None:
            verification_root = args.verify_tree.absolute()
            paths, _ = verify_tree(verification_root, manifest)
            print(
                f"open-source boundary verified: {len(paths)} files in {verification_root}"
            )
            return 0
        if args.test_export is not None:
            export_root = args.test_export.absolute()
            paths, _ = verify_tree(export_root, manifest)
            test_export(export_root, manifest)
            print(
                f"open-source export tested: margins-cli command_contract passed "
                f"for {len(paths)} files in {export_root}"
            )
            return 0

        candidates = audit_candidate(snapshot, manifest)
        digest = content_digest(candidates)
        if args.check:
            print(
                f"open-source boundary check passed: {len(candidates)} files, sha256 {digest}"
            )
            return 0
        if args.execute:
            embedded = next(
                (
                    candidate.content
                    for candidate in candidates
                    if candidate.source_path == "open-source-boundary.json"
                ),
                None,
            )
            if embedded is not None and embedded != manifest_content:
                raise BoundaryError(
                    "the manifest used for export differs from the indexed "
                    "open-source-boundary.json"
                )
            materialize(repo, args.output, candidates, manifest)
            print(
                f"exported {len(candidates)} files to {args.output.resolve()}, sha256 {digest}"
            )
            return 0
        print_plan(snapshot, manifest_path, candidates, args.output)
        return 0
    except BoundaryError as exc:
        print(f"open-source boundary error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
