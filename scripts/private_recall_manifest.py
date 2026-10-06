#!/usr/bin/env python3
"""Compose the official Cargo manifest with the pinned private recall source."""

from pathlib import Path
import sys
import tomllib


ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "Cargo.toml"
DEPENDENCY = ROOT / "scripts/private_recall_dependency.toml"


def compose() -> str:
    source = MANIFEST.read_text()
    fragment = DEPENDENCY.read_text()
    dependency = next(line for line in fragment.splitlines() if line.startswith("recall-engine ="))

    def replace_once(old: str, new: str, description: str) -> None:
        nonlocal source
        count = source.count(old)
        if count != 1:
            raise ValueError(f"expected exactly one {description}, found {count}")
        source = source.replace(old, new, 1)

    replace_once(
        'recall = ["dep:reqwest", "dep:tokio"]',
        'recall = ["dep:recall-engine", "dep:reqwest", "dep:tokio"]',
        "public recall feature line",
    )
    replace_once(
        'recall-local-model = ["recall"]',
        'recall-local-model = ["recall", "recall-engine/local-llm"]',
        "public recall-local-model feature line",
    )
    replace_once(
        'rusqlite.workspace = true\n',
        f'rusqlite.workspace = true\n{dependency}\n',
        "recall dependency insertion point",
    )
    if "[patch." in source:
        raise ValueError("public manifest unexpectedly declares a [patch] section")
    manifest = tomllib.loads(source)
    assert manifest["features"]["recall"] == [
        "dep:recall-engine",
        "dep:reqwest",
        "dep:tokio",
    ]
    assert manifest["features"]["recall-local-model"] == [
        "recall",
        "recall-engine/local-llm",
    ]
    assert manifest["dependencies"]["recall-engine"]["optional"] is True
    # The linked engine and the enzyme CLI that Margins runs are one revision.
    pinned_cli = (ROOT / "scripts/enzyme-cli.pin").read_text().strip()
    assert manifest["dependencies"]["recall-engine"]["rev"] == pinned_cli
    return source


if __name__ == "__main__":
    if sys.argv[1:] != ["--apply"]:
        raise SystemExit("usage: scripts/private_recall_manifest.py --apply")
    MANIFEST.write_text(compose())
