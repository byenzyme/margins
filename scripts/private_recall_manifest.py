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
    source = source.replace(
        'recall = ["dep:reqwest", "dep:tokio"]',
        'recall = ["dep:recall-engine", "dep:reqwest", "dep:tokio"]',
        1,
    ).replace(
        'recall-local-model = ["recall"]',
        'recall-local-model = ["recall", "recall-engine/local-llm"]',
        1,
    ).replace(
        'rusqlite.workspace = true\n',
        f'rusqlite.workspace = true\n{dependency}\n',
        1,
    )
    manifest = tomllib.loads(source)
    assert manifest["dependencies"]["recall-engine"]["optional"] is True
    assert manifest["dependencies"]["recall-engine"]["rev"] == "d92f9e52ffddbe318ed2d6797cde2d821985c61a"
    return source


if __name__ == "__main__":
    if sys.argv[1:] != ["--apply"]:
        raise SystemExit("usage: scripts/private_recall_manifest.py --apply")
    MANIFEST.write_text(compose())
