#!/usr/bin/env node
// Margins UserPromptSubmit hook for Claude Code and Codex CLI.
//
// On every user turn it asks the local Margins server whether a meeting is
// being captured and how far the durable live transcript has advanced, then
// injects a short context note so the agent knows a previously fetched
// transcript is stale. Both agents speak the same hook contract: JSON on
// stdin, `hookSpecificOutput.additionalContext` on stdout.
//
// It must never get in the user's way: any failure (server down, no token,
// timeout) exits 0 with no output.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { homedir } from "node:os";

const HOST = process.env.MARGINS_HOST || "127.0.0.1";
const PORT = process.env.MARGINS_PORT || "8787";
const DATA_DIR = process.env.MARGINS_DATA_DIR || join(homedir(), ".margins-app");
const TIMEOUT_MS = parseInt(process.env.MARGINS_HOOK_TIMEOUT_MS || "1500", 10);

// Per-install state lives next to the script so it follows the project the
// hook was installed into, regardless of the cwd agents run hooks with.
const stateDir = dirname(fileURLToPath(import.meta.url));
const statePath = join(stateDir, "state.json");

function readToken() {
  if (process.env.MARGINS_TOKEN) return process.env.MARGINS_TOKEN;
  try {
    return readFileSync(join(DATA_DIR, "token"), "utf-8").trim();
  } catch {
    return null;
  }
}

function readState() {
  try {
    return JSON.parse(readFileSync(statePath, "utf-8"));
  } catch {
    return null;
  }
}

function writeState(state) {
  try {
    mkdirSync(stateDir, { recursive: true });
    writeFileSync(statePath, JSON.stringify(state));
  } catch {
    // Best effort; the hook still works without cross-turn deltas.
  }
}

function fmtClock(ms) {
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = String(m).padStart(2, "0");
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

function fmtDuration(secs) {
  if (secs < 90) return `${Math.max(1, Math.round(secs))}s`;
  return `${Math.round(secs / 60)}m`;
}

async function fetchRecordingStatus(token) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
  try {
    const res = await fetch(`http://${HOST}:${PORT}/api/invoke/get_recording_status`, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: "{}",
      signal: controller.signal,
    });
    if (!res.ok) return null;
    const payload = await res.json();
    return payload && payload.ok ? payload.result : null;
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

function emit(context) {
  process.stdout.write(
    JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "UserPromptSubmit",
        additionalContext: context,
      },
    }),
  );
}

async function main() {
  // Drain stdin so the agent process never blocks on a full pipe; the payload
  // itself is not needed (all state comes from the Margins server).
  try {
    if (!process.stdin.isTTY) {
      await new Promise((resolve) => {
        process.stdin.on("data", () => {});
        process.stdin.on("end", resolve);
        process.stdin.on("error", resolve);
      });
    }
  } catch {
    // ignore
  }

  const token = readToken();
  if (!token) return;

  const status = await fetchRecordingStatus(token);
  if (!status) return;

  const previous = readState();
  const watermark = status.transcript_watermark || null;

  if (!status.is_recording) {
    if (previous && previous.session_name) {
      // Report the transition to idle exactly once, then go quiet.
      writeState({});
      emit(
        `Margins: the meeting "${previous.session_name}" that was being captured earlier has finished recording. ` +
          `If you fetched its live transcript during the meeting, it is a snapshot — the finalized transcript may supersede it once processing completes.`,
      );
    }
    return;
  }

  const name = status.session_name || "unnamed session";
  const lines = [];
  lines.push(
    `Margins: a meeting is being captured right now — session "${name}", ` +
      `${fmtDuration(status.elapsed_secs)} elapsed${status.paused ? ", currently paused" : ""}.`,
  );

  if (watermark && watermark.committed_until_ms > 0) {
    lines.push(`The durable live transcript is committed through ${fmtClock(watermark.committed_until_ms)}.`);
    const sameSession = previous && previous.session_name === name;
    if (sameSession && typeof previous.committed_until_ms === "number") {
      const deltaMs = watermark.committed_until_ms - previous.committed_until_ms;
      if (deltaMs > 0) {
        lines.push(
          `It advanced ~${fmtDuration(deltaMs / 1000)} since your previous turn — ` +
            `any transcript you fetched earlier is stale; refetch it before quoting or summarizing.`,
        );
      } else {
        lines.push(`No new transcript has been committed since your previous turn.`);
      }
    } else {
      lines.push(`If you already fetched this transcript, refetch it before quoting or summarizing — it keeps growing while the meeting runs.`);
    }
  } else {
    lines.push(`No live transcript checkpoint has been committed yet.`);
  }

  writeState({
    session_name: name,
    committed_until_ms: watermark ? watermark.committed_until_ms : 0,
    updated_unix_ms: watermark ? watermark.updated_unix_ms : 0,
  });

  emit(lines.join(" "));
}

main().then(
  () => process.exit(0),
  () => process.exit(0),
);
