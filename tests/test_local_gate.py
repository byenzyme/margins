#!/usr/bin/env python3
"""scripts/local-gate quick must notice Cargo graph changes on the branch itself."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest
from unittest import mock


REPO_ROOT = Path(__file__).resolve().parent.parent
LOADER = importlib.machinery.SourceFileLoader("local_gate", str(REPO_ROOT / "scripts/local-gate"))
SPEC = importlib.util.spec_from_loader("local_gate", LOADER)
local_gate = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(local_gate)

REGISTRY = 'source = "registry+https://github.com/rust-lang/crates.io-index"'
PUBLIC_LOCK = textwrap.dedent(
    f"""\
    version = 4

    [[package]]
    name = "margins-server"
    version = "0.4.15"
    dependencies = [
     "base64",
     "url",
    ]

    [[package]]
    name = "base64"
    version = "0.22.1"
    {REGISTRY}
    checksum = "aaa"

    [[package]]
    name = "url"
    version = "2.5.4"
    {REGISTRY}
    checksum = "bbb"
    """
)
# The private composition adds the engine, a second base64 version (so its
# dependency entries are version-qualified) and unified third-party deps.
PRIVATE_LOCK = textwrap.dedent(
    f"""\
    version = 4

    [[package]]
    name = "margins-server"
    version = "0.4.15"
    dependencies = [
     "base64 0.22.1",
     "enzyme-core",
     "url",
    ]

    [[package]]
    name = "enzyme-core"
    version = "0.7.2"
    source = "git+https://github.com/byenzyme/enzyme-rust.git?rev=d92f9e5#d92f9e5"
    dependencies = [
     "base64 0.21.7",
    ]

    [[package]]
    name = "base64"
    version = "0.21.7"
    {REGISTRY}
    checksum = "ccc"

    [[package]]
    name = "base64"
    version = "0.22.1"
    {REGISTRY}
    checksum = "aaa"

    [[package]]
    name = "url"
    version = "2.5.4"
    {REGISTRY}
    checksum = "bbb"
    dependencies = [
     "serde",
    ]
    """
)


class LockDriftTests(unittest.TestCase):
    def test_consistent_private_lock_has_no_drift(self) -> None:
        self.assertEqual(local_gate.lock_drift(PUBLIC_LOCK, PRIVATE_LOCK), [])

    def test_committed_locks_are_consistent(self) -> None:
        self.assertEqual(
            local_gate.lock_drift(
                (REPO_ROOT / "Cargo.lock").read_text()
                if os.environ.get("MARGINS_PRIVATE_RECALL_ACTIVE") != "1"
                else git(REPO_ROOT, "show", "HEAD:Cargo.lock"),
                (REPO_ROOT / "Cargo.private-recall.lock").read_text(),
            ),
            [],
        )

    def test_new_dependency_of_a_workspace_member_is_drift(self) -> None:
        # The #11 shape: url already exists privately, but the member's edge is new.
        private = PRIVATE_LOCK.replace(' "url",\n]', "]", 1)
        self.assertEqual(
            local_gate.lock_drift(PUBLIC_LOCK, private),
            ["margins-server 0.4.15 depends on url only in Cargo.lock, not in Cargo.private-recall.lock"],
        )

    def test_missing_package_and_changed_checksum_are_drift(self) -> None:
        public = PUBLIC_LOCK.replace('checksum = "bbb"', 'checksum = "new"') + textwrap.dedent(
            f"""
            [[package]]
            name = "itoa"
            version = "1.0.0"
            {REGISTRY}
            checksum = "ddd"
            """
        )
        self.assertEqual(
            local_gate.lock_drift(public, PRIVATE_LOCK),
            [
                "url 2.5.4 has a different checksum in Cargo.private-recall.lock",
                "itoa 1.0.0 is missing from Cargo.private-recall.lock",
            ],
        )


class PrivateCompositionCheckTests(unittest.TestCase):
    def test_unreachable_private_engine_is_a_clear_skip(self) -> None:
        gate = local_gate.Gate("quick")
        with mock.patch.object(
            local_gate, "private_recall_reachable", return_value="no access to fixture"
        ), mock.patch.object(gate, "run") as run:
            local_gate.check_private_composition_lock(gate, optional=True)
        run.assert_not_called()
        self.assertEqual(
            gate.skipped, ["verify private recall manifest and lock (no access to fixture)"]
        )


def git(root: Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
    return subprocess.run(
        ["git", "-C", str(root), "-c", "user.name=fixture", "-c", "user.email=fixture@invalid", *args],
        env=env, check=True, capture_output=True, text=True,
    ).stdout


class ChangedCargoFilesTests(unittest.TestCase):
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        git(self.root, "init", "-q", "-b", "main")
        for name, text in {
            "Cargo.toml": "[workspace]\n",
            "Cargo.lock": PUBLIC_LOCK,
            "Cargo.private-recall.lock": PRIVATE_LOCK,
            "src/lib.rs": "\n",
            "crates/a/Cargo.toml": "[package]\n",
        }.items():
            (self.root / name).parent.mkdir(parents=True, exist_ok=True)
            (self.root / name).write_text(text)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "base")
        git(self.root, "checkout", "-q", "-b", "work")
        env = {k: v for k, v in os.environ.items() if not k.startswith(("MARGINS_PRIVATE_RECALL_", "GIT_"))}
        patcher = mock.patch.dict(os.environ, env, clear=True)
        patcher.start()
        self.addCleanup(patcher.stop)

    def changed(self) -> set[str]:
        changed, note = local_gate.changed_cargo_files(self.root, "main")
        self.assertIsNone(note)
        return changed

    def test_source_only_branch_reports_nothing(self) -> None:
        (self.root / "src/lib.rs").write_text("// edit\n")
        git(self.root, "commit", "-qam", "source")
        self.assertEqual(self.changed(), set())

    def test_committed_lock_change_is_found_without_path_arguments(self) -> None:
        (self.root / "Cargo.lock").write_text(PUBLIC_LOCK + "\n")
        git(self.root, "commit", "-qam", "lock")
        self.assertEqual(self.changed(), {"Cargo.lock"})

    def test_uncommitted_and_nested_manifest_changes_are_found(self) -> None:
        (self.root / "crates/a/Cargo.toml").write_text("[package]\nname = 'a'\n")
        (self.root / "crates/b").mkdir()
        (self.root / "crates/b/Cargo.toml").write_text("[package]\n")
        (self.root / "Cargo.private-recall.lock").write_text(PRIVATE_LOCK + "\n")
        self.assertEqual(
            self.changed(),
            {"crates/a/Cargo.toml", "crates/b/Cargo.toml", "Cargo.private-recall.lock"},
        )

    def test_with_private_recall_substitution_is_not_a_change(self) -> None:
        backup_dir = tempfile.TemporaryDirectory()  # outside the repo, like mktemp -d
        self.addCleanup(backup_dir.cleanup)
        backup = Path(backup_dir.name)
        for name in ("Cargo.toml", "Cargo.lock"):
            (backup / name).write_bytes((self.root / name).read_bytes())
        (self.root / "Cargo.toml").write_text("[workspace]\n# private\n")
        (self.root / "Cargo.lock").write_text(PRIVATE_LOCK)
        os.environ["MARGINS_PRIVATE_RECALL_ACTIVE"] = "1"
        os.environ["MARGINS_PRIVATE_RECALL_PUBLIC_DIR"] = str(backup)
        self.assertEqual(self.changed(), set())
        (backup / "Cargo.lock").write_text(PUBLIC_LOCK + "\n")
        self.assertEqual(self.changed(), {"Cargo.lock"})

    def test_missing_base_ref_is_reported(self) -> None:
        changed, note = local_gate.changed_cargo_files(self.root, "no-such-ref")
        self.assertEqual(changed, set())
        self.assertIn("no-such-ref", note)


if __name__ == "__main__":
    unittest.main()
