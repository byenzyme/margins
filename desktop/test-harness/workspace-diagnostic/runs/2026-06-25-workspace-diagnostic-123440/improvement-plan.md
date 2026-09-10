# Improvement Plan — Run 2026-06-25-workspace-diagnostic-123440

Judge: Enzyme-aware Judge. Active revision evaluated: `skill-revisions/v000-baseline.md`.
Surface: `skill-only`. `expected-diagnostic.md` treated as evidence, not oracle.

## Headline

All 12 cases pass (overall_score >= 2; 1 case at 3). The baseline skill is
**safe and product-correct on every axis that matters for trust**:

- `non_mutation_boundary` = 3 in all 12 cases
- `capture_not_blocked` = 3 in all 12 cases
- `workspace_classification` = 3 in all 12 cases
- No case suggests restructuring, no mechanism-first blocking language, and the
  import-vs-capture distinction holds (most visibly in zoom-transcript-dump's
  "do not import transcripts as finished notes" and partial-enzyme-or-margins's
  refusal to rebuild the cache before capture).

The remaining gaps are **explanation quality and evidence accuracy**, not
correctness. This warrants a bounded stacked patch (v001), not a stop.

## Enzyme-lens assessment (the core question for this run)

> Does the candidate use Enzyme as an indexability/setup lens without requiring
> init, import, cache-building, or schema repair before first capture?

**Yes, on the requirement.** No case demands `enzyme init`, refresh, import, or
schema/frontmatter repair before capture. partial-enzyme-or-margins is the proof:
it sees `.enzyme`/`.margins` state and explicitly refuses to repair/rebuild it,
keeping cache-building as a deferred, separate action. frontmatter-obsidian
refuses to normalize malformed YAML first.

**But the lens is shallow.** The "What Is Uncertain" sections restate manifest
convention tags rather than reasoning about *what is already usable retrieval
signal vs. what is weak/raw*. The diagnostics read as manifest-derived, not
scan-derived (detected_patterns even leaks tokens like "none"/"mixed"). The
indexability framing from ENZYME_EVALUATOR_CONTEXT — "already usable / weakly
indexable / would materially improve retrieval / can wait" — is not yet the
backbone of the uncertainty copy.

## Recurring failure patterns (priority order)

### 1. Generic import narrative (highest impact)
The user-facing "History And Import" block is boilerplate across empty, clean,
and import-heavy workspaces alike. The cases that most need specificity get the
least:
- google-drive-export: duplicate `client-kickoff` exports, half-converted
  `Untitled document.md`, partial cache — none named in prose.
- zoom-transcript-dump: partial/incomplete export log, VTT/TXT duplicate, a
  truncated transcript — none named.
- partial-enzyme-or-margins: interrupted `1.2.0-beta` migration, partially-applied
  import, config/destination mismatch — flattened to boilerplate.
Safety is preserved (do-not-do lists are specific), but the user is told
"optional" without being told *what* it is optional about.

### 2. Manifest-tag leakage into user-facing copy
Raw tokens ("malformed-frontmatter", "wikilinks", "csv-headers", "none",
"mixed") appear in uncertainty text and detected_patterns. Borderline
mechanism/vault-health tone; fails language-fit ceiling.

### 3. Cache-state detection bug
"no hidden app/cache folders visible" prints even when visible cache/import
files (cache-state.json, import manifests, export logs) exist and are in
evidence_files (google-drive-export, zoom-transcript-dump).

### 4. Ambiguity + confidence honesty
- daily-notes-vault: do-not-do "force a meeting folder that doesn't exist" while
  a `meetings/` folder exists; competing Daily/ vs meetings/ convention never
  named; "no import requirement" despite a transcript present.
- rfc-decision-log: "high" confidence without the "process docs are not a
  meeting workflow" nuance.

## Recommended next agent: Skill Surgeon (v001)

Patch one bounded surface — the diagnostic skill/prompt — producing
`skill-revisions/v001-import-narrative-and-language.md` built on v000. Scope:

1. **Case-specific import narrative.** When import/cache/export artifacts are
   present, read them and name, in plain language: duplicates, partial/incomplete
   imports, half-converted files, interrupted migrations. Keep "optional, after
   first capture" framing. (improves cases: google-drive-export,
   zoom-transcript-dump, partial-enzyme-or-margins, mixed-messy-power-vault)
2. **Translate convention tags to outcome language.** Never surface raw manifest
   tokens. Map "malformed-frontmatter" -> "some notes have inconsistent
   metadata, which is fine and will not be changed." Strip junk tokens
   ("none"/"mixed") from detected_patterns. (improves: frontmatter-obsidian,
   sparse-random-notes, zoom-transcript-dump, foreign-domain-vault)
3. **Fix cache-state reporting.** Report any cache/import/export state files
   regardless of dot-prefix, and tie them to the "build cache is separate" and
   "leave leftover state alone" framing. (improves: google-drive-export,
   zoom-transcript-dump, partial-enzyme-or-margins)
4. **Name competing destinations + calibrate confidence.** When >1 plausible
   destination exists, state the alternative and the tradeoff; lower confidence
   when strong structure is non-meeting. Fix the daily-notes contradiction.
   (improves: daily-notes-vault, rfc-decision-log)
5. **Optional, lower priority:** for foreign-domain folders prefer a clearly
   Margins-namespaced destination over `notes/meetings/` to reinforce isolation.

### Regression watch for the Skill Surgeon / Regression Hunter
- Do not let richer import prose start sounding like a cleanup mandate — keep
  capture-now and non-mutation at score 3 everywhere.
- After patching import/transcript handling, rerun fresh-empty and
  codebase-plans-folder to confirm the boilerplate removal did not strip the
  reassuring "start now / start fresh" language from low-import cases.
- After language-tag changes, rerun append-only-dated-log to confirm it still
  avoids imposing append-only structure.

## Not recommended this run
- **No fixture patches.** Fixtures are well-evidenced, realistic, and
  product-correct; `expected_diagnostic_issue` is false in all 12. (`fresh-empty`
  is the lone fixture that pre-creates a folder up front, which is acceptable for
  an empty workspace.)
- **No app-UX patch yet** (skill-only run). The partial-enzyme-or-margins and
  google-drive findings do map to future surfaces ("Build private search cache",
  "What Margins noticed", import entry point); flag them for a later
  `first-project-onboarding` / `build-private-search-cache` UX run, but the skill
  is the right lever now.
- **Do not stop.** No stop condition is met: substrate was read-only, artifacts
  are complete and valid, and the fixture is not the blocker for any case.

## Final summary
- Coverage: 12/12 cases judged; all required artifacts present.
- Pass/fail: 12 pass, 0 fail. fresh-empty = 3; the other 11 = 2.
- Top recurring failures: generic import narrative; manifest-tag leakage;
  cache-state detection bug; weak ambiguity/confidence honesty.
- Patching needed: yes — bounded skill patch (v001), quality/language/accuracy
  only; no safety or mutation defects.
- Recommended next agent: **Skill Surgeon**, then **Regression Hunter** on the
  watch list above.
