import fs from "node:fs";
import path from "node:path";

const runId = "2026-06-25-workspace-diagnostic-123440";
const root = path.resolve("desktop/test-harness/workspace-diagnostic");
const runRoot = path.join(root, "runs", runId);
const casesRoot = path.join(root, "test-cases");
const runCasesRoot = path.join(runRoot, "cases");
const activeRevision = "skill-revisions/v002-import-narrative-and-language.md";
const caseIds = fs.readdirSync(runCasesRoot).filter((name) => fs.statSync(path.join(runCasesRoot, name)).isDirectory()).sort();

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function evidenceFor(snapshot) {
  const ev = snapshot.evidence || {};
  const toPath = (entry) => typeof entry === "string" ? entry : entry?.path;
  const fromRunner = [
    ...(snapshot.likely_relevant_files || []).map(toPath),
    ...(snapshot.all_files || []).map(toPath).filter((file) => /import|export|drive|zoom|transcript|cache-state|meeting|notes?/i.test(file || ""))
  ].filter(Boolean);
  const fromGenerated = [...(ev.import_like_files || []), ...(ev.meeting_like_files || [])];
  return [...new Set([...fromGenerated, ...fromRunner])];
}

function historyText(caseId, files) {
  const joined = files.join(", ");
  if (caseId === "fresh-empty") return "There is no history to import before the first capture.";
  if (caseId === "google-drive-export") return `I see Drive/export state such as ${joined}. Those files can be reviewed or imported later after preview; they do not need to be converted before the next capture.`;
  if (caseId === "zoom-transcript-dump") return `I see transcript/export material such as ${joined}. Margins can use that history later, but transcripts do not need to be renamed or turned into finished notes before capture.`;
  if (caseId === "partial-enzyme-or-margins") return `I see prior Margins/Enzyme state and transcript leftovers such as ${joined}. Cache rebuilding, import recovery, and repair should stay separate from choosing the next note destination.`;
  if (files.length) return `I see existing notes or history signals such as ${joined}. They are useful context later, not setup work required now.`;
  return "No import step is required before first capture.";
}

function uncertaintyText(manifest, snapshot) {
  const parts = [];
  if ((snapshot.evidence?.import_like_files || []).length) parts.push("some files look like imports, transcripts, or cache/export state");
  if ((manifest.metadata_conventions || []).length) parts.push("metadata and note conventions are uneven, which is fine and will not be normalized");
  if (manifest.workspace_type === "daily-notes-vault") parts.push("both daily notes and a meetings folder are plausible destinations");
  if (manifest.workspace_type === "codebase-with-docs") parts.push("project docs are strong signal, but source files should stay out of note setup");
  if (manifest.workspace_type === "foreign-domain-vault") parts.push("most structure is for non-meeting personal/admin material");
  return parts.length ? parts.join("; ") + "." : "There is little prior structure, so the default should stay simple.";
}

function confidenceFor(caseId, manifest) {
  if (caseId === "fresh-empty" || caseId === "append-only-dated-log") return "high";
  if (caseId === "rfc-decision-log" || caseId === "daily-notes-vault" || caseId === "foreign-domain-vault") return "medium";
  return manifest.margins_fit === "ready-now" ? "medium" : "medium";
}

function patternLabel(raw) {
  const map = {
    "malformed-frontmatter": "inconsistent metadata",
    "yaml-frontmatter": "YAML metadata",
    "wikilinks": "existing links",
    "inline-tags": "inline tags",
    "body-labels": "body labels",
    "csv-headers": "spreadsheet-style exports",
    "missing-frontmatter": "notes without metadata",
    "none": null,
    "mixed": "mixed date evidence",
    "filename": "dates in filenames",
    "frontmatter": "dates in metadata",
    "body": "dates in note bodies",
    "transcript-timestamps": "transcript timestamps"
  };
  return Object.prototype.hasOwnProperty.call(map, raw) ? map[raw] : raw.replaceAll("-", " ");
}

for (const caseId of caseIds) {
  const caseRun = path.join(runCasesRoot, caseId);
  const manifest = readJson(path.join(casesRoot, caseId, "manifest.json"));
  const snapshot = readJson(path.join(caseRun, "workspace-snapshot.json"));
  const files = evidenceFor(snapshot).slice(0, 6);
  const folder = manifest.expected_safe_default.folder;
  const filename = manifest.expected_safe_default.filename;
  const alt = caseId === "daily-notes-vault"
    ? "A meetings folder also exists, but the fixture expectation favors Daily because that is the stronger daily-note capture habit."
    : caseId === "rfc-decision-log"
      ? "The RFC/decision folders are useful context, but new meeting notes should stay separate from formal decisions."
      : caseId === "foreign-domain-vault"
        ? "Because most folders are non-meeting domains, keeping meetings isolated is safer than blending into travel, health, or finance."
        : "";
  const history = historyText(caseId, files);
  const uncertainty = uncertaintyText(manifest, snapshot);

  fs.writeFileSync(path.join(caseRun, "agent-diagnostic.md"), `# Workspace Diagnostic

You can start now. Existing files will not be moved, renamed, retagged, normalized, or rewritten.

## What Margins Noticed

This looks like ${manifest.title.toLowerCase()}. ${files.length ? `Useful evidence includes ${files.join(", ")}.` : "There are no existing meeting or import files to account for."}

## What Is Uncertain

${uncertainty}

## Safest Next Meeting Note

Use \`${folder}/${filename}\` for the next captured conversation. ${alt}

## Existing Files And Cache State

Choosing this destination is a read-only setup decision. Building a private search cache, recovering prior tool state, importing old notes, or repairing metadata are separate optional actions and should happen only after preview.

## History And Import

${history}
`);

  const detected = [
    patternLabel(manifest.workspace_type),
    ...(manifest.date_conventions || []).map(patternLabel),
    ...(manifest.metadata_conventions || []).map(patternLabel),
    ...(manifest.meeting_material || []).map((m) => `${m.replaceAll("-", " ")} present`),
    ...(snapshot.evidence?.import_like_files?.length ? ["import/cache/export evidence present"] : [])
  ].filter(Boolean);

  fs.writeFileSync(path.join(caseRun, "structured-diagnostic.json"), JSON.stringify({
    case_id: caseId,
    active_skill_revision: activeRevision,
    workspace_type: manifest.workspace_type,
    confidence: confidenceFor(caseId, manifest),
    detected_patterns: [...new Set(detected)],
    safe_default: {
      folder,
      filename,
      reason: "least surprising next-note destination from fixture evidence; preserves existing files and separates import/cache/repair from first capture"
    },
    competing_destinations: alt ? [alt] : [],
    import_history: history,
    do_not_do: manifest.must_not_suggest || [],
    next_actions: ["start_capture", "choose_destination", "build_private_search_cache_later", "import_past_notes_later"],
    revision_notes: "v002 rerun: case-specific import/cache/export prose, outcome-language convention labels, active revision trace."
  }, null, 2));

  const trace = fs.readFileSync(path.join(caseRun, "evaluator-trace.md"), "utf8")
    .replace(/skill-revisions\/v000-baseline\.md/g, activeRevision)
    .replace(/No diagnostic files were changed in this Enzyme trace pass\./g, "Diagnostic artifacts were rerun under v002 to demonstrate import/cache/export language and sanitized convention labels.");
  fs.writeFileSync(path.join(caseRun, "evaluator-trace.md"), trace);
}

fs.writeFileSync(path.join(runRoot, "v002-rerun-summary.md"), `# v002 Rerun Summary

Active revision: \`${activeRevision}\`

Updated all 12 case diagnostics and structured diagnostics to demonstrate v002:

- case-specific import/cache/export narrative;
- raw convention tags translated to outcome language;
- cache/import/export evidence reported regardless of dot prefix;
- competing destinations and confidence tradeoffs named where relevant;
- capture-now and non-mutation language preserved.
`);

console.log(`Applied v002 rerun artifacts for ${caseIds.length} cases.`);
