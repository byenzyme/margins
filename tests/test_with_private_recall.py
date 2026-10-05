#!/usr/bin/env python3
"""scripts/with-private-recall must not report its own manifest edits as dirty."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parent.parent
COPIED = (
    "scripts/with-private-recall",
    "scripts/private_recall_manifest.py",
    "scripts/private_recall_dependency.toml",
    "Cargo.toml",
    "Cargo.lock",
    "Cargo.private-recall.lock",
)
PROBE = 'printf "%s\\n" "${MARGINS_BUILD_DIRTY-unset}"; git status --porcelain'


class WithPrivateRecallDirtyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        for relative in COPIED:
            source = REPO_ROOT / relative
            if source.exists():
                target = self.repo / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, target)
        self.git("init", "-q")
        self.git("add", "-A")
        self.git(
            "-c", "user.name=fixture", "-c", "user.email=fixture@invalid",
            "commit", "-q", "-m", "fixture",
        )

    def git(self, *args: str) -> None:
        subprocess.run(["git", *args], cwd=self.repo, check=True)

    def run_wrapper(self, **env: str) -> tuple[str, str]:
        environment = {k: v for k, v in os.environ.items() if k != "MARGINS_BUILD_DIRTY"}
        environment.update(env)
        result = subprocess.run(
            [str(self.repo / "scripts/with-private-recall"), "bash", "-c", PROBE],
            cwd=self.repo,
            env=environment,
            check=True,
            capture_output=True,
            text=True,
        )
        dirty, _, status = result.stdout.partition("\n")
        return dirty, status

    def test_clean_checkout_is_clean_despite_manifest_substitution(self) -> None:
        dirty, status = self.run_wrapper()
        self.assertIn("Cargo.toml", status, "wrapper should have rewritten the manifest")
        self.assertEqual(dirty, "false")
        self.assertEqual(self.git_status(), "", "wrapper must restore the manifest")

    def test_real_uncommitted_changes_stay_dirty(self) -> None:
        (self.repo / "untracked.rs").write_text("// edit\n")
        dirty, _ = self.run_wrapper()
        self.assertEqual(dirty, "true")

    def test_real_manifest_edit_stays_dirty(self) -> None:
        manifest = self.repo / "Cargo.toml"
        manifest.write_text(manifest.read_text() + "\n# local edit\n")
        dirty, _ = self.run_wrapper()
        self.assertEqual(dirty, "true")

    def test_explicit_value_is_preserved(self) -> None:
        dirty, _ = self.run_wrapper(MARGINS_BUILD_DIRTY="true")
        self.assertEqual(dirty, "true")

    def git_status(self) -> str:
        return subprocess.run(
            ["git", "status", "--porcelain"],
            cwd=self.repo, check=True, capture_output=True, text=True,
        ).stdout


if __name__ == "__main__":
    unittest.main()
