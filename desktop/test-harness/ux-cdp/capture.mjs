import { readFileSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { cdpJson, closeTarget } from "./chrome.mjs";
import { CdpClient } from "./cdp-client.mjs";
import { delay } from "./timing.mjs";

export const DEFAULT_BASE = process.env.MARGINS_UX_BASE || "http://localhost:5173";
export const DEFAULT_OUT = process.env.MARGINS_UX_OUT || "ux-shots";

// Keep preview screenshots a consistent size that matches the real desktop
// window. The viewport is read from tauri.conf.json so the harness and the
// shipped app window can never drift apart; deviceScaleFactor mimics a Retina
// Mac display. Override the scale with MARGINS_UX_DPR if needed.
export const VIEWPORT = resolveViewport();

function resolveViewport() {
  const dpr = Number(process.env.MARGINS_UX_DPR || 2) || 2;
  const fallback = { width: 1280, height: 860, deviceScaleFactor: dpr };
  try {
    const here = dirname(fileURLToPath(import.meta.url));
    const confPath = resolve(here, "../../src-tauri/tauri.conf.json");
    const conf = JSON.parse(readFileSync(confPath, "utf8"));
    const win = conf?.app?.windows?.[0];
    if (win?.width && win?.height) {
      return { width: Math.round(win.width), height: Math.round(win.height), deviceScaleFactor: dpr };
    }
  } catch {
    // tauri.conf.json missing/unreadable — fall back to the shipped default.
  }
  return fallback;
}
export const DEFAULT_SCENARIOS = [
  "home-real",
  "settings-audio",
  "recording-healthy",
  "recording-dead-tap",
  "distill-complete",
];
export const DEFAULT_REAL_VIEWS = [
  "home",
  "settings-audio",
];

export async function openScenario(base, scenario) {
  const url = `${base.replace(/\/$/, "")}/?scenario=${encodeURIComponent(scenario)}`;
  return openUrl(url, { waitForMock: true });
}

export async function openRealView(base, view) {
  const params = new URLSearchParams({ backend: "http" });
  if (view !== "home") params.set("view", view);
  const url = `${base.replace(/\/$/, "")}/?${params.toString()}`;
  return openUrl(url, { waitForMock: false, view });
}

async function openUrl(url, { waitForMock, view } = {}) {
  let target;
  let client;
  try {
    try {
      target = await cdpJson(`/json/new?${encodeURIComponent(url)}`, { method: "PUT" });
    } catch {
      target = await cdpJson(`/json/new?${encodeURIComponent(url)}`);
    }

    client = await CdpClient.connect(target.webSocketDebuggerUrl);
    await client.send("Page.enable");
    await client.send("Runtime.enable");
    await client.send("Accessibility.enable").catch(() => undefined);
    // Pin every preview to the same viewport as the real desktop window so
    // screenshots are a consistent, comparable size run-to-run.
    await client.send("Emulation.setDeviceMetricsOverride", {
      width: VIEWPORT.width,
      height: VIEWPORT.height,
      deviceScaleFactor: VIEWPORT.deviceScaleFactor,
      mobile: false,
    }).catch(() => undefined);
    await waitForLoad(client);
    await client.send("Runtime.evaluate", {
      expression: "document.fonts && document.fonts.ready",
      awaitPromise: true,
    }).catch(() => undefined);
    // Gate on the mock backend having run its first command before settling.
    // On a cold Vite start the mock module (incl. the snapshot fetch) compiles
    // lazily; without this the screenshot can catch the pre-data render.
    if (waitForMock) await waitForMockReady(client);
    else await waitForAppReady(client);
    if (view) await prepareRealView(client, view);
    await delay(400);
    return { client, url, targetId: target.id };
  } catch (err) {
    client?.close();
    await closeTarget(target?.id);
    throw err;
  }
}

export async function captureScenario(base, scenario, outDir) {
  const page = await openScenario(base, scenario);
  const { client, url, targetId } = page;
  try {
    if (scenario.endsWith("-narrow")) {
      await client.send("Emulation.setDeviceMetricsOverride", {
        width: 360,
        height: VIEWPORT.height,
        deviceScaleFactor: VIEWPORT.deviceScaleFactor,
        mobile: false,
      }).catch(() => undefined);
      await client.send("Runtime.evaluate", {
        expression: "window.__toggleSidebar && window.__toggleSidebar()",
        awaitPromise: true,
      }).catch(() => undefined);
      await delay(200);
    }
    const safe = scenario.replace(/[^a-z0-9-]+/gi, "-");
    const screenshotPath = resolve(outDir, `${safe}.png`);
    const textPath = resolve(outDir, `${safe}.txt`);
    const axPath = resolve(outDir, `${safe}.ax.json`);
    const consolePath = resolve(outDir, `${safe}.console.json`);

    const logs = [];
    client.on("Runtime.consoleAPICalled", evt => {
      logs.push({
        type: evt.type,
        args: evt.args?.map(a => a.value ?? a.description),
        timestamp: evt.timestamp,
      });
    });

    await prepareScenarioForCapture(client, scenario);

    const text = await evalValue(client, "document.body.innerText");
    const title = await evalValue(client, "document.title");
    const appRect = await evalValue(client, `(() => { const r = document.querySelector('#app')?.getBoundingClientRect(); return r ? { width: r.width, height: r.height } : null })()`);
    const ax = await client.send("Accessibility.getFullAXTree").catch(err => ({ error: String(err) }));
    // captureBeyondViewport:false keeps the shot clipped to the window-sized
    // viewport, exactly what the real app shows (overflowing lists scroll in
    // the app rather than expanding the window), so sizes stay consistent.
    const shot = await client.send("Page.captureScreenshot", {
      format: "png",
      captureBeyondViewport: false,
      fromSurface: true,
    });

    await writeFile(screenshotPath, Buffer.from(shot.data, "base64"));
    await writeFile(textPath, text || "", "utf8");
    await writeFile(axPath, JSON.stringify(ax, null, 2));
    await writeFile(consolePath, JSON.stringify(logs, null, 2));

    return {
      scenario,
      url,
      targetId,
      title,
      appRect,
      screenshotPath,
      textPath,
      axPath,
      consolePath,
      textPreview: summarizeText(text || ""),
    };
  } finally {
    client.close();
    await closeTarget(targetId);
  }
}

export async function captureRealView(base, view, outDir) {
  const page = await openRealView(base, view);
  const { client, url, targetId } = page;
  try {
    const safe = `real-${view}`.replace(/[^a-z0-9-]+/gi, "-");
    const screenshotPath = resolve(outDir, `${safe}.png`);
    const textPath = resolve(outDir, `${safe}.txt`);
    const axPath = resolve(outDir, `${safe}.ax.json`);
    const consolePath = resolve(outDir, `${safe}.console.json`);

    const logs = [];
    client.on("Runtime.consoleAPICalled", evt => {
      logs.push({
        type: evt.type,
        args: evt.args?.map(a => a.value ?? a.description),
        timestamp: evt.timestamp,
      });
    });

    const text = await evalValue(client, "document.body.innerText");
    const title = await evalValue(client, "document.title");
    const appRect = await evalValue(client, `(() => { const r = document.querySelector('#app')?.getBoundingClientRect(); return r ? { width: r.width, height: r.height } : null })()`);
    const backend = await evalValue(client, "window.__MARGINS_TOKEN__ ? 'http' : 'unknown'").catch(() => "unknown");
    const ax = await client.send("Accessibility.getFullAXTree").catch(err => ({ error: String(err) }));
    const shot = await client.send("Page.captureScreenshot", {
      format: "png",
      captureBeyondViewport: false,
      fromSurface: true,
    });

    await writeFile(screenshotPath, Buffer.from(shot.data, "base64"));
    await writeFile(textPath, text || "", "utf8");
    await writeFile(axPath, JSON.stringify(ax, null, 2));
    await writeFile(consolePath, JSON.stringify(logs, null, 2));

    return {
      scenario: `real:${view}`,
      view,
      backend,
      url,
      targetId,
      title,
      appRect,
      screenshotPath,
      textPath,
      axPath,
      consolePath,
      textPreview: summarizeText(text || ""),
    };
  } finally {
    client.close();
    await closeTarget(targetId);
  }
}

export function renderReport(results) {
  const lines = ["# margins UX CDP run", "", `Generated: ${new Date().toISOString()}`, ""];
  for (const r of results) {
    lines.push(
      `## ${r.scenario}`,
      "",
      `- URL: ${r.url}`,
      `- Screenshot: ${r.screenshotPath}`,
      `- Text: ${r.textPath}`,
      `- AX tree: ${r.axPath}`,
      "",
      "Visible text preview:",
      "",
      "```",
      r.textPreview,
      "```",
      "",
    );
  }
  return lines.join("\n");
}

async function prepareScenarioForCapture(client, scenario) {
  if (scenario === "recording-device-switching-slow") {
    await delay(3_100);
    return;
  }
  if (scenario === "recording-device-fallback-settings") {
    await client.send("Runtime.evaluate", {
      expression: "window.__chooseCaptureDevice && window.__chooseCaptureDevice()",
      awaitPromise: true,
    }).catch(() => undefined);
    await delay(300);
    return;
  }
  if (scenario === "recording-device-switch-failed") {
    // Exercise the real three-second Bluetooth/candidate confirmation window
    // before capturing the neutral failure notice.
    await delay(3_500);
    return;
  }
  if (scenario !== "distill-error-pi-login") return;

  await client.send("Runtime.evaluate", {
    expression: "window.__processSession && window.__processSession('customer-call')",
    awaitPromise: true,
  }).catch(() => undefined);

  for (let i = 0; i < 20; i++) {
    const hasError = await evalValue(client, "Boolean(document.querySelector('.processing-error'))").catch(() => false);
    if (hasError) return;
    await delay(100);
  }
}

async function prepareRealView(client, view) {
  if (view === "home") return;
  if (view === "settings" || view === "settings-audio" || view === "settings-ai") {
    await waitForRealView(client, view);
    return;
  }
  const script = realViewScript(view);
  if (!script) return;
  await client.send("Runtime.evaluate", {
    expression: script,
    awaitPromise: true,
  }).catch(() => undefined);
  await waitForRealView(client, view);
  await delay(300);
}

function realViewScript(view) {
  if (view === "settings" || view === "settings-audio" || view === "settings-ai") {
    const section = view === "settings-ai" ? "ai" : "audio";
    return `(async () => {
      for (let i = 0; i < 60 && !window.__nav; i++) await new Promise(r => setTimeout(r, 50));
      if (window.__nav) await window.__nav('settings');
      if (window.__settingsNav) window.__settingsNav(${JSON.stringify(section)});
    })()`;
  }
  return null;
}

async function waitForRealView(client, view, { timeoutMs = 5000, intervalMs = 100 } = {}) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const ready = await evalValue(client, realViewReadyExpression(view)).catch(() => false);
    if (ready) return;
    await delay(intervalMs);
  }
}

function realViewReadyExpression(view) {
  if (view === "settings" || view === "settings-audio") {
    return "Boolean(document.querySelector('.settings-overlay') && document.body.innerText.includes('Audio'))";
  }
  if (view === "settings-ai") {
    return "Boolean(document.querySelector('.settings-overlay') && document.body.innerText.includes('Note-making AI'))";
  }
  return "true";
}

async function evalValue(client, expression) {
  const res = await client.send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (res.exceptionDetails) throw new Error(res.exceptionDetails.text || "Runtime.evaluate failed");
  return res.result?.value;
}

function summarizeText(text) {
  return text.split("\n").map(s => s.trim()).filter(Boolean).slice(0, 18).join("\n");
}

async function waitForMockReady(client, { timeoutMs = 6000, intervalMs = 100 } = {}) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const ready = await evalValue(
      client,
      "Boolean(window.__marginsMock && window.__marginsMock.getState && window.__marginsMock.getState())",
    ).catch(() => false);
    if (ready) return;
    await delay(intervalMs);
  }
  // Fall through after the timeout: a scenario may legitimately never create
  // mock state, and the caller still gets a best-effort screenshot.
}

async function waitForAppReady(client, { timeoutMs = 8000, intervalMs = 100 } = {}) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const ready = await evalValue(
      client,
      "Boolean(window.__marginsAppReady && document.querySelector('#app')?.innerText?.trim())",
    ).catch(() => false);
    if (ready) return;
    await delay(intervalMs);
  }
}

function waitForLoad(client) {
  return new Promise(resolve => {
    const timeout = setTimeout(resolve, 3000);
    client.on("Page.loadEventFired", () => {
      clearTimeout(timeout);
      resolve();
    });
  });
}
