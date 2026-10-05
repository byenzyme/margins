# Public cutover scan and replay filter

The source tree becomes the public repository after the coordinator creates a
fresh initial commit. The squashed root commit must contain the **filtered tree
at `54a7a9d7c`**, rather than the original tree at that revision. The coordinator
then replays `54a7a9d7c..HEAD` through the same path and text filters. This file
records those filters; it does not rewrite history.

## Remove from every replayed commit

The machine-readable list is in
[`scripts/public_scan_config.json`](../scripts/public_scan_config.json). It
includes the vendored private engine, the embedded Google Desktop OAuth JSON,
the export manifest and exporter, their tests and E2E wrapper, the boundary and
export-only workflows, and the entire old `public-repository/` mirror. Current
documentation from that mirror was moved to the repository root or `docs/`
before these paths were removed. The engine's model card, examples, and
benchmarks remain at the exact pinned private enzyme-rust commit.

## Replace in the squash base and every replayed commit

Pass this exact `git filter-repo --replace-text` file alongside the path
filters:

```text
literal:/Users/example/==>/Users/example/
literal:/home/example/==>/home/example/
```

Because this runbook contains the literal expressions, `--replace-text` also
rewrites the two lines above in replayed history; that self rewrite is expected.

These are the only real personal home prefixes found. Paths such as
`/Users/alice/` and `/Users/me/` are deliberate examples and fixtures.

Run the base-tree scan before creating the squashed root:

```bash
python3 scripts/scan-public-history.py --tree 54a7a9d7c
```

It reports 78 personal-path occurrences, all using the macOS prefix above, in
41 files outside the vendored recall-engine directory. The macOS replacement
above covers every one of them. The base tree has no Linux-prefix occurrences;
that prefix first appears in the replay range. The 41 base-tree files are:

```text
.claude/workflows/RESUME_STATE.md
.claude/workflows/design-roadmap-implement.js
crates/private/recall-engine.SUBTREE.md
desktop/AUDIO_SETTINGS_SIMPLIFICATION_SPEC.md
desktop/DISTILL_PERF_NOTES.md
desktop/NOTE_MAKING_TAB_REDESIGN_SPEC.md
desktop/scripts/build-signed-mac.sh
desktop/scripts/build-updater-release.sh
desktop/scripts/build-windows-nsis.sh
desktop/scripts/distill-perf-metrics.mjs
desktop/scripts/reinstall-app.sh
desktop/scripts/snapshot-local-state.sh
desktop/scripts/ux-e2e-real-distill.mjs
desktop/scripts/windows-parakeet-validate.sh
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/append-only-dated-log/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/codebase-plans-folder/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/daily-notes-vault/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/foreign-domain-vault/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/fresh-empty/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/frontmatter-obsidian/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/google-drive-export/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/mixed-messy-power-vault/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/partial-enzyme-or-margins/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/rfc-decision-log/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/sparse-random-notes/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-123440/cases/zoom-transcript-dump/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/aggregate-verdict.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/cases/append-only-dated-log/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/cases/codebase-plans-folder/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/cases/codebase-plans-folder/enzyme-skill-diagnostic-probe.md
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/cases/codebase-plans-folder/verifier-verdict.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/cases/google-drive-export/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/cases/partial-enzyme-or-margins/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/cases/zoom-transcript-dump/enzyme-scan.json
desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/rerun-comparison.md
desktop/ux-shots-real-smoke-default/report.md
desktop/ux-shots-real-smoke-keychain/report.md
desktop/ux-shots-real-smoke/report.md
docs/open-core-modularization.md
docs/public-cli-extraction.md
enzyme-skill-failure-modes-2026-07-03.findings.json
```

## Scan results before replay

`scripts/local-gate scan 54a7a9d7c..HEAD` reports hits per commit and does
not print matched values. Against the 247 reachable commits in this checkout,
the scan found 121 added-line hits across nine commits:

| Commit | Location | Hits | Disposition |
| --- | --- | ---: | --- |
| `44154f3f109c` | BB plugin test harness | 1 email | Synthetic fixture |
| `5102563388b2` | Generated BB plugin `dist/host.js` | 101 Linux paths | Replace the absolute builder prefix in replayed commits |
| `6438c03dd47c` | Mac audio verification report | 4 paths | Replace personal home prefixes in replayed commits |
| `c954f879b70e` | Remote workspace report | 1 path | Replace personal home prefix in replayed commits |
| `f0117844d395` | Native bridge test code | 1 email | Synthetic fixture |
| `fd392a611237` | Fresh onboarding script | 1 email | Synthetic fixture |
| `580acc15c047` | Vendored Matroska manifests | 2 emails | Public upstream author attribution |
| `c469b962f79b` | Diagnostic evidence and cutover report | 8 emails | Reviewed synthetic fixtures/evidence |
| Cutover review documentation | This filter runbook | 2 paths | Exact replacement literals above |

The current tree was also scanned after removing the embedded OAuth JSON and
vendored engine. It has **zero unreviewed secret-shaped hits and zero
unreviewed personal home-path hits**. The two remaining personal-path matches
are the exact replacement expressions documented above. The 625 email matches
comprise 539 obvious synthetic-domain addresses and 86 reviewed matches in unit
tests, fixture and diagnostic snapshots, public vendor sender examples,
third-party package attribution, and one icon filename false positive. Two
OpenAI-shaped strings are synthetic test cases in
`tests/test_workspace_setup_rollout_review.sh`. The retained
diagnostic evidence was kept and its home paths redacted; one email in an
evidence JSON file was replaced with `person@example.com`.

For the replayed commit range, both replacements above are required in addition
to path filtering. They cover the three historical macOS-prefix and 103
historical Linux-prefix added-line hits, plus the two documented replacement
literals in this runbook. In particular, filtering the
generated BB plugin file outright would remove a shipped artifact from those
historical trees. The current plugin bundle no longer contains the absolute
builder path. Re-run the scanner on the filtered history and final tree before
publishing the new repository. The `#NN` references in replayed commit messages
are removed by the coordinator's filter step.
