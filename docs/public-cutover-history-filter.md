# Public cutover scan and replay filter

The source tree becomes the public repository after the coordinator creates a
fresh initial commit. This file records the paths and text replacement needed
for the subsequent replay of `54a7a9d7c..HEAD`. It does not rewrite history.

## Remove from every replayed commit

The machine-readable list is in
[`scripts/public_scan_config.json`](../scripts/public_scan_config.json). It
includes the vendored private engine, the embedded Google Desktop OAuth JSON,
the export manifest and exporter, their tests and E2E wrapper, the boundary and
export-only workflows, and the entire old `public-repository/` mirror. Current
documentation from that mirror was moved to the repository root or `docs/`
before these paths were removed. The engine's model card, examples, and
benchmarks remain at the exact pinned private enzyme-rust commit.

## Scan results before replay

`scripts/local-gate scan 54a7a9d7c..HEAD` reports hits per commit and does
not print matched values. Against the 246 reachable commits in this checkout,
the scan found 119 added-line hits across eight commits:

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

The current tree was also scanned after removing the embedded OAuth JSON and
vendored engine. It has **zero unreviewed secret-shaped hits and zero personal
home-path hits**. The 628 email matches comprise 539 obvious synthetic-domain
addresses and 89 reviewed matches in unit tests, fixture and diagnostic
snapshots, public vendor sender examples, third-party package attribution, and
one icon filename false positive. Two OpenAI-shaped strings are synthetic
test cases in `tests/test_workspace_setup_rollout_review.sh`. The retained
diagnostic evidence was kept and its home paths redacted; one email in an
evidence JSON file was replaced with `person@example.com`.

For the replayed commit range, a content replacement of personal home prefixes
is still required in addition to path filtering. In particular, filtering the
generated BB plugin file outright would remove a shipped artifact from those
historical trees. The current plugin bundle no longer contains the absolute
builder path. Re-run the scanner on the filtered history and final tree before
publishing the new repository. The `#NN` references in replayed commit messages
are removed by the coordinator's filter step.
