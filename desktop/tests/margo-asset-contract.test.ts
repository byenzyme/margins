import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { basename, join } from "node:path";
import test from "node:test";

const desktopDir = fileURLToPath(new URL("..", import.meta.url));
const margoDir = join(desktopDir, "public/margo");
const srcDir = join(desktopDir, "src");

const ALLOWED_MARGO_FILES = new Set([
  "README.md",
  "mark-paper.png",
  "mark.png",
  "welcome.png",
]);

const DEPRECATED_POSE_PATTERNS = [
  "margo-base.png",
  "margo-recording.png",
  "margo-weaving.png",
  "margo-note.png",
  "margo-recoverable.png",
  "/margo/margo-",
];

const APPROVED_HASHES: Record<string, string> = {
  "mark-paper.png": "a42d9d76417d6dd6ca33403894b4ff1ee47afaf86df57aa9ab3492dc276efc67",
  "mark.png": "20881636a021c2c207ff44215ad3c2079bdde5f365e63983f43bc53da74f703c",
  "welcome.png": "e9bb600b2aa87b85e5c1ae61345f2bd8df1cc25e1deee93dc56d46591c4a6aab",
};

function walkSourceFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walkSourceFiles(full, acc);
    else if (/\.(ts|css|html)$/.test(entry.name)) acc.push(full);
  }
  return acc;
}

function sourceFiles(): string[] {
  return [
    ...walkSourceFiles(srcDir),
    ...readdirSync(desktopDir)
      .filter(name => name.endsWith(".html"))
      .map(name => join(desktopDir, name)),
  ];
}

test("desktop/public/margo contains exactly the approved asset roles", () => {
  assert.deepEqual(new Set(readdirSync(margoDir)), ALLOWED_MARGO_FILES);
});

test("approved Margo PNG bytes, dimensions, and alpha contract stay stable", () => {
  for (const [name, expectedHash] of Object.entries(APPROVED_HASHES)) {
    const bytes = readFileSync(join(margoDir, name));
    assert.equal(createHash("sha256").update(bytes).digest("hex"), expectedHash, name);
    assert.equal(bytes.subarray(1, 4).toString("ascii"), "PNG", `${name} must be PNG`);
    if (name.startsWith("mark")) {
      assert.equal(bytes.readUInt32BE(16), 1254, `${name} width`);
      assert.equal(bytes.readUInt32BE(20), 1254, `${name} height`);
      assert.equal(bytes[25], 6, `${name} must retain RGBA color type`);
    }
  }
});

test("source has no references to deprecated Margo pose assets", () => {
  const offenders: string[] = [];
  for (const file of sourceFiles()) {
    const text = readFileSync(file, "utf8");
    for (const pattern of DEPRECATED_POSE_PATTERNS) {
      if (text.includes(pattern)) offenders.push(`${file}: ${pattern}`);
    }
  }
  assert.deepEqual(offenders, []);
});

test("welcome stays in the main UI while both mark layers stay circle-only", () => {
  const refs = new Map<string, string[]>();
  for (const asset of ["welcome.png", "mark.png", "mark-paper.png"]) refs.set(asset, []);
  for (const file of sourceFiles()) {
    const text = readFileSync(file, "utf8");
    for (const asset of refs.keys()) {
      if (text.includes(`/margo/${asset}`)) refs.get(asset)!.push(file);
    }
  }

  assert.ok(refs.get("welcome.png")!.some(file => basename(file) === "sidebar.ts"));
  for (const asset of ["mark.png", "mark-paper.png"]) {
    assert.deepEqual(
      refs.get(asset)!.map(file => basename(file)),
      ["circle.ts"],
      `${asset} must stay inside the capture-circle renderer`,
    );
  }
});

test("circle polish keeps drag, cursor, motion, and borderless-surface contracts explicit", () => {
  const renderer = readFileSync(join(srcDir, "circle.ts"), "utf8");
  const styles = readFileSync(join(srcDir, "circle.css"), "utf8");

  assert.match(renderer, /windowToDrag\.startDragging\(\)/);
  assert.doesNotMatch(renderer, /data-tauri-drag-region/);
  assert.match(styles, /#circle-root\.dragging \.grip,\s*\.grip:active\s*{[^}]*cursor:\s*grabbing/s);
  for (const selector of [".mark", ".more", ".controls button"]) {
    const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    assert.match(styles, new RegExp(`${escaped}\\s*\\{[^}]*cursor:\\s*pointer`, "s"), selector);
  }

  assert.match(styles, /\.mark:hover \.margo-art/);
  assert.match(styles, /\.mark:active \.margo-art/);
  assert.doesNotMatch(styles, /\.mark:active\s*{[^}]*transform:/s);
  assert.match(styles, /#circle-root\.expanded \.mark-row/);
  assert.match(styles, /@media \(prefers-reduced-motion: reduce\)/);
  assert.doesNotMatch(styles, /backdrop-filter|border:\s*1px solid rgba\([^)]*255/);
});
