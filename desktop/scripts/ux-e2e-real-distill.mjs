import { readFile, writeFile, mkdir } from "node:fs/promises";
import { existsSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { setupRuntimeVault } from "../test-harness/ux-loop/runtime-vault.mjs";

const desktopDir = process.cwd();
const runId = process.env.MARGINS_UX_E2E_RUN_ID || "latest";
const fixture = process.env.MARGINS_UX_E2E_FIXTURE || "customer-call";
const runDir = resolve(desktopDir, "ux-e2e-runs", runId);
const realDir = resolve(runDir, "real-distill");
const summaryPath = resolve(realDir, "summary.json");
const runtimePath = resolve(desktopDir, "test-harness", "ux-loop", "runtime", `${runId}.json`);
const workDir = resolve(runDir, "real-distill-work");
const piAgentDir = resolve(realDir, "pi-agent");
const sourceFixtureDir = resolve(desktopDir, "test-harness", "ux-loop", "fixtures", fixture);
const compactFixtureDir = resolve(realDir, "compact-fixture");
const dotenv = loadDotenv(resolve(desktopDir, "..", ".env"));

await mkdir(realDir, { recursive: true });
await mkdir(piAgentDir, { recursive: true });
const runtime = await setupRuntimeVault({ runId, fixture });
const distillFixtureDir = process.env.MARGINS_UX_E2E_FULL_REAL_FIXTURE === "1"
  ? sourceFixtureDir
  : await writeCompactFixture(sourceFixtureDir, compactFixtureDir);

const env = {
  ...process.env,
  CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR || resolve(process.env.HOME || "/tmp", ".cache/margins-cargo-target"),
  PI_CODING_AGENT_DIR: piAgentDir,
  MARGINS_UX_FIXTURE_WORK_DIR: workDir,
  MARGINS_UX_FIXTURE_VAULT_PATH: runtime.vault_path,
  MARGINS_UX_FIXTURE_OUT: summaryPath,
};

const apiKey = process.env.MARGINS_UX_E2E_OPENAI_API_KEY || process.env.OPENAI_API_KEY || dotenv.OPENAI_API_KEY;
const baseUrl = process.env.OPENAI_BASE_URL || dotenv.OPENAI_BASE_URL || "";
const model = process.env.MARGINS_UX_E2E_OPENAI_MODEL || dotenv.OPENAI_MODEL || "gpt-5.5";
const maxTokens = Number(process.env.MARGINS_UX_E2E_MAX_TOKENS || 768);
const provider = process.env.MARGINS_UX_E2E_AI_PROVIDER
  || dotenv.MARGINS_UX_E2E_AI_PROVIDER
  || (apiKey ? "margins-openai-compatible" : undefined);

if (apiKey && provider === "margins-openai-compatible") {
  await writeFile(resolve(piAgentDir, "models.json"), `${JSON.stringify({
    providers: {
      "margins-openai-compatible": {
        baseUrl: baseUrl || "https://api.openai.com/v1",
        api: "openai-completions",
        apiKey: "env:MARGINS_API_KEY",
        authHeader: true,
        compat: {
          supportsDeveloperRole: false,
          supportsReasoningEffort: false,
        },
        models: [{
          id: model,
          name: `${model} (Margins UX E2E)`,
          reasoning: false,
          input: ["text"],
          contextWindow: 128000,
          maxTokens: 1024,
          options: {
            maxTokens,
          },
        }],
      },
    },
  }, null, 2)}\n`, "utf8");
}

if (apiKey) {
  env.MARGINS_UX_E2E_OPENAI_API_KEY = apiKey;
}
if (model) {
  env.MARGINS_UX_E2E_OPENAI_MODEL = model;
}
if (provider) {
  env.MARGINS_UX_E2E_AI_PROVIDER = provider;
}
env.MARGINS_UX_E2E_MAX_TOKENS = String(maxTokens);

const result = spawnSync(
  "cargo",
  ["run", "--example", "pi_distill_fixture", "--", distillFixtureDir],
  {
    cwd: resolve(desktopDir, "src-tauri"),
    env,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  },
);

await writeFile(resolve(realDir, "stdout.log"), result.stdout || "", "utf8");
await writeFile(resolve(realDir, "stderr.log"), result.stderr || "", "utf8");

if (result.status !== 0) {
  console.error(result.stdout);
  console.error(result.stderr);
  console.error(`Real distill failed; logs written under ${realDir}`);
  process.exit(result.status ?? 1);
}

const summary = JSON.parse(await readFile(summaryPath, "utf8"));
const notePath = summary.note_path;
if (!notePath) {
  console.error(`Real distill did not produce a note_path. Summary: ${summaryPath}`);
  process.exit(1);
}

const note = await readFile(notePath, "utf8");
const updatedRuntime = JSON.parse(await readFile(runtimePath, "utf8"));
const sessionName = updatedRuntime.sessions?.[0]?.name || "customer-call";
const groundingPath = resolve(workDir, ".margins", `${sessionName}_grounding.json`);
const grounding = await readJsonFile(groundingPath, null);
updatedRuntime.real_distill = {
  summary_path: summaryPath,
  note_path: notePath,
  session_file: summary.session_file || null,
  elapsed_ms: summary.elapsed_ms,
  event_count: Array.isArray(summary.events) ? summary.events.length : 0,
};
updatedRuntime.processing_events = normalizeEvents(summary.events || []);
updatedRuntime.samples ??= {};
updatedRuntime.samples[sessionName] ??= {};
updatedRuntime.samples[sessionName].note = note;
if (grounding) {
  updatedRuntime.samples[sessionName].grounding = grounding;
}

await writeFile(runtimePath, `${JSON.stringify(updatedRuntime, null, 2)}\n`, "utf8");
await writeFile(resolve(runDir, "real-distill-manifest.json"), `${JSON.stringify(updatedRuntime.real_distill, null, 2)}\n`, "utf8");

console.log(`Real distill note: ${notePath}`);
console.log(`Real distill events: ${updatedRuntime.processing_events.length}`);
console.log(`Runtime fixture updated: ${runtimePath}`);

function normalizeEvents(events) {
  return events
    .filter(event => event && typeof event.stage === "string" && typeof event.message === "string")
    .map(event => ({
      stage: event.stage,
      message: event.message,
      progress: typeof event.progress === "number" ? event.progress : null,
      delay_ms: delayFor(event),
    }));
}

function delayFor(event) {
  if (event.stage === "note_stream") return 4;
  if (event.stage === "transcript") return 10;
  if (/Tool start|Tool done|Tool failed/i.test(event.message)) return 90;
  return 70;
}

function loadDotenv(path) {
  if (!existsSync(path)) return {};
  const text = readFileSync(path, "utf8");
  const values = {};
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) continue;
    const idx = line.indexOf("=");
    if (idx === -1) continue;
    const key = line.slice(0, idx).trim();
    if (!["OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_MODEL", "MARGINS_UX_E2E_AI_PROVIDER"].includes(key)) continue;
    values[key] = unquote(line.slice(idx + 1).trim());
  }
  return values;
}

function unquote(value) {
  if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
    return value.slice(1, -1);
  }
  return value;
}

async function writeCompactFixture(sourceDir, outDir) {
  await mkdir(outDir, { recursive: true });
  const [settings, memo, capture, aligned] = await Promise.all([
    readFile(resolve(sourceDir, "settings.json"), "utf8"),
    readFile(resolve(sourceDir, "memo.md"), "utf8"),
    readFile(resolve(sourceDir, "capture-context.md"), "utf8"),
    readFile(resolve(sourceDir, "aligned.md"), "utf8"),
  ]);
  const keepPatterns = [
    /00:00:/,
    /00:01:/,
    /00:03:/,
    /00:06:/,
    /00:07:/,
    /00:09:/,
    /00:12:/,
    /00:13:/,
    /00:14:/,
    /00:17:/,
    /00:20:/,
    /00:22:/,
    /00:26:/,
    /00:31:/,
    /00:38:/,
  ];
  const compactAligned = aligned
    .split(/\r?\n/)
    .filter(line => line.startsWith("#") || !line.trim() || keepPatterns.some(pattern => pattern.test(line)))
    .join("\n");
  await Promise.all([
    writeFile(resolve(outDir, "settings.json"), settings, "utf8"),
    writeFile(resolve(outDir, "memo.md"), memo, "utf8"),
    writeFile(resolve(outDir, "capture-context.md"), capture, "utf8"),
    writeFile(resolve(outDir, "aligned.md"), compactAligned, "utf8"),
  ]);
  return outDir;
}

async function readJsonFile(path, fallback) {
  try {
    return JSON.parse(await readFile(path, "utf8"));
  } catch {
    return fallback;
  }
}
