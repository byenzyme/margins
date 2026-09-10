# Enzyme setup skill — design brief (2026-07-03)

Source: 44-thread failure-mode consolidation (`enzyme-skill-failure-modes-2026-07-03.md`) + product interview with Josh. This brief is the contract for the next skill revision; implementation threads should treat the "Settled decisions" section as fixed.

## Settled decisions (from interview)

| Decision | Answer |
|---|---|
| Credentials default | Minted/provided API keys always. Environment keys (`OPENAI_API_KEY` etc.) are used **only** when `--use-env-llm` is explicitly set, and then require the full triple (key + base URL + model). The skill never lets ambient env keys leak into init. |
| Hooks | None installed by default, ever. Verify the enzyme-petri removal actually completed — consolidation found `enzyme-petri` still referenced in `agent/SKILL.md` and `agent-runtime-skill.md`. |
| Trust flow | In-place restructure with backup + one-command revert. |
| Consent | **One gate before mutation.** Diagnosis is read-only and free; then one plain-language moment presents the full plan + backup promise + revert story; a single yes unlocks everything. |
| Mutation bounds | File moves/renames within reason + batch frontmatter edits. **No note-body content edits.** Revert = replay recorded moves in reverse + restore original frontmatter. |
| Structure ground truth | Doctrine **derived from Enzyme internals** (how scan/catalyze actually chunk, index, weight, retrieve — mine enzyme-rust), and each recommendation **proven locally** on the user's own content before it is asserted. |
| Proof artifact | A **distilled example note** produced from the user's own content — they see the end product, not the plumbing. |
| Time envelope | **Fast first pass + background deepening.** <~2 min to usable + proof note; deeper restructure work continues afterwards and reports back. |
| Healthy vault | Say so explicitly ("no restructuring needed — your vault already compounds well"), show the four-tier map as confirmation, still produce the proof note. Never manufacture findings (over-diagnosis is a known failure mode with zero tolerance). |
| Audience | Margins app user, plain register. No CLI/flag jargon in user-facing narration; technical detail (commands, paths) folded away but available. Explanations must work read aloud. |
| Lifecycle | Setup-time, one shot. (On-demand review / continuous health check deferred.) |
| First wave scope | Hardening + full restructure vision ships together. |

## Resolving the one tension: fast pass vs. gate-before-mutation

The fast first pass must be **non-mutating** (read-only diagnosis + additive-only steps: init, index, proof note from an already-good source file). The consent gate then presents the restructure plan; on yes, the restructure runs as the background deepening phase and reports back. So the sequencing is:

1. **Harden + init (seconds).** Version check (pre-0.5.8 contract warning), minted-key auth, no env-key pickup, no hooks written.
2. **Fast diagnosis (<2 min).** Sampled scan + representative file reads (never trust scan counts alone — scan blindness to non-Markdown corpora is a confirmed failure mode). Produces the four-tier map: *already usable / weakly indexable / would improve retrieval / can wait*.
3. **Proof note.** Distill one real note from the user's best existing content and place it correctly. This is the wow moment and it happens before any mutation.
4. **The gate.** One narrated moment: here's what your vault does well, here's where Enzyme can't leverage it yet and why (each finding transitions into the doctrine principle it violates), here's exactly what I'd move/stamp, here's the backup, here's the one-command revert. Single yes.
5. **Background restructure.** Moves + frontmatter batch, executed against the backup contract below, reporting back with a before/after in the four-tier frame when done.
6. **Healthy-vault branch.** If step 2 finds nothing above the "can wait" tier: say so, show the map, still do step 3, skip 4–5.

## The two value paths, braided

From the interview: the class-3/4 value is (a) diagnose-and-restructure with education as the trust mechanism, and (b) teaching what compounding-friendly structure *is*, independent of Enzyme mechanics. The skill braids them: every vault-specific failure-mode observation in the diagnosis narration must land on the general structural principle behind it ("these 40 meeting notes live in six folders — Enzyme compounds on knowledge when related notes share a home and a frontmatter contract, so retrieval can stack them"). The user should come away understanding their vault's structure, not just Enzyme's needs.

## Backup + revert contract

- Before any mutation: full sidecar copy of affected files + a machine-readable manifest (every move, every frontmatter diff) under `.margins/restructure/<timestamp>/`.
- Revert is a single generated script that replays the manifest in reverse and restores frontmatter byte-for-byte from the sidecar; it must leave the vault checksum-identical to pre-restructure (verify with `find`+shasum snapshot taken before mutation).
- The shasum snapshot doubles as the **mutation guard** for the read-only phases: diagnosis must produce zero filesystem residue (the `enzyme scan` → `.enzyme/enzyme.log` leak is a confirmed failure mode).
- If the vault is a git repo, additionally commit a checkpoint — but never rely on git being present.

## Doctrine derivation (prerequisite work)

Mine enzyme-rust for how scan/catalyze actually work — chunking, frontmatter use, link/entity weighting, retrieval scoring — and distill 3–5 structural principles, each mechanically justified ("Enzyme weights X, therefore structure Y compounds"). Known internals-driven findings to fold in: entity weighting dominated by noise folders (Readwise/daily/packages), templates outranking real notes in semantic search, non-Markdown blindness. Josh reviews the drafted doctrine before it enters the skill.

## Failure modes this design must retire (from the consolidation taxonomy)

- **A (credentials):** bare/stale env key hijacking init → retired by minted-key default + `--use-env-llm` gate.
- **B (version contract):** pre-0.5.8 guard already live; keep, verify behaviorally.
- **C (pollution):** hooks dead (verify residual references); scan residue → mutation guard; `.margins` visibility handled by ignore stamping during restructure.
- **D (vault-shape blindness):** representative-file reads mandatory after scan; four-tier map is the output contract.
- **E (communication):** four-tier framing is the reasoning backbone; no boilerplate narratives (raw-token `rg` gate stays); confidence honesty — when evidence doesn't pick a winner destination, the gate presents the ambiguity instead of collapsing it.

## Verification plan (from consolidation §6 — don't rebuild the judge machine)

- Keep ~6 high-signal fixtures + **both controls**: fully-configured vault (must get "no action needed" + proof note) and append-only-dated-log (must attract ~zero findings). Resolve the `partial-enzyme-or-aside` / `-or-margins` naming split first.
- Gate promotion on **live reruns** of the real skill against fixtures — never artifact regeneration.
- Automate: shasum mutation guard, raw-token `rg` gate in `make check-agent-skill`, fixture-key harness so there is finally a known-good catalyze baseline, revert-script round-trip test (restructure → revert → checksum-identical).
- Single serial judge with a tight rubric; no parallel judge fan-out.
- On green: promote to canonical `plugin/plugins/enzyme/skills/enzyme/SKILL.md`, `make sync-agent-skill && make check-agent-skill`, extend the Rust content-assertion tests with anchors for the promoted text.

## Open items

- Fix or stub the cidre/Xcode sandbox blocker so bundled-skill content tests run in-loop.
- Login-gate under isolated HOME (no `--config-dir`) is still an Enzyme CLI gap, not a skill-text gap.
- On-demand "review my vault" and refresh-lifecycle integration: explicitly deferred to a later wave.
