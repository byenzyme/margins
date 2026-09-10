import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { delay } from "./timing.mjs";

export const CDP_HOST = process.env.MARGINS_CDP_HOST || "127.0.0.1";
export const CDP_PORT = Number(process.env.MARGINS_CDP_PORT || 9222);

export async function ensureChrome() {
  if (await cdpJson("/json/version", { quiet: true })) return { launched: false };

  const plan = chromeLaunchPlan();
  spawn(plan.command, plan.args, { detached: true, stdio: "ignore", windowsHide: true }).unref();

  const started = Date.now();
  while (Date.now() - started < 8000) {
    await delay(250);
    if (await cdpJson("/json/version", { quiet: true })) {
      await closeBlankTargets();
      return { launched: true };
    }
  }
  throw new Error(`Chrome CDP did not start on ${CDP_HOST}:${CDP_PORT}`);
}

export function chromeLaunchPlan() {
  if (!canLaunchForHost(CDP_HOST)) {
    throw new Error(`No CDP endpoint is available at ${CDP_HOST}:${CDP_PORT}, and this harness can only launch a local browser. Start Chrome with remote debugging yourself or set MARGINS_CDP_HOST to a local address.`);
  }

  const defaultProfile = process.platform === "win32" ? join(tmpdir(), "margins-ux-chrome") : "/tmp/margins-ux-chrome";
  const profile = process.env.MARGINS_CHROME_USER_DATA_DIR || defaultProfile;
  const chromeArgs = [
    `--remote-debugging-port=${CDP_PORT}`,
    `--user-data-dir=${profile}`,
    "--no-first-run",
    "--no-default-browser-check",
    "about:blank",
  ];
  const headless = shouldLaunchHeadless();
  if (headless) {
    chromeArgs.splice(2, 0, "--headless=new", "--disable-gpu");
  }

  if (process.platform === "darwin" && !process.env.MARGINS_CHROME_BIN && !headless) {
    return {
      mode: "mac-open",
      command: "open",
      args: ["-na", "Google Chrome", "--args", ...chromeArgs],
      headless,
      profile,
    };
  }

  const browser = findChromeBin();
  if (!browser) {
    throw new Error("Chrome/Chromium executable not found. Install Chrome/Chromium or set MARGINS_CHROME_BIN to the browser executable.");
  }
  return {
    mode: "direct",
    command: browser,
    args: chromeArgs,
    headless,
    profile,
  };
}

function shouldLaunchHeadless() {
  const value = process.env.MARGINS_UX_HEADLESS;
  if (value !== undefined) return !["0", "false", "no", "off"].includes(value.toLowerCase());
  return process.env.CI === "1" || process.env.CI === "true" || process.platform === "linux";
}

function canLaunchForHost(host) {
  return ["127.0.0.1", "localhost", "::1", "0.0.0.0"].includes(host);
}

function findChromeBin() {
  const candidates = [
    process.env.MARGINS_CHROME_BIN,
    process.platform === "darwin" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : null,
    process.platform === "darwin" ? "/Applications/Chromium.app/Contents/MacOS/Chromium" : null,
    process.platform === "win32" ? join(process.env.PROGRAMFILES || "C:\\Program Files", "Google", "Chrome", "Application", "chrome.exe") : null,
    process.platform === "win32" ? join(process.env["PROGRAMFILES(X86)"] || "C:\\Program Files (x86)", "Google", "Chrome", "Application", "chrome.exe") : null,
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "chrome",
  ].filter(Boolean);

  for (const candidate of candidates) {
    if (candidate.includes("/") || candidate.includes("\\")) {
      if (existsSync(candidate)) return candidate;
      continue;
    }
    const resolved = resolveCommand(candidate);
    if (resolved) return resolved;
  }
  return null;
}

function resolveCommand(command) {
  const lookup = process.platform === "win32" ? "where" : "which";
  const result = spawnSync(lookup, [command], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
  if (result.status !== 0) return null;
  return result.stdout.split(/\r?\n/).map(line => line.trim()).find(Boolean) || null;
}

export async function cdpJson(path, { quiet = false, method = "GET" } = {}) {
  try {
    const res = await fetch(`http://${CDP_HOST}:${CDP_PORT}${path}`, { method });
    if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
    return await res.json();
  } catch (err) {
    if (!quiet) throw err;
    return null;
  }
}

export async function listTargets() {
  return await cdpJson("/json/list", { quiet: true }) || [];
}

export async function closeTarget(targetId) {
  if (!targetId) return false;
  for (const method of ["PUT", "GET"]) {
    try {
      const res = await fetch(`http://${CDP_HOST}:${CDP_PORT}/json/close/${encodeURIComponent(targetId)}`, { method });
      if (res.ok) return true;
    } catch {
      // Try the next method before giving up. Chrome accepts GET in many
      // versions; newer builds prefer PUT for /json/new but still usually
      // tolerate GET for /json/close.
    }
  }
  return false;
}

export async function closeBlankTargets() {
  const targets = await listTargets();
  await Promise.all(targets
    .filter(t => t.type === "page" && (t.url === "about:blank" || t.url === "chrome://newtab/"))
    .map(t => closeTarget(t.id)));
}

export async function closeBrowser() {
  const version = await cdpJson("/json/version", { quiet: true });
  const browserWs = version?.webSocketDebuggerUrl;
  if (!browserWs || typeof WebSocket === "undefined") return false;

  const ws = new WebSocket(browserWs);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", reject, { once: true });
  });

  ws.send(JSON.stringify({ id: 1, method: "Browser.close" }));
  await new Promise(resolve => {
    const timeout = setTimeout(resolve, 1000);
    ws.addEventListener("close", () => {
      clearTimeout(timeout);
      resolve();
    }, { once: true });
  });
  return true;
}
