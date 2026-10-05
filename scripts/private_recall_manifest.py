#!/usr/bin/env python3
"""Compose the official Cargo manifest with the pinned private recall source."""

from pathlib import Path
import os
import sys
import tomllib


ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "Cargo.toml"
DEPENDENCY = ROOT / "scripts/private_recall_dependency.toml"
ENGINE_GIT = "https://github.com/byenzyme/enzyme-rust.git"
SPEC_DECLARER = ROOT / "crates/public/margins-workflows/Cargo.toml"


def enzyme_spec_patch() -> str:
    """Point the engine's own enzyme-spec at the copy Margins declares.

    enzyme-core depends on enzyme-spec by path inside enzyme-rust, so Cargo
    would otherwise link a second enzyme-spec (source: the enzyme-rust git
    rev) whose `Program` type cannot unify with the one margins-workflows
    parses. margins-workflows' declaration is the single authority; the
    private composition patches the engine's edge to that same source.
    """
    declared = tomllib.loads(SPEC_DECLARER.read_text())["dependencies"]["enzyme-spec"]
    if "path" in declared:
        absolute = (SPEC_DECLARER.parent / declared["path"]).resolve()
        target = f'{{ path = "{os.path.relpath(absolute, ROOT)}" }}'
    else:
        fields = ", ".join(
            f'{key} = "{declared[key]}"' for key in ("git", "tag", "rev", "branch") if key in declared
        )
        target = f"{{ {fields} }}"
    return f'\n[patch."{ENGINE_GIT}"]\nenzyme-spec = {target}\n'


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
    source += enzyme_spec_patch()
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
    assert manifest["dependencies"]["recall-engine"]["rev"] == "2a1f175312b72f8d4ad1b1a1abec76b8d1e7f669"
    assert "enzyme-spec" in manifest["patch"][ENGINE_GIT]
    return source


if __name__ == "__main__":
    if sys.argv[1:] != ["--apply"]:
        raise SystemExit("usage: scripts/private_recall_manifest.py --apply")
    MANIFEST.write_text(compose())
