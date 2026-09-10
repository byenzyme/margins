I have enough grounding. The naming divergence (both `partial-enzyme-or-aside` AND `partial-enzyme-or-margins` fixtures exist) is confirmed, promotion was explicitly deferred, loop fixes are unpromoted, and `enzyme-petri` still appears in the live skill text (the hook-removal assertion from thr_ntikvmvrbd may be incomplete). Writing the report.

---

# Update on the Situation: Enzyme Skill Failure-Mode Exploration (June 23 – July 3, 2026)

## 1. TL;DR

The exploration effort produced two disjoint bodies of work that never fully connected. The bulk of the energy (June 25, ~20 threads) went into a **workspace-diagnostic self-improvement loop** — a maker/checker/judge/patcher/verifier machine over 12–13 synthetic fixture vaults that surfaced a strong, well-evidenced failure taxonomy but never promoted a single fix to the real skill and never ran a *live* rerun (all verification was artifact-text checking, not re-execution). Separately, a smaller **skill-test/retest era** (July 2) ran the real skill against real and fixture vaults and caught genuine, shipped-or-shippable bugs (login-gate isolation, invalid-key catalyst failure, template pollution, folder mismatch). The **installation-fix era** (June 24 setup patch thr_ntikvmvrbd, plus the July `margins import-granola` work) is the only place where fixes actually landed in the live `plugin/plugins/enzyme/skills/enzyme/SKILL.md`. Today the live skill contains the setup/auth/version-contract fixes but **none** of the diagnostic-loop taxonomy fixes (temp-copy scan guard, import-narrative, Non-Failures sections — grep confirms absence). The loop is intact but open-ended: it has no closing leg back to the canonical skill and no behavioral (vs. artifact) verification.

## 2. Failure-mode taxonomy (deduplicated & grouped)

### A. Setup / auth / credentials
- **Stale/bare `OPENAI_API_KEY` causes auth mismatch on init** — any vault where the shell inherits a personal/proxy key. **Fixed & in live SKILL.md** (confirmed: prerequisite section checks env var *names* only). From *Enzyme Schedule Setup Failure* (thr_ntikvmvrbd).
- **Login-gate blocks even local indexing when HOME is isolated** — every vault where the skill enforces workspace isolation. Only workaround is `--use-env-llm`. **Open** (no `--config-dir` flag exists). From *Retest v2: setup power-vault* (thr_8ajai2wjdv), *Skill test: setup power-vault* (thr_dj4c36kdxq).
- **Invalid/expired key → catalysts=0, silent-continue-then-fail** — any vault with a bad key at init; catalyze returns empty. Related misreport: **catalysts=0 wrongly reported as setup failure** is now **fixed** in the workspace-setup skill text (thr_td77hxfp46), but the underlying silent-fail-after-3-retries behavior is **open** (thr_8ajai2wjdv, thr_dj4c36kdxq).

### B. Version / upgrade contract
- **Pre-0.5.8 auth-contract mismatch, no version guard** — any vault with an old install. **Fixed & in live SKILL.md** (confirmed: `0.5.8` warning + no-delete-auth-without-confirmation present). From thr_ntikvmvrbd.

### C. Install-path / side-effect pollution
- **Default install silently writes Claude hooks (`enzyme-petri`) & mutates settings.json** — any plugin-installed vault. Marked **fixed** (thr_ntikvmvrbd), but grep shows `enzyme-petri` still referenced in live `agent/SKILL.md` and `agent-runtime-skill.md` — **fix likely incomplete or the sync did not reach these surfaces**.
- **`enzyme install codex` pollutes AGENTS.md / .agents/** and **`.margins` visible/committable in Git & Obsidian** — codebase, obsidian, sync-tree vaults. **Open** (spec proposals only). From *Enzyme Skill Prompt Audit* (thr_e5gjah4tjx).
- **`enzyme scan` writes `.enzyme/enzyme.log` residue into uninitialized/pre-configured repos** — any repo with existing `.enzyme/`. **Fixed in loop-local revision** (v001 temp-copy-scan-guard) but **NOT promoted to live skill** (grep confirms). From thr_9pmvshfejz, thr_gkhq3xn3ch, and every v001 thread.

### D. Vault-shape assumptions / scan blindness
- **Scan blind to non-Markdown corpus** (transcripts, import logs, cache-state, nested docs; scan returns 1 file when 9–16 exist) — google-drive-export, zoom-transcript-dump, codebase-plans-folder. **Partial** (process discipline: "read representative files after scan"), never a binary fix. Ubiquitous across thr_35p36qaa4v, thr_wdwezs6tb6, thr_swwxz4qhsv, thr_dscda2ix48.
- **Scan entity weighting dominated by noise/package folders** (Readwise, daily, `packages/`) — live obsidian (thr_gv5d95xrxb), bb/hermes-agent codebases (thr_9pmvshfejz). **Open**; identified as a missing "evaluator layer."
- **Template outranks real notes in semantic search** (0.601 similarity) — obsidian fixture. **Open**. From *Retest v2: assess obsidian-vault* (thr_dqj93fujr7).
- **Folder/destination mismatch not surfaced** (stated "meetings" ≠ actual `notes/meetings`) — obsidian, power-vault. **Fixed** in workspace-setup skill (thr_td77hxfp46); **still open** as a diagnostic-detection problem (thr_dqj93fujr7).
- **Stale/duplicate tool-state directories** (root `.enzyme` + `inbox/.enzyme`) mis-read as canonical — live obsidian, partial-enzyme fixture. **Open** as skill behavior; the *cwd* root-cause was **fixed** in the desktop backend (thr_uivw837d62).

### E. Prompt / instruction-following & output quality
- **Eager init during ordinary diagnosis** / **global-config-write surprises sandboxed agents** — any workspace. **Fixed** in workspace-setup skill (thr_td77hxfp46, thr_ca5nm2je4t).
- **Generic import narrative / manifest-tag leakage / cache-state detection bug / ambiguity-collapse & overconfidence** — the four recurring judge findings across all 12 fixtures. **Fixed-unverified** in loop-local v002, **NOT promoted** to live skill. From the entire judge cluster (thr_7fkhm8x6ad, thr_i8a675hatm, thr_dmf7ib4ff3, thr_sbp5n22veb, thr_myrk2ek72u, thr_n6un5qwnnz, thr_gkhq3xn3ch, thr_ykgyqzad9r, thr_dzdstbnw5h, thr_y9uvugcpv5).
- **Failure model missing Non-Failures/Ambiguities + templated retrieval-impact** — over-diagnoses healthy vaults. **Attempted-unverified** (v002). Same cluster.
- **Shallow Enzyme lens** (manifest-derived, not scan-derived; "already usable / weakly indexable / would improve retrieval / can wait" framing absent) — all fixtures. **Partial/Open** — the deepest finding, explicitly *not* fixed by v001/v002.

### F. Distillation output quality (adjacent to setup)
- **Single-mention failure mode dropped / pivot-reframe turn missed / speaker misattribution under bad diarization / frontmatter schema imposed / invented tags** — real obsidian session notes. **All open** (recommendations only). From *Distill quality judge* (thr_w279wvpbjn, thr_dtkpp93jam).

### G. Import compliance (CLI/lib)
- **Missing enzyme frontmatter, silent `.margins` fallback, org double-counting, non-idempotent re-import** — any Granola-import vault. **All fixed & verified live** (cargo test 99→103 passing). From *Codex: Granola import enzyme compliance* (thr_cfqhae4vka). TS type-check **open**.
- **Non-Markdown/malformed-YAML/stale-mtime/Drive-streamed import risks** — export vaults. **Attempted-unverified** (prompt-level). From *Review import skill success criteria* (thr_6et2es8kb6).

### H. Lifecycle / refresh & desktop integration
- **No post-distillation refresh; `.margins`/skill not created on project-add; `inbox_folder` defaults to `""` (notes land at vault root); validate_vault checks wrong dir / dir-existence not DB** — desktop projects. **Mostly open**; some backend cwd/vault-root pieces **fixed** (thr_uivw837d62). From thr_85yxh28icp, thr_qsxyy2dkzv, thr_95rr3fgv4a.

## 3. Exploration methods used — honest assessment

- **Maker/checker/judge/patcher/verifier loop over fixture vaults** (the June 25 machine). *Produced real findings* — the judge rubric (8 scored axes, evidence-vs-oracle distinction) reliably caught templating and structural gaps and routed them correctly to a Skill Surgeon rather than an app patch. *But it broke down repeatedly*: judge threads stalled 3× (thr_sbp5n22veb delegated to subagents, thr_myrk2ek72u/thr_n6un5qwnnz stopped after contract corrections); makers were stopped mid-run creating coordination races (thr_35p36qaa4v vs thr_36actjrbrg); and critically **verification was artifact-text-only** — no thread ever re-ran the diagnostic generator or `enzyme scan` and diffed output. thr_dzdstbnw5h is the smoking gun: all 12 traces still recorded `v000` while claiming a v002 pass, so the "12/12 pass" is a synthetic assertion. This is the single biggest source of *noise dressed as signal*.
- **Case-level probe prompts** (v001). Genuinely fixed the "artifact-without-skill-lens" process failure for 3 priority cases — the highest-value *process* outcome of the whole effort (thr_dscda2ix48, thr_swwxz4qhsv, thr_wdwezs6tb6, thr_i3wb88ddf9).
- **Single-turn live/fixture probes** (July 2 skill tests/retests). *Highest signal-per-token*: thr_8ajai2wjdv and thr_dqj93fujr7 caught login-gate, invalid-key, template-pollution, and folder-mismatch by just running the real skill once and reading the output. These are cheap and caught bugs the elaborate loop never surfaced (the loop tests a *proxy* diagnostic, not the shipped skill).
- **Static source/spec audits** (thr_e5gjah4tjx, thr_85yxh28icp, thr_qsxyy2dkzv, thr_26hnfy7gpw, thr_6et2es8kb6, thr_5qrzpfe6qe). Produced accurate risk maps with file:line citations but *zero verification* — several were blocked from reading the canonical skill (permitted-roots), so findings are `[inferred]`.
- **Direct patcher-from-findings** (thr_ntikvmvrbd, thr_td77hxfp46). The only method that *changed the live skill*. Weakness: no behavioral check — `make check-agent-skill` is a lint, not a judge, and cargo tests were sandbox-blocked (cidre/Xcode module-cache write).

**Verdict:** the cheap single-turn live retests and the direct patcher threads did the real work. The elaborate 12-fixture loop generated an excellent *taxonomy* but was expensive, orchestration-fragile, and never closed — its outputs are stranded in `runs/.../skill-revisions/`.

## 4. Chronology of the loop

- **June 23–24 (desktop-integration + first setup fixes):** thr_95rr3fgv4a / thr_uivw837d62 diagnosed readiness-dot and enzyme-cwd bugs against the *real* vault; thr_ntikvmvrbd landed the env-var/version-contract/hook-removal setup fixes into the live skill; distill-quality judges (thr_w279wvpbjn, thr_dtkpp93jam) ran but hit the OpenRouter 402 baseline blocker.
- **June 25 (the diagnostic-loop era — the bulk):** the full maker/checker/judge/patcher/verifier machine spun up over 12–13 fixtures. It iterated v000→v001 (temp-copy scan guard + case-level probes)→v002 (import-narrative + failure-taxonomy). Judge/runner threads stalled repeatedly; the spec-review (thr_26hnfy7gpw) correctly flagged that the loop **has no promotion leg back to canonical SKILL.md** and no gate effect for failed checkers. thr_y9uvugcpv5 revealed the whole run was executed under a *goal misinterpretation* (improved the harness, never edited upstream `plugin/agent/SKILL.md`).
- **July 1–2 (skill-tests / retests era):** the approach pivoted from the heavy loop to lightweight single-turn probes against real and freshly-provisioned fixture vaults (thr_ca5nm2je4t, thr_dj4c36kdxq, thr_8ajai2wjdv, thr_dqj93fujr7) and a concrete patcher spec (thr_td77hxfp46) that landed the eager-init/global-config/catalysts=0/destination-mismatch fixes. Granola import compliance (thr_cfqhae4vka) shipped with real cargo-test gating — the cleanest closed loop of the whole period.
- **July 3 (installation improvements / today):** the live `plugin/plugins/enzyme/skills/enzyme/SKILL.md` now frames setup as an "indexability assessment," carries the setup/auth/version-contract fixes, and reads as a mature setup skill. **But:** grep confirms *none* of the loop's diagnostic taxonomy fixes (temp-copy guard, import-narrative, Non-Failures) reached it; the promotion plan explicitly says "do not promote now"; `enzyme-petri` still appears in `agent/SKILL.md`; and both `partial-enzyme-or-aside` **and** `partial-enzyme-or-margins` fixtures still exist (the unresolved naming divergence from thr_8q787v45tt / thr_ppahjnvgvs that risks double-counting cases).

**Implication for current status:** the effort has strong *knowledge* (taxonomy, fixtures, rubric) and weak *closure*. The live skill improved via the cheap direct-patch path, not via the expensive loop. The loop's v001 process fix is genuinely worth promoting; v002 remains unverified.

## 5. Gaps

**Vault types never exercised (live or fixture):**
- **fresh-empty and highly-structured Obsidian vaults under the probe requirement** — flagged as the top regression risk (v001 could over-diagnose clean vaults) in thr_i3wb88ddf9 / thr_au8gpawzqu, never tested.
- **A correctly, fully-configured (non-partial) vault** — the "no action needed / start now" control path. thr_ppahjnvgvs explicitly asks for this fixture; it doesn't exist.
- **Import-preview-only vault (no Markdown notes)**, **large-`.enzyme`-to-exclude vault**, and **multi-plausible-destination-with-no-winner vault** — the three fixture gaps named in `memory-notes.md` across multiple threads. None built.
- **Pi agent path** (`.pi/skills/`) — never tested; bb reported no Pi models (thr_9ezns84g8v).
- **Real large production vault under the *canonical* skill** — thr_gv5d95xrxb ran against live obsidian but SKILL.md was missing at the expected path, so it exercised an ad-hoc substitute.

**Failure classes under-explored:**
- **Behavioral verification of any fix** — every "pass" is artifact-text; no fix has been confirmed by re-running the skill and diffing output.
- **Filesystem-checksum mutation guards** — non-mutation is always *attested*, never *proven* (thr_ykgyqzad9r, thr_dzdstbnw5h).
- **The shallow-lens fix** — the "usable / weakly-indexable / would-improve / can-wait" reasoning backbone was diagnosed everywhere but patched nowhere.
- **Fixture-vs-live divergence** — the loop tests a proxy diagnostic; the July probes show the *real* skill has different failure modes (login-gate, invalid-key) the loop never saw.

## 6. Recommended improvement loop (cost-aware, opinionated)

**Principle: stop scaling the 12-fixture judge machine; make it *close* and make it test the *real* skill.** The evidence is unambiguous — cheap single-turn live probes (thr_8ajai2wjdv, thr_dqj93fujr7) out-yielded the entire June-25 orchestration, and the loop's fatal flaw is artifact-only verification (thr_dzdstbnw5h).

**Fixtures — keep, cut, add:**
- **Keep** the ~6 high-signal cases that produced distinct findings: `google-drive-export`, `zoom-transcript-dump`, `partial-enzyme-or-*`, `daily-notes-vault` (competing destinations), `rfc-decision-log` (overconfidence control), `append-only-dated-log` (ready-now control).
- **Cut the duplication now:** resolve `partial-enzyme-or-aside` vs `partial-enzyme-or-margins` to one canonical name — this is an active case-enumeration bug (thr_8q787v45tt).
- **Add the three named-but-missing fixtures** (import-preview-only, large-`.enzyme`-exclude, no-winner-destination) and a **fully-configured-vault control** — because the highest-risk regression (over-diagnosing clean/structured vaults) has zero coverage.

**What the judge should score** (tighten to what caught real bugs): (1) non-mutation — *proven by pre/post `find`+shasum diff, not attestation*; (2) capture-not-blocked; (3) destination-mismatch surfaced when evidence conflicts with stated setting; (4) import specificity — named artifacts, not boilerplate; (5) no raw-token leakage (the `rg malformed-frontmatter|csv-headers|[object Object]` gate already works — keep it, it's the one automated check that fired); (6) confidence honesty on non-meeting structure; (7) the shallow-lens axis: does uncertainty copy use the four-tier indexability framing. Drop the templated adversarial/product-critic artifacts — they were byte-identical across every case in every run and produced no signal.

**How retests gate skill changes** (the missing closing leg — anchor in thr_26hnfy7gpw's finding that the loop has no promotion path):
1. A change is a *candidate* only after a **live rerun**: actually invoke the skill against fixtures and capture real `enzyme scan`/output, not regenerate an artifact. thr_dzdstbnw5h proves artifact-regeneration is a tautological pass.
2. Gate promotion on **both control cases passing** (`append-only-dated-log` must attract ~zero failure modes; fully-configured vault must say "no action needed") to catch over-diagnosis regressions before shipping.
3. On green, **promote to the canonical `plugin/plugins/enzyme/skills/enzyme/SKILL.md`** and re-run the existing `make sync-agent-skill && make check-agent-skill` — then extend the Rust content-assertion test (thr_td77hxfp46 pattern) with anchors for the promoted text so regressions are caught in CI. Right now v001's genuinely-good process fix is stranded in `runs/` because this leg doesn't exist.

**What to automate:**
- **The mutation guard** — a wrapper that snapshots `find + shasum` before/after every scan and fails hard on residue. This closes the single most-repeated open risk (thr_ykgyqzad9r, thr_dzdstbnw5h, thr_gkhq3xn3ch) and would have caught the `enzyme.log` leak automatically.
- **The raw-token `rg` gate** — already proven; wire it into `make check-agent-skill`.
- **A `--use-env-llm`-with-fixture-key harness** so the login-gate/invalid-key paths (thr_8ajai2wjdv) get a *clean-credentials* baseline — no run to date has ever seen a successful catalyst generation on a fixture, so there is no "known good" to diff against.
- **Fix the cidre/Xcode sandbox blocker** (or stub cidre for text-only tests) so the bundled-skill content tests can actually run in-loop — thr_td77hxfp46 and thr_i76j9kqxxw both died here.

**Do NOT automate / de-prioritize:** the multi-agent judge fan-out (four of five subagents were interrupted every time, thr_7fkhm8x6ad) — a single serial judge with a tight rubric is more reliable than parallel subagents that stall. And resolve the **goal ambiguity** first (thr_y9uvugcpv5): the target is the upstream `plugin/agent/SKILL.md`, not the harness — every run that forgets this produces stranded revisions.