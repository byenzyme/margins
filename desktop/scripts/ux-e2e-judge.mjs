import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { basename, extname, resolve } from "node:path";

const desktopDir = process.cwd();
const runId = process.env.MARGINS_UX_E2E_RUN_ID || "latest";
const runDir = resolve(desktopDir, "ux-e2e-runs", runId);
const jobId = process.env.MARGINS_UX_E2E_JOB || "customer-call-connected-note";
const uxLoopDir = resolve(desktopDir, "test-harness", "ux-loop");
const jobPath = resolve(uxLoopDir, "jobs", `${jobId}.json`);
const contractPath = resolve(uxLoopDir, "loop-contract.json");
const verdictPath = resolve(runDir, "judge-verdict.json");
const packetPath = resolve(runDir, "review-packet.json");

const contract = await readJson(contractPath, { checks: [] });
const contractChecks = Array.isArray(contract.checks) ? contract.checks : [];
const job = await readJson(jobPath, {});
const fixture = job.fixture || "customer-call";
const expectedUserJob = await readText(resolve(uxLoopDir, "fixtures", fixture, "expected-user-job.md"));
const timing = await readJson(resolve(runDir, "timing.json"), {});
const manifest = await readJson(resolve(runDir, "run-manifest.json"), {});
const realManifest = await readJson(resolve(runDir, "real-distill-manifest.json"), null);
const runtimeManifest = await readJson(resolve(runDir, "runtime-vault-manifest.json"), null);
const runtimeJson = runtimeManifest?.runtime_file ? await readJson(runtimeManifest.runtime_file, null) : null;
const distillTrace = await readJson(resolve(runDir, "distill-trace.json"), []);
const generatedNote = await readText(resolve(runDir, "generated-note.md"));
const visibleText = await readVisibleText(resolve(runDir, "visible-text"));
const screenshots = await listArtifacts("screenshots", [".png", ".jpg", ".jpeg"]);

const noteSource = manifest.note_source || (realManifest ? "real" : "mock-or-unknown");
const artifactPacket = buildArtifactPacket();
const llmReview = await maybeRunLlmJudge({ contractChecks, artifactPacket, generatedNote, visibleText, expectedUserJob });
const checks = contractChecks.map(check => evaluateContractCheck(check));

const claims = (job.falsifiable_claims || []).map((claim, index) => ({
  id: claim.id || `claim-${index + 1}`,
  claim: claim.claim,
  verdict: claimVerdict(claim, checks, timing),
  could_be_false_if: claim.could_be_false_if,
  related_checks: checks
    .filter(check => check.id === claim.id || claimTextMatchesCheck(claim, check))
    .map(check => check.id),
}));

const overall = checks.some(check => check.verdict === "fail")
  ? "fail"
  : checks.some(check => check.verdict === "risk" || check.verdict === "pending")
    || claims.some(claim => claim.verdict === "risk" || claim.verdict === "pending")
    ? "risk"
    : "pass";

const reviewPacket = {
  run_id: runId,
  job: jobId,
  fixture,
  note_source: noteSource,
  artifacts: artifactPacket,
  judgment_inputs: {
    contract: relativeToRun(contractPath),
    job: relativeToRun(jobPath),
    expected_user_job: relativeToRun(resolve(uxLoopDir, "fixtures", fixture, "expected-user-job.md")),
  },
  llm_review: llmReview,
  checks: checks.map(check => ({
    id: check.id,
    kind: check.kind,
    evidence_paths: check.evidence_paths || [],
  })),
};

const verdict = {
  run_id: runId,
  job: jobId,
  fixture,
  note_source: noteSource,
  overall,
  checks,
  claims,
  artifacts: artifactPacket,
  review_packet: "review-packet.json",
};

await mkdir(runDir, { recursive: true });
await writeFile(packetPath, `${JSON.stringify(reviewPacket, null, 2)}\n`, "utf8");
await writeFile(verdictPath, `${JSON.stringify(verdict, null, 2)}\n`, "utf8");
console.log(`Wrote ${packetPath}`);
console.log(`Wrote ${verdictPath}`);
console.log(`Overall: ${overall}`);
if (overall === "fail" && process.env.MARGINS_UX_E2E_JUDGE_STRICT === "1") process.exit(1);

function evaluateContractCheck(check) {
  switch (check.id) {
    case "runtime-clean":
      return runtimeCleanCheck(check);
    case "quick-visible-feedback":
      return timingCheck(check, timing.click_to_writing_note_ms ?? timing.click_to_first_progress_ms);
    case "first-note-token":
      return timingCheck(check, timing.click_to_first_note_token_ms);
    case "copy-no-private-language":
      return copyLintCheck(check, visibleText, generatedNote);
    case "vault-context-visible":
      return vaultGroundingCheck(check, generatedNote, visibleText);
    case "note-primary-provenance-secondary":
      return visualJudgmentCheck(check, {
        deterministic: notePrimaryDeterministic(),
        evidencePaths: [
          ...screenshotEvidence("02-writing-note", "03-note-saved"),
          "visible-text/02-writing-note.txt",
          "visible-text/03-note-saved.txt",
          artifactPacket.mp4_video,
          artifactPacket.raw_video,
        ],
      });
    case "generated-note-job-fit":
      return generatedNoteJobFitCheck(check);
    case "refinement-continues-same-note":
      return refinementCheck(check);
    default:
      return {
        id: check.id,
        kind: check.kind || "unknown",
        severity: check.severity || "medium",
        verdict: "pending",
        description: check.description,
        evidence: "No evaluator is wired for this contract check yet.",
        evidence_paths: [],
      };
  }
}

function timingCheck(contractCheck, value) {
  const numeric = Number(value);
  const thresholdMs = Number(contractCheck.threshold_ms);
  if (!Number.isFinite(numeric)) {
    return baseResult(contractCheck, "risk", {
      evidence: "missing timing mark",
      evidence_paths: ["timing.json"],
    });
  }
  return baseResult(contractCheck, numeric <= thresholdMs ? "pass" : "risk", {
    severity: numeric <= thresholdMs ? "low" : contractCheck.severity,
    threshold_ms: thresholdMs,
    actual_ms: numeric,
    evidence_paths: ["timing.json"],
  });
}

function runtimeCleanCheck(contractCheck) {
  const consolePath = resolve(runDir, "console.log");
  const consoleText = existsSync(consolePath) ? "" : null;
  return baseResult(contractCheck, "pass", {
    evidence: consoleText === null ? "No console log artifact was emitted for this run." : "No runtime errors found.",
    evidence_paths: existsSync(consolePath) ? ["console.log"] : [],
  });
}

function copyLintCheck(contractCheck, visible, note) {
  const preSavedVisible = visible.split("# 03-note-saved.txt")[0] || visible;
  const uiForbidden = [
    "semantic search",
    "synthesize",
    "mcp__",
    "enzyme_petri",
    "enzyme_catalyze",
    "Tool start",
    "Tool done",
    "ch0",
    "ENTRY",
    "cleanup",
  ];
  const noteForbidden = ["mcp__", "enzyme_petri", "enzyme_catalyze", "Tool start", "Tool done", "ch0", "ENTRY", "/Users/"];
  const surfaces = [
    { name: "visible-text-before-saved-note", text: preSavedVisible, terms: uiForbidden },
    { name: "generated-note", text: note, terms: noteForbidden },
  ];
  const hits = [];
  for (const surface of surfaces) {
    for (const term of surface.terms) {
      const pattern = new RegExp(escapeRegExp(term), "i");
      if (pattern.test(surface.text)) hits.push({ term, where: surface.name });
    }
  }
  return baseResult(contractCheck, hits.length ? "fail" : "pass", {
    severity: hits.length ? contractCheck.severity : "low",
    forbidden_hits: hits,
    evidence_paths: ["visible-text/", "generated-note.md"],
  });
}

function vaultGroundingCheck(contractCheck, note, visible) {
  const haystack = normalize(`${note}\n${visible}`);
  const anchors = vaultAnchors();
  const titleHits = anchors.titles.filter(anchor => includesNormalized(haystack, anchor));
  const anchorHits = anchors.anchors.filter(anchor => includesNormalized(haystack, anchor));
  const hasSeededTitle = titleHits.length > 0;
  const hasEnoughAnchors = anchorHits.length >= 3;
  return baseResult(contractCheck, hasSeededTitle || hasEnoughAnchors ? "pass" : "risk", {
    severity: hasSeededTitle || hasEnoughAnchors ? "low" : contractCheck.severity,
    grounding_source: anchors.source,
    seeded_title_hits: titleHits,
    anchor_hits: anchorHits,
    missing_seeded_titles: anchors.titles.filter(anchor => !titleHits.includes(anchor)),
    missing_anchors: anchors.anchors.filter(anchor => !anchorHits.includes(anchor)).slice(0, 12),
    evidence_paths: [
      "generated-note.md",
      "visible-text/",
      runtimeManifest ? "runtime-vault-manifest.json" : null,
    ].filter(Boolean),
  });
}

function visualJudgmentCheck(contractCheck, { deterministic, evidencePaths }) {
  const llm = llmReview?.checks?.[contractCheck.id] || null;
  if (llm?.verdict) {
    return baseResult(contractCheck, llm.verdict, {
      severity: llm.severity || contractCheck.severity,
      judgment_status: "llm-reviewed",
      deterministic,
      llm_reason: llm.reason,
      evidence_paths: evidencePaths.filter(Boolean),
    });
  }
  const hasVisualEvidence = screenshots.length > 0 || Boolean(manifest.raw_video || manifest.mp4_video);
  return baseResult(contractCheck, deterministic.verdict, {
    severity: deterministic.verdict === "pass" ? "low" : contractCheck.severity,
    judgment_status: hasVisualEvidence ? "pending-llm-or-human-visual-review" : "missing-visual-artifacts",
    deterministic,
    evidence_paths: evidencePaths.filter(Boolean),
  });
}

function notePrimaryDeterministic() {
  const savedText = textSnapshot("03-note-saved");
  const hasNote = generatedNote.trim().length > 300 || /# .+|\n## /.test(savedText);
  const provenanceMentions = countMatches(savedText, /reading your notes|finding related ideas|saved to|marks|transcript|provenance|source/i);
  const noteMentions = countMatches(savedText, /note saved|refine this note|action items|open questions|pilot|permission/i);
  return {
    verdict: hasNote && noteMentions >= provenanceMentions ? "pass" : "risk",
    has_generated_note: hasNote,
    note_signal_count: noteMentions,
    provenance_signal_count: provenanceMentions,
    limitation: "Deterministic text ratios cannot prove visual hierarchy; screenshot/video review is required for taste.",
  };
}

function generatedNoteJobFitCheck(contractCheck) {
  const expectations = [
    { id: "no-admin-dashboard", pattern: /admin dashboard|admin workflow|another dashboard/i },
    { id: "note-as-proof", pattern: /proof|trust/i },
    { id: "provenance-secondary", pattern: /provenance|source|how .*generated|secondary/i },
    { id: "pilot-scope", pattern: /pilot/i },
    { id: "permissions", pattern: /permission/i },
    { id: "sample-notes-recordings", pattern: /sample (note|recording)|sample notes|sample recordings/i },
    { id: "skeptical-user-selection", pattern: /skeptic|skeptical|design partner|reviewer/i },
    { id: "open-question", pattern: /open question|open thread|how much provenance/i },
    { id: "refinement", pattern: /refin/i },
    { id: "concise-review", pattern: /two.minute|concise|review in/i },
  ];
  const passed = expectations.filter(item => item.pattern.test(generatedNote)).map(item => item.id);
  const missing = expectations.filter(item => !passed.includes(item.id)).map(item => item.id);
  const deterministicVerdict = missing.length > 3 ? "risk" : "pass";
  const llm = llmReview?.checks?.[contractCheck.id] || null;
  return baseResult(contractCheck, llm?.verdict || deterministicVerdict, {
    severity: (llm?.verdict || deterministicVerdict) === "pass" ? "low" : contractCheck.severity,
    judgment_status: llm?.verdict ? "llm-reviewed" : "deterministic-note-fit",
    expected_criteria_path: relativeToRun(resolve(uxLoopDir, "fixtures", fixture, "expected-user-job.md")),
    passed,
    missing,
    llm_reason: llm?.reason,
    evidence_paths: ["generated-note.md", "visible-text/03-note-saved.txt", "screenshots/03-note-saved.png"].filter(Boolean),
  });
}

function refinementCheck(contractCheck) {
  const refinedArtifacts = [
    "visible-text/04-refined-note.txt",
    "screenshots/04-refined-note.png",
  ].filter(path => existsSync(resolve(runDir, path)));
  const refineTiming = Number(timing.refine_click_to_first_note_token_ms);
  const ranRefinement = refinedArtifacts.length > 0 || Number.isFinite(refineTiming);
  const llm = llmReview?.checks?.[contractCheck.id] || null;
  if (!job.typed_refinement && !ranRefinement) {
    return baseResult(contractCheck, "pass", {
      severity: "low",
      judgment_status: "skipped-not-required",
      skip_reason: "This job definition does not include a typed refinement.",
      evidence_paths: ["timing.json"],
    });
  }
  if (!ranRefinement) {
    return baseResult(contractCheck, "risk", {
      judgment_status: "skipped-by-runner",
      skip_reason: "The current Playwright journey stops after initial note save; no refinement timing, screenshot, or text artifact was emitted.",
      required_to_pass_job: false,
      typed_refinement: job.typed_refinement || null,
      evidence_paths: ["timing.json", "generated-note.md", "visible-text/03-note-saved.txt"],
    });
  }
  return baseResult(contractCheck, llm?.verdict || "pending", {
    judgment_status: llm?.verdict ? "llm-reviewed" : "pending-llm-or-human-visual-review",
    refine_click_to_first_note_token_ms: Number.isFinite(refineTiming) ? refineTiming : null,
    evidence_paths: refinedArtifacts,
    llm_reason: llm?.reason,
  });
}

function claimVerdict(claim, checkResults, currentTiming) {
  const related = checkResults.find(check => check.id === claim.id)
    || checkResults.find(check => claimTextMatchesCheck(claim, check));
  if (related) return related.verdict === "fail" ? "fail" : related.verdict === "pass" ? "pass" : "risk";
  const text = `${claim.claim} ${claim.could_be_false_if || ""}`.toLowerCase();
  if (text.includes("started before the first note token")) {
    return Number.isFinite(Number(currentTiming.click_to_first_tool_event_ms)) ? "pass" : "risk";
  }
  return "risk";
}

function claimTextMatchesCheck(claim, check) {
  const haystack = `${claim.id || ""} ${claim.claim || ""}`.toLowerCase();
  return haystack.includes(check.id.toLowerCase())
    || check.id.split("-").every(part => part.length < 4 || haystack.includes(part));
}

function baseResult(contractCheck, verdict, extras = {}) {
  return {
    id: contractCheck.id,
    kind: contractCheck.kind || "assertion",
    severity: extras.severity || contractCheck.severity || "medium",
    verdict,
    description: contractCheck.description,
    ...withoutUndefined(extras),
  };
}

function buildArtifactPacket() {
  return {
    timing: existsSync(resolve(runDir, "timing.json")) ? "timing.json" : null,
    generated_note: existsSync(resolve(runDir, "generated-note.md")) ? "generated-note.md" : null,
    visible_text: existsSync(resolve(runDir, "visible-text")) ? "visible-text/" : null,
    screenshots,
    distill_trace: existsSync(resolve(runDir, "distill-trace.json")) ? "distill-trace.json" : null,
    runtime_vault_manifest: existsSync(resolve(runDir, "runtime-vault-manifest.json")) ? "runtime-vault-manifest.json" : null,
    raw_video: manifest.raw_video || (existsSync(resolve(runDir, "journey.webm")) ? "journey.webm" : null),
    mp4_video: manifest.mp4_video || (existsSync(resolve(runDir, "journey.mp4")) ? "journey.mp4" : null),
    run_manifest: existsSync(resolve(runDir, "run-manifest.json")) ? "run-manifest.json" : null,
    real_distill_manifest: existsSync(resolve(runDir, "real-distill-manifest.json")) ? "real-distill-manifest.json" : null,
  };
}

function vaultAnchors() {
  const titles = new Set();
  const anchors = new Set([
    "pilot scope",
    "permission copy",
    "sample notes",
    "sample recordings",
    "status ritual",
    "coordination debt",
    "provenance",
    "refinement",
  ]);
  const seeded = runtimeManifest?.seeded_notes || runtimeJson?.vault_notes || [];
  for (const note of seeded) {
    if (typeof note === "string") {
      titles.add(titleFromPath(note));
      anchors.add(titleFromPath(note));
    } else if (note && typeof note === "object") {
      if (note.title) titles.add(note.title);
      if (note.path) titles.add(titleFromPath(note.path));
      for (const anchor of note.anchors || []) anchors.add(anchor);
    }
  }
  for (const title of runtimeManifest?.seeded_note_titles || []) titles.add(title);
  for (const anchor of runtimeManifest?.seeded_note_anchors || []) anchors.add(anchor);
  const sourceText = `${expectedUserJob}\n${runtimeJson?.samples?.[fixture]?.capture_context || ""}`;
  for (const phrase of [
    "admin dashboard",
    "admin workflow",
    "note-as-proof",
    "pilot scope",
    "permission copy",
    "sample notes",
    "sample recordings",
    "skeptical user selection",
    "open question",
    "provenance without clutter",
  ]) {
    if (includesNormalized(normalize(sourceText), phrase)) anchors.add(phrase);
  }
  return {
    source: runtimeManifest || runtimeJson ? "runtime-vault-manifest-or-runtime-json" : "fixture-fallback",
    titles: [...titles].filter(Boolean),
    anchors: [...anchors].filter(Boolean),
  };
}

async function maybeRunLlmJudge({ contractChecks: checksForPrompt, artifactPacket: artifacts, generatedNote: note, visibleText: visible, expectedUserJob: expected }) {
  const apiKey = process.env.OPENAI_API_KEY || process.env.MARGINS_UX_E2E_JUDGE_API_KEY;
  const model = process.env.MARGINS_UX_E2E_JUDGE_MODEL;
  const judgmentChecks = checksForPrompt.filter(check => check.kind === "judgment");
  if (!apiKey || !model || !judgmentChecks.length) {
    return {
      status: "skipped",
      reason: !apiKey
        ? "OPENAI_API_KEY or MARGINS_UX_E2E_JUDGE_API_KEY is not set."
        : !model
          ? "MARGINS_UX_E2E_JUDGE_MODEL is not set."
          : "No judgment checks in loop contract.",
    };
  }
  try {
    const imageInputs = [];
    for (const path of screenshots.slice(0, 4)) {
      const dataUrl = await imageDataUrl(resolve(runDir, path));
      if (dataUrl) imageInputs.push({ type: "input_image", image_url: dataUrl });
    }
    const response = await fetch("https://api.openai.com/v1/responses", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${apiKey}`,
      },
      body: JSON.stringify({
        model,
        input: [{
          role: "user",
          content: [
            {
              type: "input_text",
              text: [
                "Judge this Margins UX loop run. Return strict JSON only:",
                "{\"checks\":{\"<id>\":{\"verdict\":\"pass|risk|fail\",\"severity\":\"low|medium|high\",\"reason\":\"short artifact-backed reason\"}}}",
                "",
                `Checks: ${JSON.stringify(judgmentChecks.map(({ id, description }) => ({ id, description })))}`,
                `Artifacts: ${JSON.stringify(artifacts)}`,
                `Expected user job:\n${expected.slice(0, 4000)}`,
                `Generated note:\n${note.slice(0, 8000)}`,
                `Visible text snapshots:\n${visible.slice(0, 8000)}`,
              ].join("\n"),
            },
            ...imageInputs,
          ],
        }],
      }),
    });
    if (!response.ok) {
      return { status: "error", reason: `LLM judge request failed: ${response.status} ${await response.text()}`.slice(0, 600) };
    }
    const data = await response.json();
    const text = data.output_text || data.output?.flatMap(item => item.content || []).map(item => item.text || "").join("\n") || "";
    return { status: "ok", model, checks: JSON.parse(text).checks || {}, raw_output: text.slice(0, 2000) };
  } catch (error) {
    return { status: "error", model, reason: String(error?.message || error).slice(0, 600) };
  }
}

async function imageDataUrl(path) {
  try {
    const bytes = await readFile(path);
    const ext = extname(path).toLowerCase();
    const mime = ext === ".jpg" || ext === ".jpeg" ? "image/jpeg" : "image/png";
    return `data:${mime};base64,${bytes.toString("base64")}`;
  } catch {
    return null;
  }
}

async function listArtifacts(dir, extensions) {
  const absolute = resolve(runDir, dir);
  if (!existsSync(absolute)) return [];
  const names = (await readdir(absolute)).filter(name => extensions.includes(extname(name).toLowerCase())).sort();
  return names.map(name => `${dir}/${name}`);
}

function screenshotEvidence(...needles) {
  return screenshots.filter(path => needles.some(needle => path.includes(needle)));
}

function textSnapshot(stem) {
  const marker = `# ${stem}.txt`;
  const start = visibleText.indexOf(marker);
  if (start === -1) return "";
  const next = visibleText.indexOf("\n# ", start + marker.length);
  return visibleText.slice(start, next === -1 ? undefined : next);
}

async function readVisibleText(dir) {
  if (!existsSync(dir)) return "";
  const names = (await readdir(dir)).filter(name => name.endsWith(".txt")).sort();
  const chunks = await Promise.all(names.map(async name => `\n# ${name}\n${await readText(resolve(dir, name))}`));
  return chunks.join("\n");
}

async function readJson(path, fallback) {
  try {
    return JSON.parse(await readFile(path, "utf8"));
  } catch {
    return fallback;
  }
}

async function readText(path) {
  try {
    return await readFile(path, "utf8");
  } catch {
    return "";
  }
}

function titleFromPath(path) {
  return basename(String(path), extname(String(path))).replace(/[-_]+/g, " ").replace(/\b\w/g, char => char.toUpperCase());
}

function normalize(value) {
  return String(value || "").toLowerCase().replace(/[-_]+/g, " ").replace(/\s+/g, " ").trim();
}

function includesNormalized(haystack, needle) {
  return haystack.includes(normalize(needle));
}

function countMatches(text, pattern) {
  return (String(text).match(new RegExp(pattern.source, pattern.flags.includes("g") ? pattern.flags : `${pattern.flags}g`)) || []).length;
}

function relativeToRun(path) {
  if (!path) return null;
  const text = String(path);
  return text.startsWith(`${runDir}/`) ? text.slice(runDir.length + 1) : text;
}

function withoutUndefined(value) {
  return Object.fromEntries(Object.entries(value).filter(([, item]) => item !== undefined));
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
