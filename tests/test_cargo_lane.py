#!/usr/bin/env python3

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "cargo-lane"
POLICY_ENV = "MARGINS_CARGO_LANE_SCCACHE"
CMAKE_LAUNCHERS = (
    "CMAKE_C_COMPILER_LAUNCHER",
    "CMAKE_CXX_COMPILER_LAUNCHER",
)


class CargoLaneSccacheTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.fake_bin = self.root / "bin"
        self.fake_bin.mkdir()
        git = shutil.which("git")
        self.assertIsNotNone(git)
        os.symlink(git, self.fake_bin / "git")
        du = shutil.which("du")
        self.assertIsNotNone(du)
        os.symlink(du, self.fake_bin / "du")
        subprocess.run(
            [git, "init"],
            cwd=self.repo,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    def env(self, **overrides: str) -> dict[str, str]:
        env = {
            "PATH": str(self.fake_bin),
            "HOME": str(self.root / "home"),
            "BB_THREAD_ID": "test-thread",
            "BB_ENVIRONMENT_ID": "test-env",
        }
        env.update(overrides)
        return env

    def write_fake_sccache(self, body: str | None = None) -> Path:
        sccache = self.fake_bin / "sccache"
        sccache.write_text(
            body
            or textwrap.dedent(
                """\
                #!/bin/sh
                if [ "$1" = "--show-stats" ]; then
                  echo "fake sccache stats"
                  exit 0
                fi
                echo "unexpected sccache invocation: $*" >&2
                exit 9
                """
            ),
            encoding="utf-8",
        )
        sccache.chmod(0o755)
        return sccache

    def run_lane(
        self, args: list[str], env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            cwd=self.repo,
            env=env or self.env(),
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=30,
        )

    def env_command(self) -> list[str]:
        code = (
            "import json, os; "
            "print(json.dumps({"
            "'RUSTC_WRAPPER': os.environ.get('RUSTC_WRAPPER'), "
            "'CMAKE_C_COMPILER_LAUNCHER': os.environ.get('CMAKE_C_COMPILER_LAUNCHER'), "
            "'CMAKE_CXX_COMPILER_LAUNCHER': os.environ.get('CMAKE_CXX_COMPILER_LAUNCHER'), "
            "'CARGO_TARGET_DIR': os.environ.get('CARGO_TARGET_DIR'), "
            "'CARGO_BUILD_BUILD_DIR': os.environ.get('CARGO_BUILD_BUILD_DIR'), "
            "'CARGO_INCREMENTAL': os.environ.get('CARGO_INCREMENTAL'), "
            "'MARGINS_CARGO_LANE_MODE': os.environ.get('MARGINS_CARGO_LANE_MODE'), "
            "'MARGINS_CARGO_LANE_TARGET_DIR': os.environ.get('MARGINS_CARGO_LANE_TARGET_DIR')"
            "}, sort_keys=True))"
        )
        return [sys.executable, "-c", code]

    def run_env_command(
        self, mode: str, env: dict[str, str] | None = None
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, str | None]]:
        completed = self.run_lane([mode, "--", *self.env_command()], env=env)
        self.assertEqual(
            completed.returncode,
            0,
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}",
        )
        payload = json.loads(completed.stdout.strip().splitlines()[-1])
        return completed, payload

    def test_auto_uses_sccache_when_available(self) -> None:
        sccache = self.write_fake_sccache()

        _, payload = self.run_env_command("shared", self.env())

        self.assertEqual(payload["RUSTC_WRAPPER"], str(sccache))
        for variable in CMAKE_LAUNCHERS:
            self.assertEqual(payload[variable], str(sccache))

    def test_off_does_not_set_wrapper_even_when_sccache_is_available(self) -> None:
        self.write_fake_sccache()

        _, payload = self.run_env_command("shared", self.env(**{POLICY_ENV: "off"}))

        self.assertIsNone(payload["RUSTC_WRAPPER"])
        for variable in CMAKE_LAUNCHERS:
            self.assertIsNone(payload[variable])

    def test_required_fails_before_command_when_sccache_is_unavailable(self) -> None:
        completed = self.run_lane(
            ["shared", "--", sys.executable, "-c", "print('should not run')"],
            self.env(**{POLICY_ENV: "required"}),
        )

        self.assertEqual(completed.returncode, 2)
        self.assertEqual(completed.stdout, "")
        self.assertIn("sccache required", completed.stderr)
        self.assertIn("no 'sccache' executable was found on PATH", completed.stderr)
        self.assertFalse((self.root / ".margins-cargo-build-owner.json").exists())

    def test_existing_rustc_wrapper_is_preserved(self) -> None:
        self.write_fake_sccache()
        wrapper = str(self.root / "custom-wrapper")

        _, payload = self.run_env_command("shared", self.env(RUSTC_WRAPPER=wrapper))

        self.assertEqual(payload["RUSTC_WRAPPER"], wrapper)
        for variable in CMAKE_LAUNCHERS:
            self.assertIsNone(payload[variable])

    def test_existing_cmake_launcher_is_preserved(self) -> None:
        sccache = self.write_fake_sccache()
        custom = str(self.root / "custom-c-launcher")

        _, payload = self.run_env_command(
            "shared", self.env(CMAKE_C_COMPILER_LAUNCHER=custom)
        )

        self.assertEqual(payload["RUSTC_WRAPPER"], str(sccache))
        self.assertEqual(payload["CMAKE_C_COMPILER_LAUNCHER"], custom)
        self.assertEqual(payload["CMAKE_CXX_COMPILER_LAUNCHER"], str(sccache))

    def test_external_sccache_wrapper_extends_to_cmake(self) -> None:
        sccache = self.write_fake_sccache()

        _, payload = self.run_env_command(
            "shared", self.env(RUSTC_WRAPPER=str(sccache))
        )

        for variable in CMAKE_LAUNCHERS:
            self.assertEqual(payload[variable], str(sccache))

    def test_shared_and_isolated_receive_same_sccache_policy(self) -> None:
        sccache = self.write_fake_sccache()

        _, shared_payload = self.run_env_command("shared", self.env())
        _, isolated_payload = self.run_env_command("isolated", self.env())

        self.assertEqual(shared_payload["RUSTC_WRAPPER"], str(sccache))
        self.assertEqual(isolated_payload["RUSTC_WRAPPER"], str(sccache))
        for variable in CMAKE_LAUNCHERS:
            self.assertEqual(shared_payload[variable], str(sccache))
            self.assertEqual(isolated_payload[variable], str(sccache))
        self.assertNotEqual(
            shared_payload["CARGO_TARGET_DIR"], isolated_payload["CARGO_TARGET_DIR"]
        )
        self.assertEqual(
            shared_payload["CARGO_BUILD_BUILD_DIR"],
            str(self.root / "margins-cargo-build"),
        )
        self.assertIsNone(isolated_payload["CARGO_BUILD_BUILD_DIR"])
        self.assertEqual(isolated_payload["CARGO_INCREMENTAL"], "0")
        self.assertEqual(shared_payload["MARGINS_CARGO_LANE_MODE"], "shared")
        self.assertEqual(isolated_payload["MARGINS_CARGO_LANE_MODE"], "isolated")
        self.assertFalse(Path(isolated_payload["CARGO_TARGET_DIR"]).exists())

    def test_disposable_lane_is_fixed_path_and_cleans_after_each_run(self) -> None:
        _, first = self.run_env_command("disposable", self.env())
        _, second = self.run_env_command("disposable", self.env())

        expected = self.root / ".margins-cargo-isolated" / "slot-1-target"
        self.assertEqual(first["CARGO_TARGET_DIR"], str(expected))
        self.assertEqual(second["CARGO_TARGET_DIR"], str(expected))
        self.assertEqual(first["CARGO_INCREMENTAL"], "0")
        self.assertEqual(first["MARGINS_CARGO_LANE_MODE"], "disposable")
        self.assertFalse(expected.exists())

    def test_disposable_recovers_abandoned_fixed_slot(self) -> None:
        target = self.root / ".margins-cargo-isolated" / "slot-1-target"
        target.mkdir(parents=True)
        (target / "orphan").write_text("stale")

        completed, payload = self.run_env_command("disposable", self.env())

        self.assertEqual(payload["CARGO_TARGET_DIR"], str(target))
        self.assertIn("recovered abandoned scratch target", completed.stderr)
        self.assertFalse(target.exists())

    def test_next_shared_build_recovers_abandoned_scratch_after_restart(self) -> None:
        target = self.root / ".margins-cargo-isolated" / "slot-1-target"
        target.mkdir(parents=True)
        (target / "orphan").write_text("stale")

        self.run_env_command("shared", self.env())

        self.assertFalse(target.exists())

    def test_shared_rejects_high_churn_test_commands(self) -> None:
        for cargo_args in (["test", "--workspace"], ["+stable", "test"]):
            with self.subTest(cargo_args=cargo_args):
                completed = self.run_lane(
                    ["shared", "--", "cargo", *cargo_args], self.env()
                )
                self.assertEqual(completed.returncode, 2)
                self.assertIn("belong in the disposable lane", completed.stderr)

    def test_prune_removes_only_legacy_intermediate_layout(self) -> None:
        target = self.root / "margins-cargo-target"
        for name in ("deps", "incremental", "build", ".fingerprint"):
            path = target / "debug" / name
            path.mkdir(parents=True)
            (path / "artifact").write_text("x")
        examples = target / "debug" / "examples"
        examples.mkdir(parents=True)
        (examples / "old-example").write_text("x")
        final_binary = target / "debug" / "margins"
        final_binary.write_text("keep")
        abandoned = self.root / ".margins-cargo-isolated" / "slot-1-target"
        abandoned.mkdir(parents=True)
        (abandoned / "artifact").write_text("x")

        completed = self.run_lane(["prune"], self.env())

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertTrue(final_binary.exists())
        self.assertFalse((target / "debug" / "deps").exists())
        self.assertFalse(examples.exists())
        self.assertFalse(abandoned.exists())

    def test_nested_shared_lane_reuses_lock_and_environment(self) -> None:
        sccache = self.write_fake_sccache()
        nested = [sys.executable, str(SCRIPT), "shared", "--", *self.env_command()]

        completed = self.run_lane(["shared", "--", *nested], self.env())

        self.assertEqual(completed.returncode, 0, completed.stderr)
        payload = json.loads(completed.stdout.strip().splitlines()[-1])
        self.assertEqual(payload["RUSTC_WRAPPER"], str(sccache))
        self.assertEqual(payload["MARGINS_CARGO_LANE_MODE"], "shared")
        self.assertIn("reusing active shared lane", completed.stderr)

    def test_status_reports_sccache_states(self) -> None:
        completed = self.run_lane(["status"], self.env())
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn(
            "sccache policy auto: unavailable; builds unchanged", completed.stdout
        )

        sccache = self.write_fake_sccache()
        completed = self.run_lane(["status"], self.env())
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("sccache policy auto: enabled", completed.stdout)
        self.assertIn(str(sccache), completed.stdout)

        completed = self.run_lane(["status"], self.env(**{POLICY_ENV: "off"}))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("available", completed.stdout)
        self.assertIn("disabled", completed.stdout)

        completed = self.run_lane(["status"], self.env(RUSTC_WRAPPER="/tmp/wrapper"))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("externally configured wrapper", completed.stdout)
        self.assertIn("will be preserved", completed.stdout)

    def test_sccache_stats_runs_only_on_explicit_command(self) -> None:
        self.write_fake_sccache()

        completed = self.run_lane(["sccache-stats"], self.env())

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stdout.strip(), "fake sccache stats")


class CargoLaneEntrypointTests(unittest.TestCase):
    def test_native_and_tauri_npm_entrypoints_use_shared_lane(self) -> None:
        package = json.loads((REPO_ROOT / "desktop" / "package.json").read_text())
        scripts = package["scripts"]
        expected = {
            "headless:server:check",
            "snapshot:state",
            "dev:app",
            "dev:app:tools",
            "app:reinstall",
            "app:reinstall:devtools",
            "app:reinstall:first-run",
            "bundle:mac",
            "bundle:mac:signed",
            "bundle:mac:universal",
            "tauri:build:updates",
            "tauri:build:windows",
            "tauri:dev",
            "tauri:dev:tools",
            "tauri:dev:global-auth",
        }

        for name in expected:
            with self.subTest(script=name):
                self.assertIn("../scripts/cargo-lane shared --", scripts[name])

        disposable = {
            "test:native-coreml-rolling",
            "ux:e2e:distill-fixture",
            "distill:perf",
        }
        for name in disposable:
            with self.subTest(script=name):
                self.assertIn("../scripts/cargo-lane disposable --", scripts[name])

    def test_updater_no_longer_uses_legacy_aside_target(self) -> None:
        updater = (
            REPO_ROOT / "desktop" / "scripts" / "build-updater-release.sh"
        ).read_text()

        self.assertNotIn("aside-cargo-target", updater)
        self.assertIn("margins-cargo-target", updater)


if __name__ == "__main__":
    unittest.main()
