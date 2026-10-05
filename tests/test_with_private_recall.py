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
SUBSTITUTED = {"Cargo.toml", "Cargo.lock"}
PROBE = 'printf "%s\\n" "${MARGINS_BUILD_DIRTY-unset}"; git status --porcelain'


class WithPrivateRecallDirtyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        # Under an outer with-private-recall (the linux gate), the checkout's
        # manifest is already substituted; use the committed one instead.
        outer_wrapper = os.environ.get("MARGINS_PRIVATE_RECALL_ACTIVE") == "1"
        for relative in COPIED:
            source = REPO_ROOT / relative
            if not source.exists():
                continue
            target = self.repo / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            if outer_wrapper and relative in SUBSTITUTED:
                target.write_bytes(
                    subprocess.run(
                        ["git", "show", f"HEAD:{relative}"],
                        cwd=REPO_ROOT, check=True, capture_output=True,
                    ).stdout
                )
            else:
                shutil.copy2(source, target)
        self.git("init", "-q")
        self.git("add", "-A")
        self.git(
            "-c", "user.name=fixture", "-c", "user.email=fixture@invalid",
            "commit", "-q", "-m", "fixture",
        )

    def git(self, *args: str) -> None:
        subprocess.run(["git", *args], cwd=self.repo, env=self.base_env(), check=True)

    @staticmethod
    def base_env() -> dict[str, str]:
        # Hermetic git: no caller repo, hooks, or user/system configuration.
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("GIT_") and key != "MARGINS_BUILD_DIRTY"
        }
        environment.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
        return environment

    def run_wrapper(self, **env: str) -> tuple[str, str]:
        environment = self.base_env()
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
            cwd=self.repo, env=self.base_env(), check=True, capture_output=True, text=True,
        ).stdout


if __name__ == "__main__":
    unittest.main()
