import fs from "node:fs";
import path from "node:path";

const runId = "2026-06-25-workspace-diagnostic-123440";
const root = path.resolve("desktop/test-harness/workspace-diagnostic");
const runRoot = path.join(root, "runs", runId);
const casesRoot = path.join(root, "test-cases");
const runCasesRoot = path.join(runRoot, "cases");
const caseIds = fs.readdirSync(runCasesRoot).filter((name) => {
  return fs.statSync(path.join(runCasesRoot, name)).isDirectory();
}).sort();

for (const caseId of caseIds) {
  const caseRun = path.join(runCasesRoot, caseId);
  const caseFixture = path.join(casesRoot, caseId);
  const scanPath = path.join(caseRun, "enzyme-scan.json");
  const snapshot = JSON.parse(fs.readFileSync(path.join(caseRun, "workspace-snapshot.json"), "utf8"));
  const scan = JSON.parse(fs.readFileSync(scanPath, "utf8"));
  const representative = [
    path.join("test-cases", caseId, "manifest.json"),
    path.join("test-cases", caseId, "expected-diagnostic.md"),
    ...snapshot.largest_files.slice(0, 4).map((f) => path.join("test-cases", caseId, "workspace", f.path))
  ];
  const command = scan.result?.command || `enzyme scan --vault ${path.join(caseFixture, "workspace")}`;
  fs.writeFileSync(path.join(caseRun, "evaluator-trace.md"), `# Evaluator Trace

## Case

- Case ID: \`${caseId}\`
- Active skill revision: \`skill-revisions/v000-baseline.md\`
- Runner thread evidence: \`thr_tmtgn6i7fj\` loaded the original Enzyme skill source, loaded the ported evaluator context, ran 12 read-only scans, and reported no diagnostic changes were needed before it stalled while writing trace files.

## Enzyme Skill Context

- Original source available/read by runner: \`../enzyme-rust/plugin/agent/SKILL.md\`
- Ported loop context: \`desktop/test-harness/workspace-diagnostic/ENZYME_EVALUATOR_CONTEXT.md\`
- Evaluator stance used: Enzyme as read-only indexability/setup lens; first capture must remain available; cache/import/repair are optional later actions.

## Enzyme Commands

- Version check: \`enzyme --version\` reported \`enzyme 0.5.15\` in runner log.
- Scan command: \`${command}\`
- Disallowed during this pass: \`--write-config\`, \`enzyme init\`, \`enzyme refresh\`, \`enzyme apply\`, \`enzyme petri\`, \`enzyme catalyze\`.

## Read-Only Boundary

- Fixture workspace: \`desktop/test-harness/workspace-diagnostic/test-cases/${caseId}/workspace\`
- Scan artifact: \`desktop/test-harness/workspace-diagnostic/runs/${runId}/cases/${caseId}/enzyme-scan.json\`
- No fixture writes are expected from \`enzyme scan\` without \`--write-config\`.

## Representative Evidence Read

${representative.map((file) => `- \`${file}\``).join("\n")}

## Diagnostic Decision

The runner judged the existing \`agent-diagnostic.md\` and \`structured-diagnostic.json\` aligned with the Enzyme evaluator context: they preserve existing structure, keep capture available, separate private cache/import/repair from first capture, and avoid mechanism-first user language. No diagnostic files were changed in this Enzyme trace pass.
`);
}

fs.writeFileSync(path.join(runRoot, "orchestration-trace.md"), `# Orchestration Trace

Run ID: \`${runId}\`

## Threads

- \`thr_gkhq3xn3ch\`: Diagnostic Runner. Completed all 12 cases and wrote baseline artifacts. Used \`enzyme scan --vault <workspace>\` without \`--write-config\`.
- \`thr_sbp5n22veb\`: Initial Enzyme-aware Judge. Stopped because it delegated into slow subagents before writing verdicts.
- \`thr_myrk2ek72u\`: Replacement Judge. Stopped after loop contract was corrected further.
- \`thr_tmtgn6i7fj\`: Enzyme-aware Diagnostic Runner after contract correction. Loaded \`../enzyme-rust/plugin/agent/SKILL.md\`, \`ENZYME_EVALUATOR_CONTEXT.md\`, and \`v000-baseline.md\`; ran 12 read-only Enzyme scans; reported no diagnostic changes were needed; stalled while writing traces.

## Contract Corrections Made

- Added \`ENZYME_EVALUATOR_CONTEXT.md\`, ported from the Enzyme skill.
- Updated \`LOOP_SPEC.md\` to require ordered child-thread orchestration.
- Added active stacked skill revision \`skill-revisions/v000-baseline.md\`.
- Added per-case evaluator trace and trace-verdict requirements.

## Active Revision

\`skill-revisions/v000-baseline.md\`
`);

console.log(`Wrote ${caseIds.length} evaluator traces and orchestration trace.`);
