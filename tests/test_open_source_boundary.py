#!/usr/bin/env python3

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
import unittest


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "open_source_boundary.py"


class RepositoryPolicyTests(unittest.TestCase):
    @unittest.skipUnless(
        (REPO_ROOT / "public-repository" / "Cargo.toml").is_file(),
        "full mixed-source repository is not present",
    )
    def test_repository_license_signals_are_consistent(self) -> None:
        cargo = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
        readme = (REPO_ROOT / "README.md").read_text(encoding="utf-8")
        license_text = (REPO_ROOT / "LICENSE").read_text(encoding="utf-8")
        open_source = (REPO_ROOT / "OPEN_SOURCE.md").read_text(encoding="utf-8")
        declared = re.search(r'(?m)^license\s*=\s*"([^"]+)"', cargo)

        self.assertIsNotNone(declared)
        self.assertEqual(declared.group(1), "Apache-2.0")
        self.assertTrue(license_text.lstrip().startswith("Apache License"))
        self.assertIn("[Apache License 2.0](LICENSE)", readme)
        self.assertNotIn("[GPLv3](LICENSE)", readme)
        self.assertIn(
            "do not themselves grant a license", " ".join(open_source.split())
        )

    def test_candidate_manifest_is_a_literal_exact_allowlist(self) -> None:
        manifest = json.loads(
            (REPO_ROOT / "open-source-boundary.json").read_text(encoding="utf-8")
        )
        self.assertIs(manifest.get("exact_allowlist"), True)
        for scope in manifest["scopes"]:
            self.assertEqual(scope["include"], scope["required_files"], scope["name"])
            self.assertEqual(scope["minimum_files"], len(scope["required_files"]))
            self.assertFalse(
                any("*" in path or "?" in path for path in scope["include"]),
                scope["name"],
            )

    @unittest.skipUnless(
        (REPO_ROOT / "public-repository" / "Cargo.toml").is_file(),
        "repository-shell sources exist only in the mixed-source repository",
    )
    def test_repository_shell_is_exact_and_maps_to_export_root(self) -> None:
        manifest = json.loads(
            (REPO_ROOT / "open-source-boundary.json").read_text(encoding="utf-8")
        )
        scope = next(
            scope for scope in manifest["scopes"] if scope["name"] == "repository-shell"
        )
        expected = {
            "public-repository/Cargo.toml": "Cargo.toml",
            "public-repository/Cargo.lock": "Cargo.lock",
            "public-repository/README.md": "README.md",
            "public-repository/CONTRIBUTING.md": "CONTRIBUTING.md",
            "public-repository/ARCHITECTURE.md": "docs/architecture.md",
            "public-repository/release.yml": ".github/workflows/release.yml",
            "public-repository/homebrew-formula.rb.template": ".github/homebrew/margins.rb.template",
        }
        self.assertEqual(manifest["export_paths"], expected)
        self.assertEqual(set(scope["include"]), set(expected))
        self.assertEqual(set(scope["required_files"]), set(expected))
        self.assertEqual(scope["minimum_files"], len(expected))

        workspace = tomllib.loads(
            (REPO_ROOT / "public-repository/Cargo.toml").read_text(encoding="utf-8")
        )["workspace"]
        expected_members = {
            f"crates/public/{name}"
            for name in (
                "margins-meeting-protocol",
                "margins-core",
                "margins-media",
                "margins-meeting-runtime",
                "margins-store",
                "margins-workflows",
                "margins-cli",
            )
        }
        self.assertEqual(set(workspace["members"]), expected_members)
        self.assertEqual(set(workspace["default-members"]), expected_members)
        self.assertEqual(workspace["resolver"], "2")

        readme = (REPO_ROOT / "public-repository/README.md").read_text(
            encoding="utf-8"
        )
        normalized_readme = " ".join(readme.split())
        for required in (
            "remote meeting recording and transcription systems",
            "customization platform",
            "Trust, security, and privacy boundaries",
            "not a claim that any crate has been published",
            "do not grant trademark rights",
        ):
            self.assertIn(required, normalized_readme)
        self.assertNotIn("cpal::", readme)
        self.assertNotIn("CoreAudio", readme)

        contributing = (
            REPO_ROOT / "public-repository/CONTRIBUTING.md"
        ).read_text(encoding="utf-8")
        normalized_contributing = " ".join(contributing.split())
        for required in (
            "Crate publication order",
            "not asserted to be reserved or available on crates.io",
        ):
            self.assertIn(required, normalized_contributing)

        workflow = (REPO_ROOT / ".github/workflows/open-source-boundary.yml").read_text(
            encoding="utf-8"
        )
        for command in (
            "cargo build --workspace --all-targets --no-default-features --locked --offline",
            "cargo test --workspace --all-targets --no-default-features --locked --offline",
        ):
            self.assertIn(command, " ".join(workflow.split()))

    def test_public_meeting_runtime_scope_is_exact_and_standalone_tested(self) -> None:
        manifest = json.loads(
            (REPO_ROOT / "open-source-boundary.json").read_text(encoding="utf-8")
        )
        runtime_scope = next(
            scope
            for scope in manifest["scopes"]
            if scope["name"] == "meeting-runtime-crate"
        )
        required = {
            "crates/public/margins-meeting-runtime/Cargo.toml",
            "crates/public/margins-meeting-runtime/LICENSE",
            "crates/public/margins-meeting-runtime/README.md",
            "crates/public/margins-meeting-runtime/src/lib.rs",
            "crates/public/margins-meeting-runtime/tests/concurrent_races.rs",
            "crates/public/margins-meeting-runtime/tests/runtime_state_machine.rs",
        }
        self.assertEqual(set(runtime_scope["required_files"]), required)
        self.assertEqual(runtime_scope["minimum_files"], len(required))

        workflow = (
            REPO_ROOT / ".github/workflows/open-source-boundary.yml"
        ).read_text(encoding="utf-8")
        self.assertIn('cp -R . "$build_export"', workflow)
        for crate in (
            "margins-meeting-protocol",
            "margins-core",
            "margins-media",
            "margins-meeting-runtime",
            "margins-store",
            "margins-workflows",
            "margins-cli",
        ):
            self.assertIn(crate, workflow)
        self.assertIn('cargo test --manifest-path "$manifest"', workflow)
        self.assertIn("cargo package --manifest-path", workflow)
        self.assertIn("RUSTDOCFLAGS='-D warnings' cargo doc", workflow)

    def test_public_store_scope_is_exact(self) -> None:
        manifest = json.loads(
            (REPO_ROOT / "open-source-boundary.json").read_text(encoding="utf-8")
        )
        scope = next(
            scope for scope in manifest["scopes"] if scope["name"] == "margins-store-crate"
        )
        expected = {
            "crates/public/margins-store/Cargo.toml",
            "crates/public/margins-store/LICENSE",
            "crates/public/margins-store/README.md",
            "crates/public/margins-store/src/index.rs",
            "crates/public/margins-store/src/legacy.rs",
            "crates/public/margins-store/src/lib.rs",
            "crates/public/margins-store/src/sqlite.rs",
            "crates/public/margins-store/tests/index_query.rs",
            "crates/public/margins-store/tests/legacy_compatibility.rs",
            "crates/public/margins-store/tests/public_graph.rs",
            "crates/public/margins-store/tests/repository_contract.rs",
        }
        self.assertEqual(set(scope["include"]), expected)
        self.assertEqual(set(scope["required_files"]), expected)
        self.assertEqual(scope["minimum_files"], len(expected))

    def test_public_workflows_scope_is_exact_and_standalone_tested(self) -> None:
        manifest = json.loads(
            (REPO_ROOT / "open-source-boundary.json").read_text(encoding="utf-8")
        )
        scope = next(
            scope
            for scope in manifest["scopes"]
            if scope["name"] == "margins-workflows-crate"
        )
        crate_root = REPO_ROOT / "crates/public/margins-workflows"
        expected = {
            path.relative_to(REPO_ROOT).as_posix()
            for path in crate_root.rglob("*")
            if path.is_file()
        }
        self.assertEqual(set(scope["include"]), expected)
        self.assertEqual(set(scope["required_files"]), expected)
        self.assertEqual(scope["minimum_files"], len(expected))

        workflow = (REPO_ROOT / ".github/workflows/open-source-boundary.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("margins-workflows", workflow)
        self.assertIn('forbidden dependency in $crate no-default graph', workflow)

    def test_public_cargo_graph_has_no_private_edges(self) -> None:
        public_root = (REPO_ROOT / "crates" / "public").resolve()
        manifests = sorted(public_root.glob("*/Cargo.toml"))
        self.assertTrue(manifests)
        forbidden = {
            "margins",
            "margins-capture-native",
            "margins-desktop",
            "tauri",
            "cpal",
            "cidre",
            "windows",
            "pi_agent_rust",
        }
        allowed_first_party = {
            "margins-meeting-protocol": set(),
            "margins-core": {"margins-meeting-protocol"},
            "margins-media": {"margins-core"},
            "margins-meeting-runtime": {"margins-meeting-protocol"},
            "margins-store": {"margins-core"},
            "margins-workflows": {
                "margins-core",
                "margins-media",
                "margins-store",
            },
            "margins-cli": {
                "margins-core",
                "margins-media",
                "margins-store",
                "margins-workflows",
            },
        }

        for manifest_path in manifests:
            data = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
            package = data["package"]["name"]
            self.assertEqual(data["package"]["license"], "Apache-2.0")
            self.assertEqual(
                (manifest_path.parent / "LICENSE").read_bytes(),
                (REPO_ROOT / "LICENSE").read_bytes(),
                f"{package} package license differs from repository license",
            )
            self.assertTrue((manifest_path.parent / data["package"]["readme"]).is_file())
            observed_first_party = set()
            tables = [data]
            tables.extend(data.get("target", {}).values())
            for table in tables:
                for section in ("dependencies", "dev-dependencies", "build-dependencies"):
                    for alias, value in table.get(section, {}).items():
                        details = value if isinstance(value, dict) else {}
                        dependency = details.get("package", alias)
                        self.assertNotIn(dependency, forbidden, f"{package} -> {dependency}")
                        self.assertFalse(
                            dependency.startswith("tauri-"), f"{package} -> {dependency}"
                        )
                        self.assertNotIn("git", details, f"{package} has git dependency")
                        if "path" in details:
                            resolved = (manifest_path.parent / details["path"]).resolve()
                            self.assertTrue(
                                resolved.is_relative_to(public_root),
                                f"{package} path escapes public root: {resolved}",
                            )
                            observed_first_party.add(dependency)
            self.assertEqual(observed_first_party, allowed_first_party[package])

    def test_public_rust_includes_cannot_escape_the_public_tree(self) -> None:
        public_root = (REPO_ROOT / "crates" / "public").resolve()
        include_literal = re.compile(
            r'\binclude(?:_str|_bytes)?!\s*\(\s*"([^"\\]+)"'
        )
        for source in sorted(public_root.rglob("*.rs")):
            text = source.read_text(encoding="utf-8")
            for relative in include_literal.findall(text):
                resolved = (source.parent / relative).resolve()
                self.assertTrue(
                    resolved.is_relative_to(public_root),
                    f"{source.relative_to(REPO_ROOT)} includes outside public tree: {relative}",
                )
                self.assertTrue(
                    resolved.is_file(),
                    f"{source.relative_to(REPO_ROOT)} includes missing file: {relative}",
                )

    def test_public_cli_scope_is_exact_and_standalone_tested(self) -> None:
        manifest = json.loads(
            (REPO_ROOT / "open-source-boundary.json").read_text(encoding="utf-8")
        )
        scope = next(
            scope for scope in manifest["scopes"] if scope["name"] == "margins-cli-crate"
        )
        crate_root = REPO_ROOT / "crates/public/margins-cli"
        expected = {
            path.relative_to(REPO_ROOT).as_posix()
            for path in crate_root.rglob("*")
            if path.is_file()
        }
        self.assertEqual(set(scope["include"]), expected)
        self.assertEqual(set(scope["required_files"]), expected)
        self.assertEqual(scope["minimum_files"], len(expected))
        self.assertNotIn("skills/margins/scripts/margins.py", scope["include"])

        workflow = (REPO_ROOT / ".github/workflows/open-source-boundary.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("margins-cli", workflow)
        self.assertIn('forbidden dependency in $crate no-default graph', workflow)

        root_main_path = REPO_ROOT / "src/main.rs"
        if root_main_path.is_file():
            root_main = root_main_path.read_text(encoding="utf-8")
            self.assertNotIn("derive(Parser)", root_main)
            self.assertNotIn("enum Command", root_main)
            self.assertIn("margins_cli::main_entry_from_env", root_main)

    def test_margins_skill_uses_the_standalone_rust_cli(self) -> None:
        skill = (REPO_ROOT / "skills/margins/SKILL.md").read_text(encoding="utf-8")
        for command in (
            "margins recent",
            "margins transcript",
            "margins artifacts",
            "margins process",
            "margins transcribe",
        ):
            self.assertIn(command, skill)
        self.assertIn("crates/public/margins-cli", skill)
        self.assertIn("do not inspect or modify `.margins/sessions.sqlite`", skill)
        for legacy in ("margins.py", "python3", "ffmpeg", "ffprobe"):
            self.assertNotIn(legacy, skill)
        self.assertFalse((REPO_ROOT / "skills/margins/scripts/margins.py").exists())
        self.assertFalse((REPO_ROOT / "skills/margins/scripts/test_margins.py").exists())

    def test_public_media_scope_is_exact_and_standalone_tested(self) -> None:
        manifest = json.loads(
            (REPO_ROOT / "open-source-boundary.json").read_text(encoding="utf-8")
        )
        media_scope = next(
            scope
            for scope in manifest["scopes"]
            if scope["name"] == "margins-media-crate"
        )
        required = {
            "crates/public/margins-media/Cargo.toml",
            "crates/public/margins-media/LICENSE",
            "crates/public/margins-media/README.md",
            "crates/public/margins-media/src/audio.rs",
            "crates/public/margins-media/src/diarization.rs",
            "crates/public/margins-media/src/info.rs",
            "crates/public/margins-media/src/lib.rs",
            "crates/public/margins-media/src/providers/coreml.rs",
            "crates/public/margins-media/src/providers/mod.rs",
            "crates/public/margins-media/src/providers/parakeet.rs",
            "crates/public/margins-media/src/providers/polyvoice.rs",
            "crates/public/margins-media/src/timeline.rs",
            "crates/public/margins-media/src/transcript.rs",
            "crates/public/margins-media/tests/fixtures/audio_golden.json",
            "crates/public/margins-media/tests/fixtures/transcript_golden.json",
            "crates/public/margins-media/tests/media_fixtures.rs",
            "crates/public/margins-media/tests/model_adapters.rs",
            "crates/public/margins-media/tests/public_graph.rs",
        }
        self.assertEqual(set(media_scope["required_files"]), required)
        self.assertEqual(set(media_scope["include"]), required)
        self.assertEqual(media_scope["minimum_files"], len(required))

        workflow = (
            REPO_ROOT / ".github/workflows/open-source-boundary.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("margins-media", workflow)
        self.assertIn("cargo tree", workflow)
        self.assertIn("parakeet-onnx-dynamic", workflow)
        self.assertIn("polyvoice-coreml", workflow)
        self.assertIn("coreml-asr", workflow)
        self.assertIn('cargo check --manifest-path "$media_manifest" --all-features', workflow)


class BoundaryToolTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name) / "repo"
        self.root.mkdir()
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write(self, relative: str, content: str) -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    def manifest(
        self,
        *,
        include=None,
        deny=None,
        imports=None,
        secrets=None,
        required=None,
        minimum=1,
        exact=False,
    ) -> None:
        data = {
            "schema_version": 1,
            "project": "fixture",
            "artifact_status": "test",
            "exact_allowlist": exact,
            "scopes": [
                {
                    "name": "public",
                    "description": "fixture",
                    "include": include
                    if include is not None
                    else ["public/**", "open-source-boundary.json"],
                    "required_files": required or [],
                    "minimum_files": minimum,
                }
            ],
            "deny_paths": deny
            if deny is not None
            else ["desktop/**", "**/.env", "**/*.key"],
            "import_scan_extensions": [".py", ".rs"],
            "forbidden_import_regexes": imports
            if imports is not None
            else [r"(?m)^\s*(?:from|import)\s+private_runtime(?:\.|\s|$)"],
            "secret_content_regexes": secrets
            if secrets is not None
            else [r"AKIA[0-9A-Z]{16}"],
        }
        self.write("open-source-boundary.json", json.dumps(data, indent=2) + "\n")

    def track(self) -> None:
        subprocess.run(["git", "-C", str(self.root), "add", "."], check=True)

    def run_tool(
        self,
        *args: str,
        manifest_path: Path | None = None,
        umask: int | None = None,
        environment: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        command = [sys.executable, str(SCRIPT), "--repo", str(self.root)]
        if manifest_path is not None:
            command.extend(["--manifest", str(manifest_path)])
        command.extend(args)
        return subprocess.run(
            command,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            preexec_fn=(lambda: os.umask(umask)) if umask is not None else None,
            env=environment,
        )

    def test_default_and_output_without_execute_are_dry_runs(self) -> None:
        self.write("public/core.rs", "pub fn answer() -> u8 { 42 }\n")
        self.manifest()
        self.track()
        output = Path(self.temporary.name) / "export"

        default = self.run_tool()
        requested = self.run_tool("--output", str(output))

        self.assertEqual(default.returncode, 0, default.stderr)
        self.assertEqual(requested.returncode, 0, requested.stderr)
        self.assertIn("DRY RUN: no files written", default.stdout)
        self.assertIn("requested output:", requested.stdout)
        self.assertFalse(output.exists())

    def test_untracked_file_is_never_planned_or_exported(self) -> None:
        self.write("public/core.rs", "pub fn safe() {}\n")
        self.manifest()
        self.track()
        self.write("public/local-secret.txt", "untracked user material\n")
        output = Path(self.temporary.name) / "export"

        result = self.run_tool("--output", str(output), "--execute")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((output / "public/core.rs").is_file())
        self.assertFalse((output / "public/local-secret.txt").exists())

    def test_index_snapshot_ignores_unstaged_worktree_content(self) -> None:
        self.write("public/core.rs", "indexed safe\n")
        self.manifest(secrets=[r"[U]NSTAGED_SECRET"])
        self.track()
        self.write("public/core.rs", "UNSTAGED_SECRET\n")
        output = Path(self.temporary.name) / "export"

        result = self.run_tool("--output", str(output), "--execute")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((output / "public/core.rs").read_text(), "indexed safe\n")

    def test_staged_secret_cannot_be_masked_by_safe_worktree_content(self) -> None:
        self.write("public/core.rs", "INDEX_SECRET\n")
        self.manifest(secrets=[r"[I]NDEX_SECRET"])
        self.track()
        self.write("public/core.rs", "safe worktree replacement\n")

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("credential signature", result.stderr)

    def test_staged_symlink_cannot_be_masked_by_regular_worktree_file(self) -> None:
        target = Path(self.temporary.name) / "target.txt"
        target.write_text("outside\n", encoding="utf-8")
        public = self.root / "public"
        public.mkdir()
        (public / "link.txt").symlink_to(target)
        self.manifest()
        self.track()
        (public / "link.txt").unlink()
        (public / "link.txt").write_text("regular worktree file\n", encoding="utf-8")

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("symlinks are not exportable", result.stderr)

    def test_git_replacement_ref_cannot_substitute_indexed_blob(self) -> None:
        self.write("public/core.rs", "indexed safe\n")
        self.manifest()
        self.track()
        indexed_object = subprocess.run(
            ["git", "-C", str(self.root), "rev-parse", ":public/core.rs"],
            text=True,
            stdout=subprocess.PIPE,
            check=True,
        ).stdout.strip()
        replacement_object = subprocess.run(
            ["git", "-C", str(self.root), "hash-object", "-w", "--stdin"],
            input="replacement private content\n",
            text=True,
            stdout=subprocess.PIPE,
            check=True,
        ).stdout.strip()
        subprocess.run(
            [
                "git",
                "-C",
                str(self.root),
                "replace",
                indexed_object,
                replacement_object,
            ],
            check=True,
        )
        output = Path(self.temporary.name) / "export"

        result = self.run_tool("--output", str(output), "--execute")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((output / "public/core.rs").read_text(), "indexed safe\n")

    def test_git_index_environment_override_is_ignored(self) -> None:
        self.write("public/core.rs", "indexed safe\n")
        self.manifest()
        self.track()
        alternate_index = Path(self.temporary.name) / "alternate-index"
        alternate_environment = os.environ.copy()
        alternate_environment["GIT_INDEX_FILE"] = str(alternate_index)
        self.write("public/core.rs", "alternate private content\n")
        subprocess.run(
            ["git", "-C", str(self.root), "add", "."],
            check=True,
            env=alternate_environment,
        )
        self.write("public/core.rs", "indexed safe\n")
        output = Path(self.temporary.name) / "export"

        result = self.run_tool(
            "--output",
            str(output),
            "--execute",
            environment=alternate_environment,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((output / "public/core.rs").read_text(), "indexed safe\n")

    def test_allowlist_cannot_override_denied_path(self) -> None:
        self.write("desktop/native/capture/private.rs", "pub fn private() {}\n")
        self.manifest(include=["desktop/**", "open-source-boundary.json"])
        self.track()

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("allowlist/denylist overlap", result.stderr)
        self.assertIn("desktop/native/capture/private.rs", result.stderr)

    def test_exact_allowlist_rejects_globs_and_include_required_drift(self) -> None:
        self.write("public/core.rs", "pub fn safe() {}\n")
        self.manifest(
            include=["public/**", "open-source-boundary.json"],
            required=["public/core.rs", "open-source-boundary.json"],
            minimum=2,
            exact=True,
        )
        self.track()

        glob = self.run_tool("--check")
        self.assertNotEqual(glob.returncode, 0)
        self.assertIn("exact allowlist cannot contain a glob", glob.stderr)

        data = json.loads((self.root / "open-source-boundary.json").read_text())
        data["scopes"][0]["include"] = [
            "public/core.rs",
            "open-source-boundary.json",
            "public/extra.rs",
        ]
        self.write("open-source-boundary.json", json.dumps(data, indent=2) + "\n")
        self.track()
        drift = self.run_tool("--check")
        self.assertNotEqual(drift.returncode, 0)
        self.assertIn("exact allowlist must equal required_files", drift.stderr)

    def test_deny_paths_are_case_insensitive(self) -> None:
        self.write("public/SECRET-material.txt", "private\n")
        self.manifest(deny=["**/*secret*"])
        self.track()

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("allowlist/denylist overlap", result.stderr)

    def test_forbidden_private_runtime_import_is_detected(self) -> None:
        forbidden = "from " + "private_runtime.capture import Recorder\n"
        self.write("public/adapter.py", forbidden)
        self.manifest()
        self.track()

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("forbidden private-runtime import", result.stderr)
        self.assertIn("public/adapter.py:1", result.stderr)

    def test_grouped_rust_import_of_native_capture_is_detected(self) -> None:
        forbidden = "use margins::{app,\n    " + "recorder,\n};\n"
        self.write("public/cli.rs", forbidden)
        self.manifest(
            imports=[
                r"(?ms)^\s*use\s+(?:crate|margins)::\{[^}]*\b(?:coreml_asr|recorder)\b"
            ]
        )
        self.track()

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("forbidden private-runtime import", result.stderr)
        self.assertIn("public/cli.rs:1", result.stderr)

    def test_whitespace_obfuscated_rust_private_import_is_detected(self) -> None:
        forbidden = "use crate " + ":: recorder::Recorder;\n"
        self.write("public/cli.rs", forbidden)
        self.manifest(
            imports=[r"\b(?:crate|margins)\s*::\s*(?:project|recorder|tui)\b"]
        )
        self.track()

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("forbidden private-runtime import", result.stderr)

    def test_rust_include_of_denied_runtime_is_detected(self) -> None:
        forbidden = "include" + '!("../src/recorder.rs");\n'
        self.write("public/cli.rs", forbidden)
        self.manifest(
            imports=[r"include(?:_bytes|_str)?!\s*\([^)]*(?:project|recorder|tui)"]
        )
        self.track()

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("forbidden private-runtime import", result.stderr)

    def test_private_cargo_path_dependency_is_detected(self) -> None:
        private_path = "../crates/" + "private/capture"
        self.write(
            "public/Cargo.toml",
            f'[dependencies]\nprivate = {{ path = "{private_path}" }}\n',
        )
        private_pattern = (
            r'(?m)path\s*=\s*["\'][^"\']*crates/'
            + r'private[^"\']*["\']'
        )
        self.manifest(imports=[private_pattern])
        data = json.loads((self.root / "open-source-boundary.json").read_text())
        data["import_scan_extensions"].append(".toml")
        self.write("open-source-boundary.json", json.dumps(data, indent=2) + "\n")
        self.track()

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("forbidden private-runtime import", result.stderr)
        self.assertIn("public/Cargo.toml:2", result.stderr)

    def test_credential_signature_is_detected_in_allowed_file(self) -> None:
        credential = "AKIA" + "ABCDEFGHIJKLMNOP"
        self.write("public/config.py", f'VALUE = "{credential}"\n')
        self.manifest()
        self.track()

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("credential signature", result.stderr)

    def test_additional_registry_token_signatures_are_detected(self) -> None:
        production_patterns = json.loads(
            (REPO_ROOT / "open-source-boundary.json").read_text(encoding="utf-8")
        )["secret_content_regexes"]
        tokens = {
            "hugging-face": "hf_" + "A" * 30,
            "gitlab": "glpat-" + "B" * 20,
            "npm": "npm_" + "C" * 36,
            "pypi": "pypi-AgEIcHlwaS5vcmc" + "D" * 20,
            "slack-cookie": "xoxc-" + "E" * 20,
        }
        for label, token in tokens.items():
            with self.subTest(label=label):
                with tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary) / "repo"
                    root.mkdir()
                    subprocess.run(["git", "init", "-q", str(root)], check=True)
                    original_root = self.root
                    self.root = root
                    try:
                        self.write("public/config.py", f'VALUE = "{token}"\n')
                        self.manifest(secrets=production_patterns)
                        self.track()
                        result = self.run_tool("--check")
                    finally:
                        self.root = original_root
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("credential signature", result.stderr)

    def test_export_is_normalized_and_refuses_existing_destination(self) -> None:
        self.write("public/tool.py", "#!/usr/bin/env python3\nprint('safe')\n")
        os.chmod(self.root / "public/tool.py", 0o755)
        self.manifest()
        self.track()
        output = Path(self.temporary.name) / "export"
        second_output = Path(self.temporary.name) / "export-again"

        first = self.run_tool("--output", str(output), "--execute")
        second = self.run_tool("--output", str(second_output), "--execute")
        replacement = self.run_tool("--output", str(output), "--execute")

        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(
            first.stdout.split("sha256 ")[-1], second.stdout.split("sha256 ")[-1]
        )
        self.assertNotEqual(replacement.returncode, 0)
        self.assertIn("refusing to replace existing output path", replacement.stderr)
        exported = output / "public/tool.py"
        exported_again = second_output / "public/tool.py"
        self.assertEqual(exported.read_bytes(), exported_again.read_bytes())
        self.assertEqual(exported.stat().st_mode & 0o777, 0o755)
        self.assertEqual(exported.stat().st_mode, exported_again.stat().st_mode)
        self.assertEqual(int(exported.stat().st_mtime), 315532800)
        self.assertEqual(exported.stat().st_mtime, exported_again.stat().st_mtime)
        self.assertEqual(output.stat().st_mode & 0o777, 0o755)
        self.assertEqual((output / "public").stat().st_mode & 0o777, 0o755)

    def test_exact_required_source_can_map_to_export_root(self) -> None:
        self.write("repository-shell/README.md", "# Public repository\n")
        self.manifest(
            include=["repository-shell/README.md", "open-source-boundary.json"],
            required=["repository-shell/README.md", "open-source-boundary.json"],
            minimum=2,
            exact=True,
        )
        data = json.loads((self.root / "open-source-boundary.json").read_text())
        data["export_paths"] = {"repository-shell/README.md": "README.md"}
        self.write("open-source-boundary.json", json.dumps(data, indent=2) + "\n")
        self.track()
        output = Path(self.temporary.name) / "export"

        created = self.run_tool("--output", str(output), "--execute")
        verified = self.run_tool("--verify-tree", str(output))

        self.assertEqual(created.returncode, 0, created.stderr)
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertEqual((output / "README.md").read_text(), "# Public repository\n")
        self.assertFalse((output / "repository-shell").exists())

        source_copy = output / "repository-shell/README.md"
        source_copy.parent.mkdir()
        source_copy.write_text("unexpected source-layout copy\n", encoding="utf-8")
        extra = self.run_tool("--verify-tree", str(output))
        self.assertNotEqual(extra.returncode, 0)
        self.assertIn("not allowlisted at export path", extra.stderr)

    def test_export_path_destinations_must_be_unique(self) -> None:
        self.write("shell/one.md", "one\n")
        self.write("shell/two.md", "two\n")
        self.manifest(
            include=["shell/one.md", "shell/two.md", "open-source-boundary.json"],
            required=["shell/one.md", "shell/two.md", "open-source-boundary.json"],
            minimum=3,
            exact=True,
        )
        data = json.loads((self.root / "open-source-boundary.json").read_text())
        data["export_paths"] = {
            "shell/one.md": "README.md",
            "shell/two.md": "README.md",
        }
        self.write("open-source-boundary.json", json.dumps(data, indent=2) + "\n")
        self.track()

        result = self.run_tool("--check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("duplicate export path destination", result.stderr)

    def test_directory_modes_do_not_depend_on_umask(self) -> None:
        self.write("public/nested/core.rs", "pub fn safe() {}\n")
        self.manifest()
        self.track()
        output = Path(self.temporary.name) / "export"

        result = self.run_tool("--output", str(output), "--execute", umask=0o077)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output.stat().st_mode & 0o777, 0o755)
        self.assertEqual((output / "public").stat().st_mode & 0o777, 0o755)
        self.assertEqual((output / "public/nested").stat().st_mode & 0o777, 0o755)

    def test_dangling_output_symlink_is_rejected(self) -> None:
        self.write("public/core.rs", "pub fn safe() {}\n")
        self.manifest()
        self.track()
        target = Path(self.temporary.name) / "target"
        output = Path(self.temporary.name) / "output-link"
        output.symlink_to(target, target_is_directory=True)

        result = self.run_tool("--output", str(output), "--execute")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("refusing to replace existing output path", result.stderr)
        self.assertTrue(output.is_symlink())
        self.assertFalse(target.exists())

    def test_custom_export_manifest_must_match_embedded_manifest(self) -> None:
        self.write("public/core.rs", "pub fn safe() {}\n")
        self.manifest()
        self.track()
        custom = Path(self.temporary.name) / "custom.json"
        data = json.loads((self.root / "open-source-boundary.json").read_text())
        data["artifact_status"] = "different-policy"
        custom.write_text(json.dumps(data), encoding="utf-8")
        output = Path(self.temporary.name) / "export"

        result = self.run_tool(
            "--output",
            str(output),
            "--execute",
            manifest_path=custom,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("manifest used for export differs", result.stderr)
        self.assertFalse(output.exists())

    def test_verify_tree_rejects_extra_and_forbidden_files(self) -> None:
        self.write("public/core.rs", "pub fn safe() {}\n")
        self.manifest()
        self.track()
        output = Path(self.temporary.name) / "export"
        created = self.run_tool("--output", str(output), "--execute")
        self.assertEqual(created.returncode, 0, created.stderr)
        extra = output / "desktop/private.rs"
        extra.parent.mkdir(parents=True)
        extra.write_text("private\n", encoding="utf-8")

        result = self.run_tool("--verify-tree", str(output))

        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(
            "not allowlisted" in result.stderr or "forbidden path" in result.stderr,
            result.stderr,
        )

    def test_verify_tree_rejects_missing_required_file(self) -> None:
        self.write("public/core.rs", "pub fn safe() {}\n")
        self.manifest(required=["public/core.rs"])
        self.track()
        output = Path(self.temporary.name) / "export"
        created = self.run_tool("--output", str(output), "--execute")
        self.assertEqual(created.returncode, 0, created.stderr)
        (output / "public/core.rs").unlink()
        (output / "public/replacement.rs").write_text("replacement\n", encoding="utf-8")

        result = self.run_tool("--verify-tree", str(output))

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing required files: public/core.rs", result.stderr)

    def test_verify_tree_rejects_symlink_root(self) -> None:
        self.write("public/core.rs", "pub fn safe() {}\n")
        self.manifest()
        self.track()
        output = Path(self.temporary.name) / "export"
        created = self.run_tool("--output", str(output), "--execute")
        self.assertEqual(created.returncode, 0, created.stderr)
        link = Path(self.temporary.name) / "export-link"
        link.symlink_to(output, target_is_directory=True)

        result = self.run_tool("--verify-tree", str(link))

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("verification target must be a real directory", result.stderr)


if __name__ == "__main__":
    unittest.main()
