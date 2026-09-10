import { writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { cdpJson, closeTarget, ensureChrome } from "./chrome.mjs";
import { CdpClient } from "./cdp-client.mjs";
import { delay } from "./timing.mjs";

await ensureChrome();
await delay(800);

const base = process.env.ASIDE_UX_BASE || "http://localhost:5173";
const scenario = process.argv[2] || "home-many-sessions";
const out = resolve(process.cwd(), "ux-shots", `_clip-${scenario}.png`);
const url = `${base}/?scenario=${encodeURIComponent(scenario)}`;

let target;
try {
  try { target = await cdpJson(`/json/new?${encodeURIComponent(url)}`, { method: "PUT" }); }
  catch { target = await cdpJson(`/json/new?${encodeURIComponent(url)}`); }
  const client = await CdpClient.connect(target.webSocketDebuggerUrl);
  await client.send("Page.enable");
  await client.send("Runtime.enable");
  await client.send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 860, deviceScaleFactor: 3, mobile: false });
  await delay(1400);
  const rect = await client.send("Runtime.evaluate", {
    expression: `(() => { const r = document.querySelector('.sidebar').getBoundingClientRect(); return JSON.stringify({x:r.x,y:r.y,width:r.width,height:r.height}); })()`,
    returnByValue: true,
  });
  const r = JSON.parse(rect.result.value);
  const shot = await client.send("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: false,
    fromSurface: true,
    clip: { x: r.x, y: r.y, width: r.width, height: Math.min(r.height, 820), scale: 1 },
  });
  await writeFile(out, Buffer.from(shot.data, "base64"));
  console.log("wrote", out);
  await client.close();
} finally {
  if (target?.id) closeTarget(target.id).catch(() => {});
}
