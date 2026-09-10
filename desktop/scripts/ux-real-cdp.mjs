#!/usr/bin/env node
import { execFileSync, spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import net from "node:net";
import { setTimeout as delay } from "node:timers/promises";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const desktopDir = resolve(scriptDir, "..");
const tauriDir = resolve(desktopDir, "src-tauri");

const args = parseArgs(process.argv.slice(2));
const serverPort = Number(args["server-port"] || await freePort());
const vitePort = Number(args["vite-port"] || await freePort());
const host = String(args.host || "127.0.0.1");
const keepState = args["keep-state"] === true;
const views = String(args.views || "home,settings-audio");
const out = String(args.out || "ux-shots-real");
const stateRoot = args["state-dir"]
  ? resolve(String(args["state-dir"]))
  : await mkdtemp(resolve(tmpdir(), "margins-ux-real-"));
const homeDir = resolve(stateRoot, "home");
const dataDir = resolve(stateRoot, "server-data");
const commonGitDir = execFileSync("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], {
  cwd: desktopDir,
  encoding: "utf8",
}).trim();
const targetDir = process.env.CARGO_TARGET_DIR || resolve(dirname(dirname(commonGitDir)), "margins-cargo-target");
const serverBin = resolve(targetDir, "debug", process.platform === "win32" ? "margins-server.exe" : "margins-server");

const children = [];

try {
  await mkdir(homeDir, { recursive: true });
  await mkdir(dataDir, { recursive: true });

  await runProcess("cargo", ["build", "--quiet", "--no-default-features", "--features", "hosted-web", "--bin", "margins-server"], {
    cwd: tauriDir,
    env: {
      ...process.env,
      CARGO_TARGET_DIR: targetDir,
    },
  });

  const server = spawnProcess(serverBin, [], {
    cwd: tauriDir,
    env: {
      ...process.env,
      CARGO_TARGET_DIR: targetDir,
      HOME: homeDir,
      MARGINS_HOST: host,
      MARGINS_PORT: String(serverPort),
      MARGINS_DATA_DIR: dataDir,
      MARGINS_PROFILE: "ux-real-cdp",
      MARGINS_DISABLE_KEYCHAIN: "1",
    },
  });
  children.push(server);

  const serverBase = `http://${host}:${serverPort}`;
  await waitForUrl(`${serverBase}/health`, "Rust HTTP backend");
  const token = (await readFile(resolve(dataDir, "token"), "utf8")).trim();

  const vite = spawnProcess("npm", ["run", "dev", "--", "--host", host, "--port", String(vitePort), "--strictPort"], {
    cwd: desktopDir,
    env: {
      ...process.env,
      MARGINS_DEV_HTTP_TARGET: serverBase,
      MARGINS_DEV_HTTP_TOKEN: token,
    },
  });
  children.push(vite);

  const viteBase = `http://${host}:${vitePort}`;
  await waitForUrl(viteBase, "Vite frontend");

  await runProcess("node", [
    "scripts/ux-cdp.mjs",
    "real",
    "--base",
    viteBase,
    "--out",
    out,
    "--views",
    views,
    ...passthroughArgs(args),
  ], {
    cwd: desktopDir,
    env: {
      ...process.env,
      MARGINS_UX_HEADLESS: process.env.MARGINS_UX_HEADLESS ?? "1",
    },
  });

  console.log(`\nReal-state CDP used isolated state at: ${stateRoot}`);
  if (!keepState) console.log("State will be removed. Re-run with --keep-state to inspect it.");
} finally {
  await Promise.allSettled(children.map(stopProcess));
  if (!keepState && !args["state-dir"]) {
    await rm(stateRoot, { recursive: true, force: true }).catch(() => undefined);
  }
}

function spawnProcess(command, argv, options) {
  const child = spawn(commandName(command), argv, {
    cwd: options.cwd,
    env: options.env,
    stdio: "inherit",
    shell: process.platform === "win32",
  });
  child.on("exit", status => {
    if (status !== 0 && status !== null) {
      console.error(`${command} ${argv.join(" ")} exited with ${status}`);
    }
  });
  return child;
}

async function runProcess(command, argv, options) {
  const child = spawn(commandName(command), argv, {
    cwd: options.cwd,
    env: options.env,
    stdio: "inherit",
    shell: process.platform === "win32",
  });
  const status = await new Promise((resolveStatus, reject) => {
    child.on("error", reject);
    child.on("close", resolveStatus);
  });
  if (status !== 0) throw new Error(`${command} ${argv.join(" ")} exited with ${status}`);
}

async function stopProcess(child) {
  if (!child || child.exitCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([
    new Promise(resolveStop => child.once("close", resolveStop)),
    delay(3000).then(() => child.kill("SIGKILL")),
  ]);
}

async function waitForUrl(url, label) {
  const started = Date.now();
  while (Date.now() - started < 120_000) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
    } catch {
      // Still starting.
    }
    await delay(500);
  }
  throw new Error(`Timed out waiting for ${label} at ${url}`);
}

function freePort() {
  return new Promise((resolvePort, reject) => {
    const server = net.createServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => resolvePort(address.port));
    });
  });
}

function parseArgs(argv) {
  const parsed = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const item = argv[i];
    if (!item.startsWith("--")) {
      parsed._.push(item);
      continue;
    }
    const eq = item.indexOf("=");
    if (eq !== -1) {
      parsed[item.slice(2, eq)] = item.slice(eq + 1);
      continue;
    }
    const key = item.slice(2);
    const next = argv[i + 1];
    if (!next || next.startsWith("--")) parsed[key] = true;
    else parsed[key] = argv[++i];
  }
  return parsed;
}

function passthroughArgs(parsed) {
  const out = [];
  if (parsed["keep-chrome"] === true) out.push("--keep-chrome");
  return out;
}

function commandName(command) {
  if (process.platform !== "win32") return command;
  if (command === "npm") return "npm.cmd";
  if (command === "npx") return "npx.cmd";
  return command;
}
