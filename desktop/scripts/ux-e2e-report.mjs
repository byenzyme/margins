import { existsSync, readdirSync, statSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

const desktopDir = process.cwd();
const runsDir = resolve(desktopDir, "ux-e2e-runs");
const runId = process.env.MARGINS_UX_E2E_RUN_ID || latestRunId();

if (!runId) {
  console.error("No ux-e2e run found. Run npm run ux:e2e first, or set MARGINS_UX_E2E_RUN_ID.");
  process.exit(1);
}

const reportDir = resolve(runsDir, runId, "playwright-report");
if (!existsSync(reportDir)) {
  console.error(`Playwright report not found: ${reportDir}`);
  process.exit(1);
}

const result = spawnSync("npx", ["playwright", "show-report", reportDir], {
  stdio: "inherit",
  cwd: desktopDir,
});

process.exit(result.status ?? 1);

function latestRunId() {
  if (!existsSync(runsDir)) return null;
  const entries = readdirSync(runsDir)
    .map(name => {
      const path = resolve(runsDir, name);
      try {
        return statSync(path).isDirectory() ? { name, mtime: statSync(path).mtimeMs } : null;
      } catch {
        return null;
      }
    })
    .filter(Boolean)
    .sort((a, b) => b.mtime - a.mtime);
  return entries[0]?.name ?? null;
}
