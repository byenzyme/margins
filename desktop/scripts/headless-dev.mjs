#!/usr/bin/env node
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { CDP_HOST, CDP_PORT, chromeLaunchPlan } from "../test-harness/ux-cdp/chrome.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const desktopDir = resolve(scriptDir, "..");
const DEFAULT_BASE = "http://127.0.0.1:5173";

const [cmd = "help", ...rest] = process.argv.slice(2);

try {
  if (cmd === "help" || cmd === "--help" || cmd === "-h") {
    usage();
  } else if (cmd === "status") {
    status();
  } else if (cmd === "check") {
    status();
    await run("node", ["scripts/ux-cdp.mjs", "help"]);
    if (depsInstalled()) {
      await runNpmScript("ux:e2e", ["--list"]);
    } else {
      console.log("Playwright list skipped: node_modules is not installed. Run npm ci before full E2E/build verification.");
    }
  } else if (cmd === "cdp") {
    await runCdp(rest);
  } else if (cmd === "e2e") {
    await runNpmScript("ux:e2e", rest);
  } else if (cmd === "real-distill-help") {
    realDistillHelp();
  } else {
    usage();
    process.exit(1);
  }
} catch (err) {
  console.error(err.stack || String(err));
  process.exit(1);
}

function usage() {
  console.log(`Margins remote/headless desktop lane

Usage:
  node scripts/headless-dev.mjs check
  node scripts/headless-dev.mjs status
  node scripts/headless-dev.mjs cdp [ux:cdp args]
  node scripts/headless-dev.mjs e2e [playwright args]
  node scripts/headless-dev.mjs real-distill-help

Examples:
  npm run headless:check
  npm run headless:cdp -- --scenarios settings-audio,recording-healthy
  npm run headless:e2e -- --grep customer-call-connected-note

The cdp lane starts Vite for a local base URL when needed, sets MARGINS_UX_HEADLESS=1 by default, and then delegates to npm run ux:cdp.
The e2e lane delegates to npm run ux:e2e; Playwright starts/reuses Vite from its config.`);
}

function status() {
  console.log("Margins remote/headless desktop status");
  console.log(`desktop: ${desktopDir}`);
  console.log(`cdp: http://${CDP_HOST}:${CDP_PORT}`);
  console.log(`MARGINS_UX_HEADLESS: ${process.env.MARGINS_UX_HEADLESS || "(auto)"}`);
  console.log(`MARGINS_CHROME_BIN: ${process.env.MARGINS_CHROME_BIN || "(auto)"}`);
  console.log(`node_modules: ${depsInstalled() ? "present" : "missing"}`);
  try {
    const plan = chromeLaunchPlan();
    console.log(`browser launch: ${plan.mode}, headless=${plan.headless}, command=${plan.command}`);
    console.log(`browser profile: ${plan.profile}`);
  } catch (err) {
    console.log(`browser launch: unavailable (${err.message})`);
  }
}

async function runCdp(args) {
  const parsed = parseArgs(args);
  const base = parsed.base || DEFAULT_BASE;
  const vite = await ensureLocalVite(base);
  const env = {
    ...process.env,
    MARGINS_UX_HEADLESS: process.env.MARGINS_UX_HEADLESS ?? "1",
  };
  try {
    await runNpmScript("ux:cdp", args, { env });
  } finally {
    if (vite) await stopProcess(vite);
  }
}

async function ensureLocalVite(base) {
  if (await urlOk(base)) return null;
  const url = new URL(base);
  if (!["127.0.0.1", "localhost"].includes(url.hostname)) {
    console.log(`Base URL is not local (${base}); expecting an existing dev server.`);
    return null;
  }

  const port = url.port || "5173";
  const child = spawn(commandName("npm"), ["run", "dev", "--", "--host", url.hostname, "--port", port], {
    cwd: desktopDir,
    env: process.env,
    stdio: "inherit",
    shell: process.platform === "win32",
  });

  const started = Date.now();
  while (Date.now() - started < 120_000) {
    if (await urlOk(base)) return child;
    if (child.exitCode !== null) {
      throw new Error(`Vite exited before ${base} became available.`);
    }
    await delay(500);
  }
  await stopProcess(child);
  throw new Error(`Timed out waiting for Vite at ${base}`);
}

async function urlOk(url) {
  try {
    const res = await fetch(url, { method: "GET" });
    return res.ok;
  } catch {
    return false;
  }
}

async function runNpmScript(script, args, options = {}) {
  await run("npm", ["run", script, ...(args.length ? ["--", ...args] : [])], options);
}

async function run(command, args, options = {}) {
  const child = spawn(commandName(command), args, {
    cwd: desktopDir,
    env: options.env || process.env,
    stdio: "inherit",
    shell: process.platform === "win32",
  });
  const status = await new Promise((resolveStatus, reject) => {
    child.on("error", reject);
    child.on("close", resolveStatus);
  });
  if (status !== 0) throw new Error(`${command} ${args.join(" ")} exited with ${status}`);
}

async function stopProcess(child) {
  if (child.exitCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([
    new Promise(resolveStop => child.once("close", resolveStop)),
    delay(3000).then(() => child.kill("SIGKILL")),
  ]);
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const value = argv[i];
    if (value === "--base" && argv[i + 1]) args.base = argv[++i];
    else if (value.startsWith("--base=")) args.base = value.slice("--base=".length);
  }
  return args;
}

function commandName(command) {
  if (process.platform !== "win32") return command;
  if (command === "npm") return "npm.cmd";
  if (command === "npx") return "npx.cmd";
  return command;
}

function depsInstalled() {
  return existsSync(resolve(desktopDir, "node_modules", "@playwright", "test"));
}

function realDistillHelp() {
  console.log(`Real LLM distillation is intentionally opt-in because it may spend model/API resources.

To run it after explicit approval:
  npm run ux:e2e:real-distill

Useful environment:
  MARGINS_UX_E2E_RUN_ID=<run-id>
  MARGINS_UX_E2E_OPENAI_API_KEY=<key>
  MARGINS_UX_E2E_OPENAI_MODEL=<model>
  MARGINS_UX_E2E_FULL_REAL_FIXTURE=1

For the combined real-distill plus Playwright customer-call lane:
  npm run ux:e2e:real`);
}
