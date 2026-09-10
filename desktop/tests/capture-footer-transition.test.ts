import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { captureFooterPosture } from "../src/lib/capture-footer-state.ts";

test("pausing exposes no premature Resume or Finish action", () => {
  assert.equal(captureFooterPosture("pausing", true), "pausing");
});

test("durably paused capture exposes Resume and Finish", () => {
  assert.equal(captureFooterPosture("paused", true), "paused");
});

test("running capture retains the Pause action posture", () => {
  assert.equal(captureFooterPosture("recording", false), "recording");
});

test("non-owner hosted states expose no global Finish while recovery is explicit", () => {
  const source = readFileSync(new URL("../src/render/recording.ts", import.meta.url), "utf8");
  assert.match(source, /capturePhase === "interrupted"[\s\S]*__takeOverInterruptedRecording/);
  assert.match(source, /capturePhase === "capturing_elsewhere"[\s\S]*controls stay with that tab/);
  assert.match(source, /capturePhase === "interrupted_recovery"[\s\S]*__finishRecording/);
  assert.match(source, /\["interrupted", "capturing_elsewhere", "recording_unknown"\][\s\S]*return ""/);
});

test("capture surfaces share one footer renderer and only permission-wait startup is cancellable", () => {
  const recordingSource = readFileSync(new URL("../src/render/recording.ts", import.meta.url), "utf8");
  const workspaceSource = readFileSync(new URL("../src/render/session-workspace.ts", import.meta.url), "utf8");
  assert.match(recordingSource, /export function renderCaptureFooterAction/);
  assert.match(recordingSource, /__pauseRecording/);
  assert.match(recordingSource, /__resumeRecording/);
  assert.match(recordingSource, /__finishRecording/);
  assert.doesNotMatch(recordingSource, /Stop capture|__stopRecording/);
  assert.match(recordingSource, /waitingForPermission[\s\S]*__cancelRecordingStartup/);
  assert.match(workspaceSource, /renderCaptureFooterAction/);
  assert.doesNotMatch(workspaceSource, /function renderLiveFooterAction/);
});

test("prepare, starting, and recording reserve one stable footer posture", () => {
  const recordingSource = readFileSync(new URL("../src/render/recording.ts", import.meta.url), "utf8");
  const workspaceSource = readFileSync(new URL("../src/render/session-workspace.ts", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
  assert.match(recordingSource, /capture-primary-action[^>]*disabled/);
  assert.match(recordingSource, /<span>Starting…<\/span>/);
  assert.doesNotMatch(recordingSource, /Preparing microphone|You can type marks now/);
  assert.match(workspaceSource, /capture-footer-reserved/);
  assert.match(styles, /\.recording-footer\s*\{[\s\S]*min-height:\s*62px/);
  assert.match(styles, /\.capture-primary-action\s*\{[\s\S]*min-width:\s*104px/);
  assert.match(styles, /\.recording-workspace \.session-title-input,[\s\S]*height:\s*42px/);
});

test("leaving prepare disarms its hidden auto-start state", () => {
  const mainSource = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const navigationSource = readFileSync(new URL("../src/actions/navigation.ts", import.meta.url), "utf8");
  assert.match(mainSource, /function abandonPreStartPrep\(\)/);
  assert.match(mainSource, /clearAutoStartTimer\(\);[\s\S]*armedFromCalendar = false;[\s\S]*autoStartDeadline = null;/);
  assert.match(mainSource, /if \(preStartPrepOpen\) abandonPreStartPrep\(\);/);
  assert.match(navigationSource, /ctx\.exitTransientPrep\(\)/);
});

test("prepare start exposes blocked readiness instead of silently returning", () => {
  const mainSource = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const recordingSource = readFileSync(new URL("../src/render/recording.ts", import.meta.url), "utf8");
  assert.match(mainSource, /if \(!captureReadyFromState\(\)\) \{[\s\S]*prepStartBlockedReason =/);
  assert.match(recordingSource, /ctx\.prepStartBlockedReason/);
  assert.match(recordingSource, />Try again</);
});
