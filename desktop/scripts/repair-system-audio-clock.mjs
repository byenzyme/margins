#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

function readArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (!arg.startsWith("--")) throw new Error(`Unexpected argument: ${arg}`);
    const value = argv[i + 1];
    if (!value || value.startsWith("--")) throw new Error(`Missing value for ${arg}`);
    args[arg.slice(2)] = value;
    i += 1;
  }
  return args;
}

function parseTimestamp(value) {
  const parts = value.split(":").map(Number);
  if (parts.some(part => !Number.isFinite(part))) return null;
  if (parts.length === 2) return (parts[0] * 60 + parts[1]) * 1_000;
  if (parts.length === 3) return (parts[0] * 3_600 + parts[1] * 60 + parts[2]) * 1_000;
  return null;
}

function formatTimestamp(milliseconds) {
  const totalSeconds = Math.max(0, Math.round(milliseconds / 1_000));
  const hours = Math.floor(totalSeconds / 3_600);
  const minutes = Math.floor((totalSeconds % 3_600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) {
    return `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
  }
  return `${String(Math.floor(totalSeconds / 60)).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

function retime(milliseconds, anchorMs, ratio) {
  if (milliseconds < anchorMs) return milliseconds * ratio;
  return anchorMs + (milliseconds - anchorMs) * ratio;
}

function main() {
  const args = readArgs(process.argv.slice(2));
  const transcriptPath = path.resolve(args.transcript || "");
  if (!args.transcript) throw new Error("--transcript is required");
  const fromRate = Number(args["from-rate"]);
  const toRate = Number(args["to-rate"]);
  const anchorMs = Number(args["anchor-ms"] || 0);
  if (!(fromRate > 0) || !(toRate > 0) || !(anchorMs >= 0)) {
    throw new Error("--from-rate, --to-rate, and --anchor-ms must be valid numbers");
  }
  const ratio = fromRate / toRate;
  const original = fs.readFileSync(transcriptPath, "utf8");
  if (/Transcript source: `offline_retimed`/.test(original)) {
    throw new Error("Transcript has already been retimed");
  }

  const timelineMarker = "\n## Timeline\n\n";
  const timelineStart = original.indexOf(timelineMarker);
  if (timelineStart < 0) throw new Error("Transcript has no ## Timeline section");
  const bodyStart = timelineStart + timelineMarker.length;
  const nextSection = original.indexOf("\n## ", bodyStart);
  const bodyEnd = nextSection < 0 ? original.length : nextSection;
  const header = original.slice(0, timelineStart);
  const suffix = original.slice(bodyEnd);
  const lines = original.slice(bodyStart, bodyEnd).split("\n");

  let repaired = 0;
  let firstSystemBefore = null;
  let firstSystemAfter = null;
  let lastSystemBefore = null;
  let lastSystemAfter = null;
  const timeline = lines.map((line, index) => {
    const match = line.match(/^\[((?:\d{2}:)?\d{2}:\d{2})\]\s+(.*)$/);
    if (!match) return { line, index, milliseconds: Number.MAX_SAFE_INTEGER };
    const milliseconds = parseTimestamp(match[1]);
    if (milliseconds === null) return { line, index, milliseconds: Number.MAX_SAFE_INTEGER };
    if (!/^them \(system\):/.test(match[2])) return { line, index, milliseconds };

    const corrected = retime(milliseconds, anchorMs, ratio);
    firstSystemBefore ??= milliseconds;
    firstSystemAfter ??= corrected;
    lastSystemBefore = milliseconds;
    lastSystemAfter = corrected;
    repaired += 1;
    return {
      line: `[${formatTimestamp(corrected)}] ${match[2]}`,
      index,
      milliseconds: corrected,
    };
  });

  timeline.sort((a, b) => a.milliseconds - b.milliseconds || a.index - b.index);
  const repairedHeader = header
    .replace(
      /^Source:.*$/m,
      `Source: Margins Desktop offline both-channel transcript and memo context; system-audio timestamps repaired from ${fromRate} Hz labeling to the ${toRate} Hz output clock.`,
    )
    .replace(
      /^Transcript source:.*$/m,
      `Transcript source: \`offline_retimed\` (recognized text unchanged; computer-audio timestamps scaled by ${ratio.toFixed(6)} around segment anchor ${anchorMs} ms)`,
    );
  const output = `${repairedHeader}${timelineMarker}${timeline.map(item => item.line).join("\n")}${suffix}`;
  const backupPath = path.resolve(args.backup || `${transcriptPath}.pre-clock-repair`);
  fs.copyFileSync(transcriptPath, backupPath, fs.constants.COPYFILE_EXCL);
  fs.writeFileSync(transcriptPath, output);

  process.stdout.write(`${JSON.stringify({
    transcript: transcriptPath,
    backup: backupPath,
    repaired_system_lines: repaired,
    ratio,
    anchor_ms: anchorMs,
    first_system_before_ms: firstSystemBefore,
    first_system_after_ms: firstSystemAfter,
    last_system_before_ms: lastSystemBefore,
    last_system_after_ms: lastSystemAfter,
  }, null, 2)}\n`);
}

try {
  main();
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
}
