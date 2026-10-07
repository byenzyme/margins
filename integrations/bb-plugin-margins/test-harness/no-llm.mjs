#!/usr/bin/env node
// Isolated bb + Chrome journey. Steps 1-6 stop before Make note; step 7 requires an explicit opt-in.
// Linux runs Chrome in Docker; macOS runs a pinned Chrome for Testing as a local headless process.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { closeSync, cpSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../..");
const plugin = path.resolve(here, "..");
const required = (name) => {
  const value = process.env[name];
  if (!value || !path.isAbsolute(value) || !existsSync(value)) throw new Error(`${name} must be an existing absolute path`);
  return value;
};
// The macOS lane uses a local headless Chrome for Testing and the CoreML recorder runtime.
const macLane = process.platform === "darwin";
const CHROME_FOR_TESTING_VERSION = "154.0.8037.92";
const marginsBin = required("MARGINS_E2E_BIN");
const freshRelease = process.env.MARGINS_E2E_FRESH_RELEASE === "1";
const coldAsr = process.env.MARGINS_E2E_COLD_ASR === "1";
const serverBin = freshRelease ? null : required("MARGINS_E2E_SERVER_BIN");
const bbApp = required("MARGINS_E2E_BB_APP");
// Host-only bb-app installs (an enrolled machine's runtime) cannot serve an isolated bb.
if (!existsSync(path.resolve(path.dirname(bbApp), "../server/dist/index.js"))) {
  throw new Error("MARGINS_E2E_BB_APP must be a full bb-app package with server/dist, not a host-only runtime");
}
const chromeBin = macLane && !process.env.MARGINS_E2E_CHROME_BIN ? null : required("MARGINS_E2E_CHROME_BIN");
const spokenWav = process.env.MARGINS_E2E_SPOKEN_WAV
  ? required("MARGINS_E2E_SPOKEN_WAV")
  : path.join(here, "fixtures", "launch-accessibility.wav");
const asrModelDir = coldAsr || macLane ? null : required("MARGINS_E2E_ASR_MODEL_DIR");
const ortLibrary = coldAsr || macLane ? null : required("MARGINS_E2E_ORT_LIBRARY");
const coremlModelDir = coldAsr || !macLane ? null : required("MARGINS_E2E_COREML_MODEL_DIR");
const realLlm = process.env.MARGINS_E2E_REAL_LLM === "1";
const ffmpeg = process.env.MARGINS_E2E_FFMPEG ? required("MARGINS_E2E_FFMPEG") : "ffmpeg";
const image = process.env.MARGINS_E2E_CHROME_IMAGE || "margins-bb-e2e-chrome:local";
const artifacts = path.resolve(process.env.MARGINS_E2E_ARTIFACTS || path.join(plugin, "e2e-artifacts", new Date().toISOString().replace(/[:.]/g, "-")));
// macOS caps Unix socket paths at 104 bytes, so avoid the long per-user TMPDIR there.
const temporary = mkdtempSync(path.join(macLane ? "/private/tmp" : os.tmpdir(), "margins-bb-meetings-"));
const home = path.join(temporary, "margins-home");
const vault = path.join(temporary, "vault");
const code = path.join(temporary, "code");
const bbData = path.join(temporary, "bb");
const freshCliBinDir = path.join(temporary, "plugin-installed-bin");
const wav = path.join(temporary, "meeting.wav");
const browserConfig = path.join(temporary, "agent-browser.json");
const chromeName = `margins-bb-e2e-${process.pid}`;
const browserSession = `margins-bb-e2e-${process.pid}`;
const browserSocketDir = path.join(temporary, "agent-browser");
// agent-browser encodes the journey video with the ffmpeg it finds on PATH.
const browserEnv = { ...process.env, AGENT_BROWSER_CONFIG: browserConfig,
  ...(macLane ? { AGENT_BROWSER_SOCKET_DIR: browserSocketDir } : {}),
  ...(ffmpeg !== "ffmpeg" ? { PATH: `${path.dirname(ffmpeg)}${path.delimiter}${process.env.PATH}` } : {}) };
const coldCache = path.join(temporary, "cache");
const coldCoremlModel = path.join(coldCache, "FluidAudio/Models/parakeet-tdt-0.6b-v2");
const menuBridgePorts = [18764, 18765];
const coremlRequired = ["Preprocessor.mlmodelc", "Encoder.mlmodelc", "Decoder.mlmodelc", "JointDecision.mlmodelc"];
let chromeProcess;
let bbProcess;
let videoStarted = false;
let videoStartedAt = 0;
let otherThreadAt = 0;
let browserUsed = false;
let browserCloseResult = null;
let interruptedSignal = null;
let passMessage = null;

mkdirSync(artifacts, { recursive: true });
mkdirSync(home, { recursive: true });
mkdirSync(code, { recursive: true });
cpSync(path.join(repo, "desktop/test-harness/local-e2e/seed-vault"), vault, { recursive: true });
mkdirSync(path.join(vault, "inbox"), { recursive: true });
writeFileSync(browserConfig, "{}\n");
mkdirSync(browserSocketDir, { recursive: true });

function command(binary, args, options = {}) {
  const result = spawnSync(binary, args, { encoding: "utf8", maxBuffer: 8 * 1024 * 1024, ...options });
  if (result.status !== 0) throw new Error(`${binary} ${args.slice(0, 3).join(" ")} failed: ${(result.stderr || result.stdout || "").slice(-1500)}`);
  return result.stdout.trim();
}
function jsonCommand(binary, args, options = {}) { return JSON.parse(command(binary, args, options)); }
function findId(value, prefix) {
  if (typeof value === "string" && value.startsWith(prefix)) return value;
  if (value && typeof value === "object") for (const entry of Object.values(value)) {
    const found = findId(entry, prefix);
    if (found) return found;
  }
  return null;
}
function makeSpokenWav(file, source) {
  // Chrome starts consuming fake microphone audio at launch, before a cold
  // release install and recorder startup finish. Repeat the utterance so the
  // captured interval contains speech even when setup takes longer.
  command(ffmpeg, ["-nostdin", "-loglevel", "error", "-stream_loop", "-1", "-i", source,
    "-t", "120", "-ac", "1", "-ar", "16000", "-acodec", "pcm_s16le", "-y", file]);
}
async function freePort() {
  const server = net.createServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}
async function until(label, probe, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    if (interruptedSignal) throw new Error(`Interrupted by ${interruptedSignal}`);
    try { const result = await probe(); if (result) return result; }
    catch (error) { last = error; }
    await new Promise((resolve) => setTimeout(resolve, 400));
  }
  throw new Error(`${label} did not become ready: ${last || "timeout"}`);
}

function chromeForTestingBinary() {
  if (chromeBin) return chromeBin;
  // Pinned Chrome for Testing in a reusable cache; never the user's Chrome or profile.
  const platformName = process.arch === "arm64" ? "mac-arm64" : "mac-x64";
  const cache = path.resolve(process.env.MARGINS_E2E_CHROME_CACHE
    || path.join(os.homedir(), "Library/Caches/margins-bb-e2e/chrome-for-testing"));
  const root = path.join(cache, CHROME_FOR_TESTING_VERSION);
  const binary = path.join(root, `chrome-${platformName}`, "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing");
  if (existsSync(binary)) return binary;
  mkdirSync(cache, { recursive: true });
  const staging = mkdtempSync(path.join(cache, ".download-"));
  try {
    const zip = path.join(staging, "chrome.zip");
    command("curl", ["-fsSL", "--retry", "3", "-o", zip,
      `https://storage.googleapis.com/chrome-for-testing-public/${CHROME_FOR_TESTING_VERSION}/${platformName}/chrome-${platformName}.zip`]);
    command("ditto", ["-x", "-k", zip, path.join(staging, "unpacked")]);
    rmSync(root, { recursive: true, force: true });
    command("mv", [path.join(staging, "unpacked"), root]);
  } finally { rmSync(staging, { recursive: true, force: true }); }
  if (!existsSync(binary)) throw new Error(`Chrome for Testing ${CHROME_FOR_TESTING_VERSION} did not unpack to ${binary}`);
  return binary;
}
function coremlInstalled(dir) {
  return coremlRequired.every((name) => existsSync(path.join(dir, name)))
    && existsSync(path.join(dir, "parakeet_vocab.json"));
}
function startLocalChrome() {
  const binary = chromeForTestingBinary();
  const out = openSync(path.join(artifacts, "chrome.log"), "w");
  chromeProcess = spawn(binary, ["--headless=new", "--disable-gpu", `--remote-debugging-port=${cdpPort}`,
    "--remote-allow-origins=*", `--user-data-dir=${path.join(temporary, "chrome-profile")}`,
    "--no-first-run", "--no-default-browser-check", "--use-mock-keychain", "--password-store=basic",
    // The macOS audio-service sandbox cannot read the fake-microphone WAV.
    "--disable-features=AudioServiceSandbox",
    "--use-fake-device-for-media-stream", `--use-file-for-fake-audio-capture=${wav}`,
    "--use-fake-ui-for-media-stream", "about:blank"], { stdio: ["ignore", out, out], detached: true });
  closeSync(out);
  return binary;
}
function browser(args) {
  if (interruptedSignal) throw new Error(`Interrupted by ${interruptedSignal}`);
  browserUsed = true;
  return command("agent-browser", ["--session", browserSession, "--cdp", String(cdpPort), ...args], { env: browserEnv });
}
function browserEval(source) {
  if (interruptedSignal) throw new Error(`Interrupted by ${interruptedSignal}`);
  browserUsed = true;
  return jsonCommand("agent-browser", ["--session", browserSession, "--cdp", String(cdpPort), "--json", "eval", source], { env: browserEnv }).data.result;
}
function shot(name) { browser(["screenshot", path.join(artifacts, name)]); }
function bb(args) { return jsonCommand("bb", [...args, "--json"], { env: bbEnv }); }
function assertMemo(text) {
  assert.equal(browserEval(`document.querySelector('textarea[aria-label="Meeting memo pad"]')?.value`), text);
}

function processTable() {
  if (!macLane) {
    // Linux containers may lack ps; /proc has the same facts.
    const entries = [];
    for (const entry of readdirSync("/proc")) {
      if (!/^\d+$/.test(entry)) continue;
      try {
        const stat = readFileSync(`/proc/${entry}/stat`, "utf8");
        const ppid = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
        entries.push({ pid: Number(entry), ppid, command: readFileSync(`/proc/${entry}/cmdline`, "utf8").replaceAll("\0", " ") });
      } catch { /* process exited while /proc was read */ }
    }
    return entries;
  }
  return command("ps", ["-A", "-ww", "-o", "pid=,ppid=,command="]).split("\n").map((line) => {
    const match = line.trim().match(/^(\d+)\s+(\d+)\s+(.*)$/);
    return match ? { pid: Number(match[1]), ppid: Number(match[2]), command: match[3] } : null;
  }).filter(Boolean);
}
function browserDaemonPids() {
  if (macLane) {
    // macOS has no /proc; the daemon records its pid in the isolated socket dir.
    try {
      const pid = Number(readFileSync(path.join(browserSocketDir, `${browserSession}.pid`), "utf8").trim());
      return pid && processTable().some((entry) => entry.pid === pid && entry.command.includes("agent-browser")) ? [pid] : [];
    } catch { return []; }
  }
  const pids = [];
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      const processRoot = `/proc/${entry}`;
      if (!readFileSync(path.join(processRoot, "cmdline"), "utf8").includes("agent-browser")) continue;
      const environment = new Set(readFileSync(path.join(processRoot, "environ"), "utf8").split("\0"));
      if (environment.has("AGENT_BROWSER_DAEMON=1") && environment.has(`AGENT_BROWSER_SESSION=${browserSession}`)) pids.push(Number(entry));
    } catch { /* process exited while /proc was read */ }
  }
  return pids;
}
function closeBrowserSession() {
  const daemonPidsBefore = browserDaemonPids();
  if (browserCloseResult && daemonPidsBefore.length === 0) return browserCloseResult;
  const result = browserUsed && daemonPidsBefore.length
    ? spawnSync("agent-browser", ["--session", browserSession, "--cdp", String(cdpPort), "close"], {
      env: browserEnv, encoding: "utf8", timeout: 10_000,
    }) : null;
  browserCloseResult = { session: browserSession,
    daemonPidsBefore: [...new Set([...(browserCloseResult?.daemonPidsBefore || []), ...daemonPidsBefore])],
    closeAttempts: (browserCloseResult?.closeAttempts || 0) + (result ? 1 : 0),
    closeStatus: result?.status ?? browserCloseResult?.closeStatus ?? null,
    closeError: result?.error?.message || (result?.status ? (result.stderr || result.stdout || "").trim() : null) };
  return browserCloseResult;
}
async function verifyBrowserClosed() {
  const closed = closeBrowserSession();
  let remaining = browserDaemonPids();
  const deadline = Date.now() + 5_000;
  while (remaining.length && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    remaining = browserDaemonPids();
  }
  const forcedPids = [...remaining];
  for (const pid of forcedPids) try { process.kill(pid, "SIGTERM"); } catch { /* already exited */ }
  const forceDeadline = Date.now() + 2_000;
  while (remaining.length && Date.now() < forceDeadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    remaining = browserDaemonPids();
  }
  for (const pid of remaining) try { process.kill(pid, "SIGKILL"); } catch { /* already exited */ }
  if (remaining.length) await new Promise((resolve) => setTimeout(resolve, 100));
  remaining = browserDaemonPids();
  return { ...closed, forcedPids, daemonPidsAfter: remaining,
    verified: remaining.length === 0 && (!closed.daemonPidsBefore.length || closed.closeStatus === 0 || forcedPids.length > 0) };
}

for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => {
  if (interruptedSignal) return;
  interruptedSignal = signal;
  process.exitCode = signal === "SIGINT" ? 130 : 143;
  try { closeBrowserSession(); } catch (error) { process.stderr.write(`Browser signal cleanup failed: ${error}\n`); }
});

let cdpPort;
let bbEnv;
let currentProjectId;
async function holdBeforeNote() {
  const marker = path.join(artifacts, "continue-preflight");
  writeFileSync(path.join(artifacts, "preflight-ready.json"), `${JSON.stringify({
    bbServerUrl: bbEnv.BB_SERVER_URL, bbData, cdpPort, browserSession, browserConfig, temporary,
    projectId: currentProjectId,
  }, null, 2)}\n`);
  console.log(`Preflight paused before Make note; inspect ${artifacts}/preflight-ready.json, then create ${marker}`);
  await until("preflight release", () => existsSync(marker), 10 * 60_000);
}
try {
  assert(!home.startsWith(path.join(os.homedir(), ".margins")));
  assert(!vault.startsWith("/workspace/obsidian"));
  writeFileSync(path.join(code, "AGENTS.md"), `# Disposable Margins meeting fixture\n\nFor a BB @Meeting, use the bundled Margins BB agent tools to read the pinned session, memo, and transcript, then link a written note. Do not use a local Margins CLI or Codex MCP to read this meeting. This project's notes are in the disposable Workspace Home ${vault}; write the connected note inside its inbox/ folder. The note must reflect distinct evidence from both the memo and the spoken transcript. Do not read or write /workspace/obsidian or ~/.margins.\n`);
  command("git", ["init", "-q", "-b", "main", code]);
  command("git", ["-C", code, "add", "AGENTS.md"]);
  command("git", ["-C", code, "-c", "user.name=Meetings E2E", "-c", "user.email=meetings-e2e@example.invalid", "commit", "-qm", "Seed disposable project"]);
  makeSpokenWav(wav, spokenWav);
  const marginsEnv = { ...process.env, MARGINS_HOME: home };
  command(marginsBin, ["workspace", "new", "e2e", "--home", vault], { env: marginsEnv });
  const config = path.join(home, "workspaces/e2e/config.toml");
  const desired = path.join(artifacts, "desired.toml");
  const original = readFileSync(config, "utf8");
  assert(original.includes("[bindings.home]"));
  writeFileSync(desired, original.replace(/(\[bindings\.home\][^[]*)/, "$1note_folder = \"inbox\"\n"));
  const plan = path.join(artifacts, "reviewed-plan.json");
  writeFileSync(plan, `${command(marginsBin, ["--workspace", "e2e", "workspace", "plan", "--desired", desired, "--json"], { env: marginsEnv })}\n`);
  assert(readFileSync(plan, "utf8").includes("inbox"));
  command(marginsBin, ["--workspace", "e2e", "workspace", "apply", "--plan", plan, "--json"], { env: marginsEnv });
  command(marginsBin, ["workspace", "default", "--set", "e2e"], { env: marginsEnv });
  const destination = jsonCommand(marginsBin, ["workspace", "destination", "--json"], { env: marginsEnv });
  assert.equal(destination.home_root, vault);
  assert.equal(destination.note_folder, "inbox");
  assert.equal(destination.destination, path.join(vault, "inbox"));
  writeFileSync(path.join(artifacts, "destination.json"), `${JSON.stringify(destination, null, 2)}\n`);

  const serverPort = await freePort();
  const daemonPort = await freePort();
  cdpPort = await freePort();
  // Drop the parent bb thread's identity so CLI calls cannot reach the real bb.
  const parentEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
    !["BB_THREAD_ID", "BB_PROJECT_ID", "BB_ENVIRONMENT_ID", "BB_HOST_DAEMON_PORT", "BB_THREAD_STORAGE"].includes(key)));
  bbEnv = { ...parentEnv, BB_SERVER_URL: `http://127.0.0.1:${serverPort}`, BB_DATA_DIR: bbData,
    MARGINS_HOME: home,
    // HOME stays real: bb's built-in plugins query the login keychain, and an
    // empty HOME makes macOS show "Keychain Not Found". Every Margins and bb
    // write location is redirected explicitly instead.
    ...(macLane ? { MARGINS_FLUID_COREML_MODEL_DIR: coldAsr ? coldCoremlModel : coremlModelDir }
      : coldAsr ? { XDG_CACHE_HOME: coldCache } : {
        MARGINS_PARAKEET_MODEL_DIR: asrModelDir, MARGINS_PARAKEET_MODEL_KIND: "tdt",
        ORT_DYLIB_PATH: ortLibrary,
      }),
    ...(freshRelease ? { MARGINS_CLI_BIN_DIR: freshCliBinDir } : { MARGINS_CLI_BIN: marginsBin }),
    // This journey exercises the explicit Make note action once. The separate
    // auto-note scheduler has its own test and must not race a paid E2E call.
    MARGINS_BB_E2E_DISABLE_AUTO_NOTE: "1" };
  if (freshRelease) {
    delete bbEnv.MARGINS_CLI_BIN;
    delete bbEnv.MARGINS_PROJECT_SERVER_PATH;
    delete bbEnv.MARGINS_BB_REMOTE_URL;
    delete bbEnv.MARGINS_BB_REMOTE_TOKEN;
  }
  if (coldAsr || macLane) {
    delete bbEnv.MARGINS_PARAKEET_MODEL_DIR;
    delete bbEnv.MARGINS_PARAKEET_MODEL_KIND;
    delete bbEnv.ORT_DYLIB_PATH;
  }
  const hostData = path.join(bbData, "plugins/margins/host-data");
  mkdirSync(hostData, { recursive: true });
  if (!freshRelease && !coldAsr && !macLane) writeFileSync(path.join(hostData, "asr-runtime.json"), `${JSON.stringify({ serverPath: serverBin,
    modelDir: asrModelDir, ortLibraryPath: ortLibrary })}\n`);
  const bbOut = openSync(path.join(artifacts, "bb.log"), "w");
  const bbErr = openSync(path.join(artifacts, "bb-errors.log"), "w");
  bbProcess = spawn(bbApp, ["--data-dir", bbData, "--server-bind-host", "0.0.0.0", "--server-port", String(serverPort), "--host-daemon-port", String(daemonPort), "start"], {
    env: { ...bbEnv, ...(freshRelease ? {} : { MARGINS_PROJECT_SERVER_PATH: serverBin }) },
    stdio: ["ignore", bbOut, bbErr],
  });
  closeSync(bbOut); closeSync(bbErr);
  await until("isolated bb", () => { try { bb(["project", "list"]); return true; } catch { return false; } }, 30_000);
  const machine = await until("isolated bb machine", () => {
    try { return bb(["machine", "list"]).find((host) => host.status === "connected"); }
    catch { return null; }
  }, 30_000);
  if (realLlm) command("bb", ["plugin", "disable", "provider-retry"], { env: bbEnv });
  command("bb", ["plugin", "install", plugin, "--yes"], { env: bbEnv });
  const projectId = findId(bb(["project", "create", "--name", "Meetings E2E code", "--root", code, "--machine", machine.id]), "proj_");
  assert(projectId, "Project creation did not return an id");
  currentProjectId = projectId;
  const threadOne = findId(bb(["thread", "spawn", "--project", projectId, "--title", "E2E thread one", "--prompt", "Fixture only; do not run", "--send-at", "7d"]), "thr_");
  const threadTwo = findId(bb(["thread", "spawn", "--project", projectId, "--title", "E2E thread two", "--prompt", "Fixture only; do not run", "--send-at", "7d"]), "thr_");
  assert(threadOne && threadTwo, "Fixture threads did not return ids");

  if (macLane) writeFileSync(path.join(artifacts, "chrome.json"), `${JSON.stringify({ binary: startLocalChrome(),
    version: chromeBin ? "caller-supplied" : CHROME_FOR_TESTING_VERSION }, null, 2)}\n`);
  else command("docker", ["run", "-d", "--rm", "--name", chromeName, "--network", "host",
    "-v", `${path.dirname(chromeBin)}:/browser:ro`, "-v", `${wav}:/meeting.wav:ro`, image,
    "/browser/chrome", "--headless=new", "--no-sandbox", "--disable-gpu", `--remote-debugging-port=${cdpPort}`,
    "--remote-allow-origins=*", "--user-data-dir=/tmp/margins-browser-profile",
    "--use-fake-device-for-media-stream", "--use-file-for-fake-audio-capture=/meeting.wav",
    "--use-fake-ui-for-media-stream", "about:blank"]);
  await until("Chrome CDP", async () => (await fetch(`http://127.0.0.1:${cdpPort}/json/version`)).ok, macLane ? 60_000 : 20_000);
  browser(["record", "start", path.join(artifacts, "journey.webm"), `${bbEnv.BB_SERVER_URL}/plugins/margins/meetings`]);
  videoStarted = true;
  videoStartedAt = Date.now();
  browser(["set", "viewport", "1440", "900"]);
  // A macOS client also talks to Margins Menu's loopback bridge. Keep the
  // isolated page away from the user's running Menu.
  if (macLane) for (const port of menuBridgePorts) for (const host of ["127.0.0.1", "localhost"]) {
    browser(["network", "route", `http://${host}:${port}/**`, "--abort"]);
  }
  await until("Meetings page", () => browserEval('!!document.querySelector(".margins-meetings-page")'));
  await until("Meetings workspace choice", () => browserEval(`document.body.innerText.includes('Choose a Margins Workspace') || !![...document.querySelectorAll('button')].find(button => button.textContent === 'Start meeting')`), 60_000);
  if (browserEval(`document.body.innerText.includes('Choose a Margins Workspace')`)) {
    browser(["find", "role", "button", "click", "--name", "Use Workspace", "--exact"]);
    await until("Workspace selected", () => browserEval(`!document.body.innerText.includes('Choose a Margins Workspace')`));
  }
  if (freshRelease) await until("fresh release runtime install", () =>
    existsSync(path.join(freshCliBinDir, "margins"))
      && existsSync(path.join(hostData, "runtime", "v0.4.20", "margins-server")), 120_000);
  shot("01-workspace.png");

  // The fake microphone is this harness's deliberate browser-only choice.
  // A modal confirm can block headless Chrome's renderer before CDP can
  // acknowledge the click, so persist the same one-time choice first.
  browserEval('sessionStorage.setItem("margins.bb.browser-mic-confirmed", "yes"); true');
  // On a macOS client, Start meeting goes through Margins Menu; the browser-only path is explicit.
  if (macLane) await until("Browser mic only", () => browserEval(`!![...document.querySelectorAll('button')].find(button => button.textContent === 'Browser mic only')`), 60_000);
  browser(["find", "role", "button", "click", "--name", macLane ? "Browser mic only" : "Start meeting", "--exact"]);
  const startState = await until("Recording", () => browserEval(`document.querySelector('button[aria-label="Pause recording"]') ? 'recording' : document.querySelector('.margins-meetings-empty [role="alert"]')?.textContent || null`), 60_000);
  assert.equal(startState, "recording", `Start failed: ${startState}`);
  await until("meeting memo pad", () => browserEval(`!!document.querySelector('textarea[aria-label="Meeting memo pad"]')`), 60_000);
  const levels = [];
  for (let i = 0; i < 6; i++) {
    levels.push(Number(browserEval(`document.querySelector('[aria-label="Live audio level"]')?.getAttribute('data-level') || 0`)));
    await new Promise((resolve) => setTimeout(resolve, 450));
  }
  assert(levels.some((level) => level > 0));
  assert(new Set(levels).size > 1);
  assert(browserEval(`document.activeElement === document.querySelector('textarea[aria-label="Meeting memo pad"]')`));
  assert.equal(browserEval(`document.querySelector('textarea[aria-label="Meeting memo pad"]')?.getAttribute('placeholder')`), "Write notes...");
  assert(browserEval(`document.querySelector('.margins-meeting-kicker')?.innerText.includes('Recording')`));
  assert(browserEval(`[...document.querySelectorAll('.margins-meeting-list section button small')].every((small) => small.textContent !== 'Live')`));
  assert(browserEval(`!!document.querySelector('button[aria-label="Stop and save recording"]')`));
  assert.equal(browserEval(`document.querySelectorAll('.margins-meeting-pad button[aria-label="Pause"], .margins-meeting-pad button[aria-label="Stop and save"]').length`), 0);
  assert(browserEval(`document.querySelector('.margins-meetings-top button')?.classList.contains('is-reserved')`));
  shot("02-recording.png");
  const liveMemo = "Decision: ship the quiet Meetings view. Owner: Maya.";
  browser(["click", 'textarea[aria-label="Meeting memo pad"]']);
  browser(["keyboard", "type", liveMemo]);
  browser(["press", "Tab"]);
  const liveSessionId = await until("meeting route", () => browserEval(`(() => { const id = location.pathname.split('/').pop(); return id?.startsWith('browser-') ? id : null; })()`));
  await until("Live memo saved", async () => {
    const response = await fetch(`${bbEnv.BB_SERVER_URL}/api/v1/plugins/margins/rpc/readWorkspaceMeeting`, {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ projectId: currentProjectId, sessionId: liveSessionId }),
    });
    const value = await response.json();
    return value.result?.meeting?.notepad?.text === liveMemo;
  }, 60_000);
  assert(!browserEval(`document.querySelector('.margins-meeting-pad footer')?.innerText.includes('Saved')`));
  browser(["click", `a[aria-label="Open E2E thread two"]`]);
  await until("thread composer", () => browserEval(`!!document.querySelector('[aria-label^="Ask for a follow-up"]')`));
  assert(browserEval(`!!document.querySelector('button[aria-label="Pause recording"]')`));
  assert(browserEval(`document.querySelector('.margins-overlay')?.innerText.includes('Microphone only')`));
  const composerClearance = browserEval(`(() => {
    const pill = document.querySelector('.margins-overlay')?.getBoundingClientRect();
    const submit = document.querySelector('[aria-label^="Ask for a follow-up"]')?.getBoundingClientRect();
    if (!pill || !submit) return { pillFound: !!pill, promptFound: !!submit, clear: false };
    const gap = 8;
    return { pillFound: true, promptFound: true,
      clear: pill.right + gap <= submit.left || pill.left >= submit.right + gap
        || pill.bottom + gap <= submit.top || pill.top >= submit.bottom + gap,
      pill: { left: pill.left, top: pill.top, right: pill.right, bottom: pill.bottom },
      prompt: { left: submit.left, top: submit.top, right: submit.right, bottom: submit.bottom } };
  })()`);
  assert(composerClearance.clear, `Recording pill overlaps thread composer: ${JSON.stringify(composerClearance)}`);
  shot("03-overlay-other-thread.png");
  writeFileSync(path.join(artifacts, "03-capture-state.json"), `${JSON.stringify(browserEval(`({stored:sessionStorage.getItem('margins.bb.capture.v1'), navigation:performance.getEntriesByType('navigation').map(x=>x.type), url:location.href})`), null, 2)}\n`);
  writeFileSync(path.join(artifacts, "03-browser-console.txt"), browser(["console"]));
  otherThreadAt = Date.now();
  browser(["click", `a[aria-label="Open E2E thread one"]`]);
  browser(["find", "role", "button", "click", "--name", "Meetings", "--exact"]);
  await until("Live memo after thread switch", () => browserEval(`!!document.querySelector('textarea[aria-label="Meeting memo pad"]')`), 60_000);
  assertMemo(liveMemo);
  shot("03-thread-switch.png");
  writeFileSync(path.join(artifacts, "03-return-capture-state.json"), `${JSON.stringify(browserEval(`({stored:sessionStorage.getItem('margins.bb.capture.v1'), navigation:performance.getEntriesByType('navigation').map(x=>x.type), url:location.href})`), null, 2)}\n`);
  writeFileSync(path.join(artifacts, "03-return-browser-console.txt"), browser(["console"]));
  browser(["click", 'button[aria-label="Pause recording"]']);
  await until("Paused", () => browserEval(`!!document.querySelector('button[aria-label="Resume recording"]')`));
  assert.match(browserEval(`document.querySelector('.margins-overlay')?.innerText || ''`), /^Paused · \d+:\d{2}/);
  assert(browserEval(`document.querySelector('.margins-level-dot.paused') !== null`));
  await until("Paused meeting details", () => browserEval(`document.querySelector('.margins-meeting-kicker')?.innerText.includes('Paused') && document.querySelector('.margins-meeting-list')?.innerText.includes('Paused')`));
  shot("04-paused.png");
  browser(["click", 'button[aria-label="Resume recording"]']);
  await until("Resumed", () => browserEval(`!!document.querySelector('button[aria-label="Pause recording"]')`));
  browser(["click", 'button[aria-label="Stop and save recording"]']);
  await until("Ready to refine", () => browserEval(`document.querySelector('.margins-meeting-list')?.innerText.toLowerCase().includes('ready to refine') && !document.querySelector('button[aria-label="Stop and save recording"]')`));
  await until("Stop acknowledgment", () => browserEval(`(() => { const text = document.querySelector('.margins-meetings-page')?.innerText || ''; return text.includes('Saved ·') && text.includes('· e2e'); })()`), 4_000);
  assertMemo(liveMemo);
  assert(browserEval(`document.querySelector('.margins-meeting-next')?.innerText.includes('Make note →')`));
  shot("05-ready.png");
  const revisedMemo = `${liveMemo} Post-stop correction: include accessibility pass.`;
  const captureDir = path.join(home, "workspaces/e2e/captures/.margins");
  browser(["click", 'textarea[aria-label="Meeting memo pad"]']);
  browser(["keyboard", "type", " Post-stop correction: include accessibility pass."]);
  browser(["press", "Tab"]);
  await until("Revised memo saved", () => readdirSync(captureDir).some((name) =>
    name.endsWith(".md") && readFileSync(path.join(captureDir, name), "utf8").includes(revisedMemo)));
  browser(["find", "role", "button", "click", "--name", "Meetings", "--exact"]);
  assertMemo(revisedMemo);
  const memos = readdirSync(captureDir).filter((name) => name.endsWith(".md"));
  assert(memos.some((name) => readFileSync(path.join(captureDir, name), "utf8").includes(revisedMemo)));
  shot("06-revised.png");

  if (coldAsr && macLane) {
    await until("fresh CoreML model install", () => coremlInstalled(coldCoremlModel), 600_000);
    await new Promise((resolve) => setTimeout(resolve, 2_000));
  } else if (coldAsr) {
    const cache = path.join(temporary, "cache/margins/asr");
    await until("fresh ASR model and runtime install", () =>
      existsSync(path.join(cache, "parakeet-tdt-0.6b-v2-int8/encoder-model.int8.onnx"))
      && existsSync(path.join(cache, "parakeet-tdt-0.6b-v2-int8/decoder_joint-model.int8.onnx"))
      && existsSync(path.join(cache, "parakeet-tdt-0.6b-v2-int8/vocab.txt"))
      && existsSync(path.join(cache, "onnxruntime-linux-x64-1.24.2/lib/libonnxruntime.so.1.24.2")), 240_000);
    // The directory rename precedes the server's readiness transition.
    await new Promise((resolve) => setTimeout(resolve, 2_000));
  }
  browser(["find", "role", "button", "click", "--name", "Transcribe", "--exact"]);

  const transcript = await until("Parakeet transcript", () => {
    const result = jsonCommand(marginsBin, ["--workspace", "e2e", "transcript", "latest", "--format", "json"], { env: marginsEnv });
    const utterances = String(result.body || "").split("\n").filter((line) => /^\[\d{2}:\d{2}(?::\d{2})?\]/.test(line) && !/\] memo:/.test(line));
    return result.view === "aligned" && String(result.body).includes("Source: Margins remote offline speech transcript")
      && utterances.length > 0 ? result : null;
  }, coldAsr ? 240_000 : 120_000);
  writeFileSync(path.join(artifacts, "transcript.md"), `${transcript.body}\n`);
  writeFileSync(path.join(artifacts, "transcript.json"), `${JSON.stringify(transcript, null, 2)}\n`);
  if (macLane) {
    const bridgeRequests = JSON.parse(browser(["network", "requests", "--json"])).data.requests
      .filter((request) => menuBridgePorts.some((port) => new RegExp(`^https?://(127\\.0\\.0\\.1|localhost):${port}/`).test(request.url)))
      .map((request) => ({ method: request.method, url: request.url, status: request.status ?? null }));
    writeFileSync(path.join(artifacts, "menu-bridge-requests.json"), `${JSON.stringify(bridgeRequests, null, 2)}\n`);
    assert(bridgeRequests.every((request) => !request.status), `A request reached the Margins Menu bridge: ${JSON.stringify(bridgeRequests)}`);
  }
  const threadIds = (value) => [...JSON.stringify(value).matchAll(/thr_[a-z0-9]+/g)].map((match) => match[0]).sort();
  const beforeNoteThreads = threadIds(bb(["thread", "list", "--project", projectId]));
  assert(!existsSync(path.join(code, ".margins")), "Capture must not fall back to the bb project folder");
  shot("07-before-note.png");
  const assertions = { projectId, threadOne, threadTwo, levels, memoSavedAfterStop: true, stopAcknowledged: true,
    overlayVisibleOnOtherThread: true, overlayClearOfComposerSubmit: composerClearance,
    projectCaptureFallbackAbsent: true, transcription: macLane ? "coreml-asr" : "parakeet-asr", transcriptObserved: true,
    freshReleaseInstalled: freshRelease, coldAsrInstalled: coldAsr, noLlm: !realLlm,
    browserLane: macLane ? "macos-local-chrome" : "linux-docker-chrome", ...(macLane ? { menuBridgeUntouched: true } : {}) };
  writeFileSync(path.join(artifacts, "assertions.json"), `${JSON.stringify(assertions, null, 2)}\n`);
  if (process.env.MARGINS_E2E_HOLD_BEFORE_NOTE === "1") await holdBeforeNote();
  if (realLlm) {
    // Make note creates and starts the BB thread immediately. There is no
    // intermediate composer draft in the current product flow.
    writeFileSync(path.join(artifacts, "llm-send.marker"), `${new Date().toISOString()}\n`);
    browser(["find", "role", "button", "click", "--name", "Make note →", "--exact"]);
    const newThreadId = await until("new connected-note thread", () => {
      const ids = threadIds(bb(["thread", "list", "--project", projectId]));
      return ids.find((id) => !beforeNoteThreads.includes(id)) || null;
    }, 60_000);
    assertions.distillationThreadId = newThreadId;
    await until("connected-note navigation", () => browserEval(`location.pathname.includes('${newThreadId}')`));
    shot("09-thread-running.png");
    const noteName = await until("connected note in throwaway inbox", () => {
      const names = readdirSync(path.join(vault, "inbox")).filter((name) => name.endsWith(".md"));
      return names.find((name) => {
        const content = readFileSync(path.join(vault, "inbox", name), "utf8");
        return content.length > 100 ? name : false;
      });
    }, 8 * 60_000);
    const notePath = path.join(vault, "inbox", noteName);
    const noteText = readFileSync(notePath, "utf8");
    assert.match(noteText, /accessibility|quiet meetings/i, "Note omits the revised memo");
    assert.match(noteText, /curiosity/i, "Note omits the spoken transcript");
    assertions.noteRelativePath = `inbox/${noteName}`;
    assertions.memoReflected = true;
    assertions.transcriptReflected = true;
    await until("note association", () => {
      const result = command(marginsBin, ["--workspace", "e2e", "note-association", "meeting"], { env: marginsEnv });
      return result.includes(newThreadId) && result.includes(noteName);
    }, 90_000);
    browser(["find", "role", "button", "click", "--name", "Meetings", "--exact"]);
    await until("Distilled meeting", () => browserEval(`(() => {
      const list = document.querySelector('.margins-meeting-list');
      return !!list && [...list.querySelectorAll('h3')].some((h) => h.textContent === 'Distilled')
        && !!document.querySelector('.margins-meeting-links a')
        && !!document.querySelector('.margins-meeting-links button');
    })()`), 45_000);
    assert(!browserEval(`document.querySelector('.margins-meeting-links')?.innerText.includes('inbox/')`));
    assert(!browserEval(`document.querySelector('.margins-meeting-links')?.innerText.includes('${newThreadId}')`));
    shot("10-distilled.png");
    browser(["click", ".margins-meeting-links button"]);
    await until("distillation thread", () => browserEval(`location.pathname.includes('${newThreadId}')`));
    browser(["find", "role", "button", "click", "--name", "Show right panel (Ctrl + J)", "--exact"]);
    browser(["find", "role", "button", "click", "--name", "Open new tab (Ctrl + T)", "--exact"]);
    browser(["find", "role", "button", "click", "--name", "Margins", "--exact"]);
    await until("thread Margins source memo", () => browserEval(`document.querySelector('textarea[aria-label="Source meeting memo pad"]')?.value.includes('Post-stop correction: include accessibility pass.')`), 45_000);
    assertions.meetingsDistilled = true;
    assertions.threadMarginsMemo = true;
    shot("11-thread-margins.png");
    cpSync(vault, path.join(artifacts, "vault"), { recursive: true });
    writeFileSync(path.join(artifacts, "assertions.json"), `${JSON.stringify(assertions, null, 2)}\n`);
    passMessage = `PASS: real-LLM step 7; note ${path.join(artifacts, "vault/inbox", noteName)}; evidence ${artifacts}`;
  } else {
    browser(["record", "stop"]);
    videoStarted = false;
    const clipStart = Math.max(0, (otherThreadAt - videoStartedAt) / 1_000 - 2);
    command(ffmpeg, ["-nostdin", "-loglevel", "error", "-ss", String(clipStart), "-i", path.join(artifacts, "journey.webm"),
      "-t", "4", "-an", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-n", path.join(artifacts, "03-overlay-other-thread.mp4")]);
    assert(existsSync(path.join(artifacts, "03-overlay-other-thread.mp4")));
    passMessage = `PASS: no-LLM steps 1-6; evidence ${artifacts}`;
  }
} catch (error) {
  if (videoStarted && !interruptedSignal) {
    try { shot("failure.png"); } catch { /* browser may be gone */ }
    try { writeFileSync(path.join(artifacts, "failure-snapshot.txt"), browser(["snapshot", "-i"])); } catch { /* browser may be gone */ }
    try { writeFileSync(path.join(artifacts, "failure-state.json"), `${JSON.stringify(browserEval(`({url:location.href,text:document.body.innerText})`), null, 2)}\n`); } catch { /* browser may be gone */ }
    try { writeFileSync(path.join(artifacts, "failure-browser-console.txt"), browser(["console"])); } catch { /* browser may be gone */ }
    try {
      const requests = JSON.parse(browser(["network", "requests", "--json"])).data.requests;
      writeFileSync(path.join(artifacts, "failure-network.json"), `${JSON.stringify(requests.map((request) => ({
        method: request.method, url: request.url, status: request.status,
        bodyBytes: request.postData?.length || 0, timestamp: request.timestamp,
      })), null, 2)}\n`);
    } catch { /* browser may be gone */ }
  }
  if (currentProjectId && bbEnv?.BB_SERVER_URL && browserUsed) {
    try {
      const sessionId = browserEval(`location.pathname.split('/').pop()`);
      const response = await fetch(`${bbEnv.BB_SERVER_URL}/api/v1/plugins/margins/rpc/readWorkspaceMeeting`, {
        method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ projectId: currentProjectId, sessionId }), signal: AbortSignal.timeout(8_000),
      });
      writeFileSync(path.join(artifacts, "failure-meeting-rpc.json"), `${JSON.stringify({ status: response.status, body: await response.json() }, null, 2)}\n`);
    } catch (cause) {
      writeFileSync(path.join(artifacts, "failure-meeting-rpc.txt"), String(cause));
    }
  }
  if (existsSync(path.join(bbData, "logs"))) cpSync(path.join(bbData, "logs"), path.join(artifacts, "bb-diagnostic-logs"), { recursive: true });
  if (realLlm && existsSync(vault)) cpSync(vault, path.join(artifacts, "vault"), { recursive: true });
  if (interruptedSignal) process.stderr.write(`Harness interrupted by ${interruptedSignal}; cleaning up.\n`);
  else throw error;
} finally {
  if (coldAsr && macLane) {
    const installed = { modelInitiallyAbsent: true, modelDir: coldCoremlModel,
      modelInstalled: coremlInstalled(coldCoremlModel), asrBackend: "coreml",
      releaseRuntimeInstalled: freshRelease
        ? existsSync(path.join(bbData, "plugins/margins/host-data/runtime/v0.4.20/margins-server")) : null };
    writeFileSync(path.join(artifacts, "cold-asr-install.json"), `${JSON.stringify(installed, null, 2)}\n`);
    if (existsSync(path.join(bbData, "logs")) && !existsSync(path.join(artifacts, "bb-diagnostic-logs"))) {
      cpSync(path.join(bbData, "logs"), path.join(artifacts, "bb-diagnostic-logs"), { recursive: true });
    }
    if (passMessage) assert(installed.modelInstalled, "Cold CoreML model was not installed");
  } else if (coldAsr) {
    const model = path.join(temporary, "cache/margins/asr/parakeet-tdt-0.6b-v2-int8");
    const runtime = path.join(temporary, "cache/margins/asr/onnxruntime-linux-x64-1.24.2/lib/libonnxruntime.so.1.24.2");
    const installed = {
      modelInitiallyAbsent: true,
      modelInstalled: existsSync(path.join(model, "encoder-model.int8.onnx"))
        && existsSync(path.join(model, "decoder_joint-model.int8.onnx"))
        && existsSync(path.join(model, "vocab.txt")),
      runtimeInstalled: existsSync(runtime),
    };
    writeFileSync(path.join(artifacts, "cold-asr-install.json"), `${JSON.stringify(installed, null, 2)}\n`);
    if (existsSync(path.join(bbData, "logs")) && !existsSync(path.join(artifacts, "bb-diagnostic-logs"))) {
      cpSync(path.join(bbData, "logs"), path.join(artifacts, "bb-diagnostic-logs"), { recursive: true });
    }
    if (passMessage) assert(installed.modelInstalled && installed.runtimeInstalled, "Cold ASR assets were not installed");
  }
  if (videoStarted && !browserCloseResult) try { browser(["record", "stop"]); } catch { /* keep prior evidence */ }
  const browserCleanup = await verifyBrowserClosed();
  writeFileSync(path.join(artifacts, "browser-cleanup.json"), `${JSON.stringify(browserCleanup, null, 2)}\n`);
  const assertionsPath = path.join(artifacts, "assertions.json");
  if (existsSync(assertionsPath)) {
    const assertions = JSON.parse(readFileSync(assertionsPath, "utf8"));
    assertions.browserDaemonReaped = browserCleanup.verified;
    writeFileSync(assertionsPath, `${JSON.stringify(assertions, null, 2)}\n`);
  }
  if (!macLane) try { command("docker", ["rm", "-f", chromeName]); } catch { /* already stopped */ }
  // Snapshot every descendant first: bb-spawned recorders outlive a killed parent.
  const snapshot = processTable();
  const owned = new Map(snapshot.filter((entry) => [bbProcess?.pid, chromeProcess?.pid].includes(entry.pid)).map((entry) => [entry.pid, entry.command]));
  for (let grew = true; grew;) {
    grew = false;
    for (const entry of snapshot) if (owned.has(entry.ppid) && !owned.has(entry.pid)) { owned.set(entry.pid, entry.command); grew = true; }
  }
  if (chromeProcess?.exitCode === null) try { process.kill(-chromeProcess.pid, "SIGTERM"); } catch { /* already exited */ }
  if (bbProcess) { bbProcess.kill("SIGINT"); await new Promise((resolve) => setTimeout(resolve, 1200)); if (bbProcess.exitCode === null) bbProcess.kill("SIGKILL"); }
  const tempTag = path.basename(temporary);
  const leftovers = () => processTable().filter((entry) => entry.pid !== process.pid
    && (owned.get(entry.pid) === entry.command || entry.command.includes(tempTag))).map((entry) => entry.pid);
  for (const signal of ["SIGTERM", "SIGKILL"]) {
    for (const pid of leftovers()) try { process.kill(pid, signal); } catch { /* already exited */ }
    const deadline = Date.now() + 3_000;
    while (leftovers().length && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 100));
  }
  const processCleanup = { ownedPids: [...owned.keys()], remaining: leftovers() };
  writeFileSync(path.join(artifacts, "process-cleanup.json"), `${JSON.stringify(processCleanup, null, 2)}\n`);
  if (process.env.MARGINS_E2E_KEEP_TEMP === "1") console.log(`Diagnostic temp kept: ${temporary}`);
  else rmSync(temporary, { recursive: true, force: true });
  assert(browserCleanup.verified, `agent-browser daemon remained for ${browserSession}: ${JSON.stringify(browserCleanup)}`);
  assert.deepEqual(processCleanup.remaining, [], "Harness processes remained after cleanup");
}
if (passMessage) console.log(`${passMessage}; browser daemon reaped`);
