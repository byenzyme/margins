#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

const defaults = {
  sessionName: "2026-06-24-13-47",
  vault: process.env.MARGINS_DISTILL_PERF_VAULT || join(process.env.HOME || "/tmp", "obsidian"),
  marginsDir: process.env.MARGINS_DISTILL_PERF_DIR || join(process.env.HOME || "/tmp", "obsidian/inbox/.margins"),
  piSession: process.env.MARGINS_DISTILL_PERF_PI_SESSION || "",
  query:
    "Theo home server visible agent thinking appliance trust local model workflow",
};

const args = parseArgs(process.argv.slice(2));
const sessionName = args.sessionName || defaults.sessionName;
const marginsDir = args.marginsDir || defaults.marginsDir;
const piSession = args.piSession || defaults.piSession;
if (!piSession) {
  throw new Error("Pass --pi-session or set MARGINS_DISTILL_PERF_PI_SESSION");
}
const vault = args.vault || defaults.vault;
const query = args.query || defaults.query;

const memoPath = join(marginsDir, `${sessionName}.md`);
const alignedPath = join(marginsDir, `${sessionName}_aligned.md`);
const captureContextPath = join(marginsDir, `${sessionName}_capture_context.md`);
const noteDraftPath = join(marginsDir, `${sessionName}_note_draft.md`);

const records = readJsonl(piSession);
const messages = records.filter((record) => record.type === "message");
const userMessages = messages.filter((record) => record.message?.role === "user");
const assistantMessages = messages.filter((record) => record.message?.role === "assistant");
const toolResults = messages.filter((record) => record.message?.role === "toolResult");

const firstUser = userMessages[0];
const firstAssistantTool = assistantMessages.find((record) =>
  contentBlocks(record).some((block) => block.type === "toolCall"),
);
const firstFinalAnswer = assistantMessages.find((record) => record.message?.stopReason === "stop");
const refineUser = userMessages[1];
const refineAnswer = assistantMessages.at(-1);
const petriResult = toolResults.find((record) => record.message?.toolName === "enzyme_petri");
const catalyzeResults = toolResults.filter((record) => record.message?.toolName === "enzyme_catalyze");

const usageRows = assistantMessages
  .filter((record) => record.message?.usage)
  .map((record) => ({
    timestamp: record.timestamp,
    role: record.message.role,
    stopReason: record.message.stopReason || "",
    input: record.message.usage.input,
    output: record.message.usage.output,
    totalTokens: record.message.usage.totalTokens,
    toolCalls: contentBlocks(record)
      .filter((block) => block.type === "toolCall")
      .map((block) => block.name)
      .join(","),
  }));

const petriText = toolText(petriResult);
const catalyzeChars = catalyzeResults.map((record) => toolText(record).length);
const firstPromptChars = textContent(firstUser?.message?.content).length;
const noteDraftChars = safeRead(noteDraftPath).trim().length;
const promptToFirstToolMs = diffMs(firstUser?.timestamp, firstAssistantTool?.timestamp);
const promptToFirstStopMs = diffMs(firstUser?.timestamp, firstFinalAnswer?.timestamp);
const refineMs = diffMs(refineUser?.timestamp, refineAnswer?.timestamp);
const firstPassNoteDraftMtimeMs = diffMs(
  firstUser?.timestamp,
  safeStatMtimeIso(noteDraftPath),
);

let boundedPetri = null;
if (args.runEnzyme) {
  const started = Date.now();
  const result = spawnSync(
    "enzyme",
    ["petri", "--vault", vault, "--query", query, "--top", "8", "--catalyst-budget", "2"],
    { cwd: vault, encoding: "utf8", maxBuffer: 20 * 1024 * 1024 },
  );
  boundedPetri = {
    command: `enzyme petri --vault ${vault} --query ${JSON.stringify(query)} --top 8 --catalyst-budget 2`,
    exitCode: result.status,
    durationMs: Date.now() - started,
    stdoutChars: result.stdout.length,
    stdoutBytes: Buffer.byteLength(result.stdout),
    stderr: result.stderr.trim(),
  };
  if (result.error) {
    boundedPetri.error = result.error.message;
  }
}

let fixtureDir = null;
if (args.writeFixture) {
  fixtureDir = resolve(args.writeFixture);
  mkdirSync(fixtureDir, { recursive: true });
  copyFileSync(memoPath, join(fixtureDir, "memo.md"));
  copyFileSync(alignedPath, join(fixtureDir, "aligned.md"));
  copyFileSync(captureContextPath, join(fixtureDir, "capture-context.md"));
  writeFileSync(
    join(fixtureDir, "settings.json"),
    `${JSON.stringify(
      {
        session_name: sessionName,
        event_title: "Theo / home server session",
        people: [],
        inbox_folder: "inbox",
        people_folder: "people",
        created_date_format: "[[%Y-%m-%d]]",
        note_filename_template: "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}",
        distill_instructions:
          "Write a grounded Obsidian-native note. Preserve memo-marked moments, represent both speakers, and keep action items concrete.",
      },
      null,
      2,
    )}\n`,
  );
}

const boundedChars = boundedPetri?.stdoutChars ?? null;
const estimatedBoundedTokens = boundedChars == null ? null : Math.ceil(boundedChars / 4);
const historicalPetriTokensEstimate = Math.ceil(petriText.length / 4);
const historicalAfterPetriInput = usageRows[1]?.input ?? null;
const firstTurnInput = usageRows[0]?.input ?? null;
const estimatedAfterBoundedPetriInput =
  firstTurnInput == null || estimatedBoundedTokens == null
    ? null
    : firstTurnInput + estimatedBoundedTokens;

const summary = {
  sessionName,
  files: {
    piSession,
    memoPath,
    alignedPath,
    captureContextPath,
    noteDraftPath,
  },
  historicalNoSpendMetrics: {
    firstPromptChars,
    historicalPetriChars: petriText.length,
    historicalPetriBytes: Buffer.byteLength(petriText),
    historicalPetriTokensEstimate,
    catalyzeResultChars: catalyzeChars,
    noteDraftChars,
    promptToFirstToolMs,
    persistedPromptToFirstStopMs: promptToFirstStopMs,
    firstPassNoteDraftMtimeMs,
    refineMs,
    usageRows,
  },
  boundedPetriNoModelRun: boundedPetri,
  estimatedImpact: {
    firstTurnInputTokens: firstTurnInput,
    historicalAfterPetriInputTokens: historicalAfterPetriInput,
    estimatedBoundedPetriTokens: estimatedBoundedTokens,
    estimatedAfterBoundedPetriInputTokens: estimatedAfterBoundedPetriInput,
    estimatedInputTokensAvoidedAfterPetri:
      historicalAfterPetriInput == null || estimatedAfterBoundedPetriInput == null
        ? null
        : historicalAfterPetriInput - estimatedAfterBoundedPetriInput,
    historicalToBoundedPetriCharReduction:
      boundedChars == null ? null : `${pct(1 - boundedChars / Math.max(1, petriText.length))}%`,
  },
  fixtureReplay: fixtureDir
    ? {
        fixtureDir,
        realRunCommand:
          `cd ${repoRoot} && scripts/cargo-lane disposable -- env ` +
          `MARGINS_UX_FIXTURE_WORK_DIR=/tmp/margins-cameron-distill-replay ` +
          `MARGINS_UX_FIXTURE_VAULT_PATH=${vault} ` +
          `MARGINS_UX_E2E_PREP_AI_PROVIDER=margins-openai-compatible ` +
          `MARGINS_UX_E2E_PREP_OPENAI_BASE_URL=https://openrouter.ai/api/v1 ` +
          `MARGINS_UX_E2E_PREP_OPENAI_MODEL=google/gemini-3-flash-preview ` +
          `MARGINS_UX_E2E_PREP_OPENAI_API_KEY="$OPENAI_API_KEY" ` +
          `MARGINS_UX_E2E_AI_PROVIDER=openai-codex ` +
          `MARGINS_UX_E2E_OPENAI_MODEL=gpt-5.5 ` +
          `cargo run --manifest-path desktop/src-tauri/Cargo.toml --example pi_distill_fixture -- ${fixtureDir}`,
        note: "This replays the Theo artifacts with OpenRouter/Gemini only for prep and ChatGPT subscription GPT-5.5 as the final writer. It spends model/API resources.",
      }
    : null,
};

if (args.json) {
  console.log(JSON.stringify(summary, null, 2));
} else {
  printSummary(summary);
}

function parseArgs(raw) {
  const out = {};
  for (let i = 0; i < raw.length; i += 1) {
    const arg = raw[i];
    if (arg === "--run-enzyme") out.runEnzyme = true;
    else if (arg === "--json") out.json = true;
    else if (arg === "--session-name") out.sessionName = raw[++i];
    else if (arg === "--margins-dir") out.marginsDir = raw[++i];
    else if (arg === "--pi-session") out.piSession = raw[++i];
    else if (arg === "--vault") out.vault = raw[++i];
    else if (arg === "--query") out.query = raw[++i];
    else if (arg === "--write-fixture") out.writeFixture = raw[++i];
    else if (arg === "--help" || arg === "-h") {
      console.log(`Usage: node desktop/scripts/distill-perf-metrics.mjs [options]

Options:
  --run-enzyme            Run bounded enzyme petri locally. No model/API calls.
  --write-fixture <dir>   Copy Theo artifacts into a fixture for opt-in real replay.
  --json                  Print JSON instead of a text report.
  --query <text>          Query for bounded Petri comparison.
  --vault <path>          Vault path. Default: ${defaults.vault}
  --pi-session <path>     Persisted Pi JSONL path.
  --margins-dir <path>      Margins artifact directory.
  --session-name <name>   Session artifact prefix. Default: ${defaults.sessionName}`);
      process.exit(0);
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return out;
}

function readJsonl(path) {
  return readFileSync(path, "utf8")
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

function safeRead(path) {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return "";
  }
}

function safeStatMtimeIso(path) {
  try {
    return statSync(path).mtime.toISOString();
  } catch {
    return null;
  }
}

function contentBlocks(record) {
  const content = record?.message?.content;
  return Array.isArray(content) ? content : [];
}

function textContent(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.map((block) => block.text || block.thinking || "").join("");
}

function toolText(record) {
  return textContent(record?.message?.content);
}

function diffMs(start, end) {
  if (!start || !end) return null;
  return new Date(end).getTime() - new Date(start).getTime();
}

function pct(value) {
  return (value * 100).toFixed(1);
}

function fmtMs(ms) {
  if (ms == null) return "n/a";
  return `${(ms / 1000).toFixed(1)}s`;
}

function printSummary(summary) {
  const historical = summary.historicalNoSpendMetrics;
  const impact = summary.estimatedImpact;
  console.log(`# Distill perf metrics: ${summary.sessionName}`);
  console.log("");
  console.log("Historical persisted Pi session, no model spend:");
  console.log(`- first prompt chars: ${historical.firstPromptChars}`);
  console.log(
    `- full unqueried enzyme_petri payload: ${historical.historicalPetriChars} chars (${historical.historicalPetriTokensEstimate} token est.)`,
  );
  console.log(`- prompt to first tool call: ${fmtMs(historical.promptToFirstToolMs)}`);
  console.log(
    `- prompt to first final answer persisted timestamp (coarse): ${fmtMs(historical.persistedPromptToFirstStopMs)}`,
  );
  console.log(
    `- prompt to note-draft mtime first-pass wall estimate: ${fmtMs(historical.firstPassNoteDraftMtimeMs)}`,
  );
  console.log(`- refine turn wall time: ${fmtMs(historical.refineMs)}`);
  console.log(`- final note draft chars: ${historical.noteDraftChars}`);
  console.log("");
  console.log("Model usage rows:");
  for (const row of historical.usageRows) {
    console.log(
      `- ${row.timestamp}: input=${row.input} output=${row.output} stop=${row.stopReason || "n/a"} tools=${row.toolCalls || "none"}`,
    );
  }
  if (summary.boundedPetriNoModelRun) {
    const bounded = summary.boundedPetriNoModelRun;
    console.log("");
    console.log("Bounded Petri local CLI run, no model spend:");
    console.log(`- command: ${bounded.command}`);
    console.log(
      `- exit=${bounded.exitCode} duration=${fmtMs(bounded.durationMs)} payload=${bounded.stdoutChars} chars`,
    );
    if (bounded.stderr) console.log(`- stderr: ${bounded.stderr}`);
    console.log("");
    console.log("Estimated impact:");
    console.log(`- estimated bounded Petri tokens: ${impact.estimatedBoundedPetriTokens}`);
    console.log(
      `- estimated post-Petri input: ${impact.estimatedAfterBoundedPetriInputTokens} tokens`,
    );
    console.log(
      `- estimated input tokens avoided on post-Petri turn: ${impact.estimatedInputTokensAvoidedAfterPetri}`,
    );
    console.log(
      `- Petri payload char reduction: ${impact.historicalToBoundedPetriCharReduction}`,
    );
  } else {
    console.log("");
    console.log("Run with --run-enzyme to measure bounded Petri payload size locally.");
  }
  if (summary.fixtureReplay) {
    console.log("");
    console.log("Opt-in real replay fixture:");
    console.log(`- fixture: ${summary.fixtureReplay.fixtureDir}`);
    console.log(`- command: ${summary.fixtureReplay.realRunCommand}`);
    console.log(`- note: ${summary.fixtureReplay.note}`);
  }
}
