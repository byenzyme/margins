#!/usr/bin/env node
// Isolated bb + Chrome journey for MEETINGS_SPEC steps 1-6. No composer submit.
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
const marginsBin = required("MARGINS_E2E_BIN");
const serverBin = required("MARGINS_E2E_SERVER_BIN");
const bbApp = required("MARGINS_E2E_BB_APP");
const chromeBin = required("MARGINS_E2E_CHROME_BIN");
const spokenWav = required("MARGINS_E2E_SPOKEN_WAV");
const asrModelDir = required("MARGINS_E2E_ASR_MODEL_DIR");
const ortLibrary = required("MARGINS_E2E_ORT_LIBRARY");
const image = process.env.MARGINS_E2E_CHROME_IMAGE || "margins-bb-e2e-chrome:local";
const artifacts = path.resolve(process.env.MARGINS_E2E_ARTIFACTS || path.join(plugin, "e2e-artifacts", new Date().toISOString().replace(/[:.]/g, "-")));
const temporary = mkdtempSync(path.join(os.tmpdir(), "margins-bb-meetings-"));
const home = path.join(temporary, "margins-home");
const vault = path.join(temporary, "vault");
const code = path.join(temporary, "code");
const bbData = path.join(temporary, "bb");
const wav = path.join(temporary, "meeting.wav");
const browserConfig = path.join(temporary, "agent-browser.json");
const chromeName = `margins-bb-e2e-${process.pid}`;
const browserSession = `margins-bb-e2e-${process.pid}`;
let bbProcess;
let videoStarted = false;

mkdirSync(artifacts, { recursive: true });
mkdirSync(home, { recursive: true });
mkdirSync(code, { recursive: true });
cpSync(path.join(repo, "desktop/test-harness/local-e2e/seed-vault"), vault, { recursive: true });
mkdirSync(path.join(vault, "inbox"), { recursive: true });
writeFileSync(browserConfig, "{}\n");

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
  // Repeat the short spoken fixture so the browser capture spans the whole journey.
  command("ffmpeg", ["-nostdin", "-loglevel", "error", "-stream_loop", "9", "-i", source,
    "-t", "40", "-ac", "1", "-ar", "16000", "-acodec", "pcm_s16le", "-y", file]);
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
    try { const result = await probe(); if (result) return result; }
    catch (error) { last = error; }
    await new Promise((resolve) => setTimeout(resolve, 400));
  }
  throw new Error(`${label} did not become ready: ${last || "timeout"}`);
}

function browser(args) {
  return command("agent-browser", ["--session", browserSession, "--cdp", String(cdpPort), ...args], {
    env: { ...process.env, AGENT_BROWSER_CONFIG: browserConfig },
  });
}
function browserEval(source) {
  return jsonCommand("agent-browser", ["--session", browserSession, "--cdp", String(cdpPort), "--json", "eval", source], {
    env: { ...process.env, AGENT_BROWSER_CONFIG: browserConfig },
  }).data.result;
}
function shot(name) { browser(["screenshot", path.join(artifacts, name)]); }
function bb(args) { return jsonCommand("bb", [...args, "--json"], { env: bbEnv }); }
function assertMemo(text) {
  assert.equal(browserEval(`document.querySelector('textarea[aria-label="Meeting memo pad"]')?.value`), text);
}

let cdpPort;
let bbEnv;
try {
  assert(!home.startsWith(path.join(os.homedir(), ".margins")));
  assert(!vault.startsWith("/workspace/obsidian"));
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
  bbEnv = { ...process.env, BB_SERVER_URL: `http://127.0.0.1:${serverPort}`, BB_DATA_DIR: bbData };
  const hostData = path.join(bbData, "plugins/margins/host-data");
  mkdirSync(hostData, { recursive: true });
  writeFileSync(path.join(hostData, "asr-runtime.json"), `${JSON.stringify({ serverPath: serverBin,
    modelDir: asrModelDir, ortLibraryPath: ortLibrary })}\n`);
  const bbOut = openSync(path.join(artifacts, "bb.log"), "w");
  const bbErr = openSync(path.join(artifacts, "bb-errors.log"), "w");
  bbProcess = spawn(bbApp, ["--data-dir", bbData, "--server-bind-host", "0.0.0.0", "--server-port", String(serverPort), "--host-daemon-port", String(daemonPort), "start"], {
    env: { ...bbEnv, MARGINS_HOME: home, MARGINS_PROJECT_SERVER_PATH: serverBin },
    stdio: ["ignore", bbOut, bbErr],
  });
  closeSync(bbOut); closeSync(bbErr);
  await until("isolated bb", () => { try { bb(["project", "list"]); return true; } catch { return false; } }, 30_000);
  const machine = await until("isolated bb machine", () => {
    try { return bb(["machine", "list"]).find((host) => host.status === "connected"); }
    catch { return null; }
  }, 30_000);
  command("bb", ["plugin", "install", plugin, "--yes"], { env: bbEnv });
  const projectId = findId(bb(["project", "create", "--name", "Meetings E2E code", "--root", code, "--machine", machine.id]), "proj_");
  assert(projectId, "Project creation did not return an id");
  const threadOne = findId(bb(["thread", "spawn", "--project", projectId, "--title", "E2E thread one", "--prompt", "Fixture only; do not run", "--send-at", "7d"]), "thr_");
  const threadTwo = findId(bb(["thread", "spawn", "--project", projectId, "--title", "E2E thread two", "--prompt", "Fixture only; do not run", "--send-at", "7d"]), "thr_");
  assert(threadOne && threadTwo, "Fixture threads did not return ids");
  const beforeThreads = bb(["thread", "list", "--project", projectId]);

  command("docker", ["run", "-d", "--rm", "--name", chromeName, "--network", "host",
    "-v", `${path.dirname(chromeBin)}:/browser:ro`, "-v", `${wav}:/meeting.wav:ro`, image,
    "/browser/chrome", "--headless=new", "--no-sandbox", "--disable-gpu", `--remote-debugging-port=${cdpPort}`,
    "--remote-allow-origins=*", "--user-data-dir=/tmp/margins-browser-profile",
    "--use-fake-device-for-media-stream", "--use-file-for-fake-audio-capture=/meeting.wav",
    "--use-fake-ui-for-media-stream", "about:blank"]);
  await until("Chrome CDP", async () => (await fetch(`http://127.0.0.1:${cdpPort}/json/version`)).ok);
  browser(["record", "start", path.join(artifacts, "journey.webm"), `${bbEnv.BB_SERVER_URL}/plugins/margins/meetings`]);
  videoStarted = true;
  browser(["set", "viewport", "1440", "900"]);
  await until("Meetings page", () => browserEval('!!document.querySelector(".margins-meetings-page")'));
  shot("01-workspace.png");

  browser(["find", "role", "button", "click", "--name", "Start meeting", "--exact"]);
  const startState = await until("Recording", () => browserEval(`document.querySelector('button[aria-label="Pause recording"]') ? 'recording' : document.querySelector('.margins-meetings-empty [role="alert"]')?.textContent || null`), 60_000);
  assert.equal(startState, "recording", `Start failed: ${startState}`);
  const levels = [];
  for (let i = 0; i < 6; i++) {
    levels.push(Number(browserEval(`document.querySelector('[aria-label="Live audio level"]')?.getAttribute('data-level') || 0`)));
    await new Promise((resolve) => setTimeout(resolve, 450));
  }
  assert(levels.some((level) => level > 0));
  assert(new Set(levels).size > 1);
  assert(browserEval(`!!document.querySelector('button[aria-label="Stop and save recording"]')`));
  assert.equal(browserEval(`document.querySelectorAll('.margins-meeting-pad button[aria-label="Pause"], .margins-meeting-pad button[aria-label="Stop and save"]').length`), 0);
  assert.equal(browserEval(`document.querySelectorAll('.margins-meetings-top button').length`), 0);
  shot("02-recording.png");
  const liveMemo = "Decision: ship the quiet Meetings view. Owner: Maya.";
  browser(["fill", 'textarea[aria-label="Meeting memo pad"]', liveMemo]);
  await until("Live memo saved", () => browserEval('document.querySelector(".margins-meeting-pad footer")?.innerText.includes("Saved")'));
  browser(["click", `a[aria-label="Open E2E thread two"]`]);
  assert(browserEval(`!!document.querySelector('button[aria-label="Pause recording"]')`));
  browser(["click", `a[aria-label="Open E2E thread one"]`]);
  browser(["find", "role", "button", "click", "--name", "Meetings", "--exact"]);
  await until("Live memo after thread switch", () => browserEval(`!!document.querySelector('textarea[aria-label="Meeting memo pad"]')`));
  assertMemo(liveMemo);
  shot("03-thread-switch.png");
  browser(["click", 'button[aria-label="Pause recording"]']);
  await until("Paused", () => browserEval(`!!document.querySelector('button[aria-label="Resume recording"]')`));
  shot("04-paused.png");
  browser(["click", 'button[aria-label="Resume recording"]']);
  await until("Resumed", () => browserEval(`!!document.querySelector('button[aria-label="Pause recording"]')`));
  browser(["click", 'button[aria-label="Stop and save recording"]']);
  await until("Ready to refine", () => browserEval(`document.querySelector('.margins-meeting-list')?.innerText.toLowerCase().includes('ready to refine') && !document.querySelector('button[aria-label="Stop and save recording"]')`));
  assertMemo(liveMemo);
  assert(browserEval(`document.querySelector('.margins-meeting-next')?.innerText.includes('Memo stays editable.')`));
  shot("05-ready.png");
  const revisedMemo = `${liveMemo} Post-stop correction: include accessibility pass.`;
  browser(["fill", 'textarea[aria-label="Meeting memo pad"]', revisedMemo]);
  await until("Revised memo saved", () => browserEval('document.querySelector(".margins-meeting-pad footer")?.innerText.includes("Saved")'));
  browser(["find", "role", "button", "click", "--name", "Meetings", "--exact"]);
  assertMemo(revisedMemo);
  const captureDir = path.join(home, "workspaces/e2e/captures/.margins");
  const memos = readdirSync(captureDir).filter((name) => name.endsWith(".md"));
  assert(memos.some((name) => readFileSync(path.join(captureDir, name), "utf8").includes(revisedMemo)));
  shot("06-revised.png");

  browser(["find", "role", "button", "click", "--name", "Make note in new thread →", "--exact"]);
  await until("Composer", () => browserEval(`document.querySelector('[role="textbox"]')?.textContent?.includes('<margins-context-v1>')`), 120_000);
  const prompt = browserEval(`document.querySelector('[role="textbox"]')?.innerText || ''`);
  const plainPrompt = prompt.split("<margins-context-v1>")[0].trim();
  assert.match(plainPrompt, /^Make a connected note from my meeting on [A-Za-z]+ \d+, \d{4}\.$/);
  const contextMatch = prompt.match(/<margins-context-v1>\s*(\{[^\n]+\})\s*<\/margins-context-v1>/);
  assert(contextMatch, "Composer lacks a delimited Margins context block");
  const composerContext = JSON.parse(contextMatch[1]);
  assert.deepEqual({ workspaceId: composerContext.workspaceId, sessionId: composerContext.sessionId,
    bbProjectId: composerContext.bbProjectId, note: composerContext.note },
  { workspaceId: "e2e", sessionId: "meeting", bbProjectId: projectId, note: "create" });
  assert.match(composerContext.memoRevision, /^v\d+-/);
  assert.equal(composerContext.transcript, "ready");
  assert(!/\(\d{3}\)/.test(prompt), "Composer draft includes a raw HTTP error");
  const transcript = await until("Parakeet transcript", () => {
    const result = jsonCommand(marginsBin, ["--workspace", "e2e", "transcript", "latest", "--format", "json"], { env: marginsEnv });
    const utterances = String(result.body || "").split("\n").filter((line) => /^\[\d{2}:\d{2}(?::\d{2})?\]/.test(line) && !/\] memo:/.test(line));
    return result.view === "aligned" && String(result.body).includes("Source: Headless parakeet-onnx transcript")
      && utterances.length > 0 ? result : null;
  }, 120_000);
  writeFileSync(path.join(artifacts, "transcript.json"), `${JSON.stringify(transcript, null, 2)}\n`);
  // The SDK's toCompose has no project argument. Select the fixture project
  // through bb's composer picker without submitting the prompt.
  browser(["find", "role", "button", "click", "--name", "Project: Work in a project", "--exact"]);
  browser(["find", "role", "option", "click", "--name", "Meetings E2E code", "--exact"]);
  assert(browserEval(`!!document.querySelector('button[aria-label="Project: Meetings E2E code"]')`));
  const afterThreads = bb(["thread", "list", "--project", projectId]);
  const threadIds = (value) => [...JSON.stringify(value).matchAll(/thr_[a-z0-9]+/g)].map((match) => match[0]).sort();
  assert.deepEqual(threadIds(afterThreads), threadIds(beforeThreads), "Distill must not spawn or send a thread");
  assert(!existsSync(path.join(code, ".margins")), "Capture must not fall back to the bb project folder");
  shot("07-composer.png");
  writeFileSync(path.join(artifacts, "assertions.json"), `${JSON.stringify({ projectId, threadOne, threadTwo, levels, memoSavedAfterStop: true, projectCaptureFallbackAbsent: true, composerPrompt: prompt, composerPlainPrompt: plainPrompt, composerContext, threadsUnchanged: true, transcription: "parakeet-asr", transcriptObserved: true, noLlm: true }, null, 2)}\n`);
  console.log(`PASS: no-LLM steps 1-6; evidence ${artifacts}`);
} catch (error) {
  if (videoStarted) {
    try { shot("failure.png"); } catch { /* browser may be gone */ }
    try { writeFileSync(path.join(artifacts, "failure-snapshot.txt"), browser(["snapshot", "-i"])); } catch { /* browser may be gone */ }
    try { writeFileSync(path.join(artifacts, "failure-state.json"), `${JSON.stringify(browserEval(`({url:location.href,text:document.body.innerText})`), null, 2)}\n`); } catch { /* browser may be gone */ }
    try { writeFileSync(path.join(artifacts, "failure-browser-console.txt"), browser(["console"])); } catch { /* browser may be gone */ }
  }
  if (existsSync(path.join(bbData, "logs"))) cpSync(path.join(bbData, "logs"), path.join(artifacts, "bb-diagnostic-logs"), { recursive: true });
  throw error;
} finally {
  if (videoStarted) try { browser(["record", "stop"]); } catch { /* keep prior evidence */ }
  try { command("docker", ["rm", "-f", chromeName]); } catch { /* already stopped */ }
  if (bbProcess) { bbProcess.kill("SIGINT"); await new Promise((resolve) => setTimeout(resolve, 1200)); if (bbProcess.exitCode === null) bbProcess.kill("SIGKILL"); }
  if (process.env.MARGINS_E2E_KEEP_TEMP === "1") console.log(`Diagnostic temp kept: ${temporary}`);
  else rmSync(temporary, { recursive: true, force: true });
}
