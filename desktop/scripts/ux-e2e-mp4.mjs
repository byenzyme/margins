import { existsSync } from "node:fs";
import { readdir, stat, writeFile } from "node:fs/promises";
import { basename, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const runId = process.env.MARGINS_UX_E2E_RUN_ID || "latest";
const runDir = resolve(process.cwd(), "ux-e2e-runs", runId);
const input = process.argv[2] ? resolve(process.argv[2]) : await findVideo(runDir);
const output = process.argv[3] ? resolve(process.argv[3]) : resolve(runDir, "journey.mp4");

if (!input) {
  console.error(`No WebM video found under ${runDir}`);
  process.exit(1);
}

const ffmpeg = findFfmpeg();
if (!ffmpeg) {
  console.error("ffmpeg not found. Install it or set FFMPEG_BIN.");
  process.exit(1);
}

const args = [
  "-y",
  "-i", input,
  "-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2",
  "-c:v", "libx264",
  "-pix_fmt", "yuv420p",
  "-movflags", "+faststart",
  "-crf", "23",
  output,
];
const result = spawnSync(ffmpeg, args, { stdio: "inherit" });
if (result.status !== 0) process.exit(result.status || 1);

await writeFile(resolve(runDir, "video-conversion.json"), `${JSON.stringify({
  input,
  output,
  command: [ffmpeg, ...args].join(" "),
}, null, 2)}\n`, "utf8");
console.log(`Wrote ${output}`);

function findFfmpeg() {
  const candidates = [
    process.env.FFMPEG_BIN,
    "/opt/homebrew/bin/ffmpeg",
    "/usr/local/bin/ffmpeg",
    "/usr/bin/ffmpeg",
    "ffmpeg",
  ].filter(Boolean);
  for (const candidate of candidates) {
    if (candidate === "ffmpeg") return candidate;
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

async function findVideo(dir) {
  const stable = resolve(dir, "journey.webm");
  if (existsSync(stable)) return stable;
  const matches = await walk(dir, file => file.endsWith(".webm"));
  matches.sort((a, b) => {
    if (basename(a) === "journey.webm") return -1;
    if (basename(b) === "journey.webm") return 1;
    return a.localeCompare(b);
  });
  return matches[0] || null;
}

async function walk(dir, predicate) {
  if (!existsSync(dir)) return [];
  const out = [];
  for (const entry of await readdir(dir)) {
    const path = resolve(dir, entry);
    const info = await stat(path);
    if (info.isDirectory()) out.push(...await walk(path, predicate));
    else if (predicate(path)) out.push(path);
  }
  return out;
}
