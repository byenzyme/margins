import test from "node:test";
import assert from "node:assert/strict";
import { captureSignalReadiness } from "../src/lib/capture-signal-state.ts";

const base = {
  is_recording: true,
  paused: false,
  capture_device: { state: "active" as const, device_name: "MacBook Pro Microphone" },
  mic_audio_frame_count: 0,
  system_audio_expected: true,
  system_audio_frame_count: 0,
  tap_status: "connecting",
};

test("startup never claims readiness before native frames arrive", () => {
  assert.deepEqual(captureSignalReadiness({ ...base, is_recording: false }, { phase: "starting" }), {
    mic: { state: "starting", label: "Starting" },
    system: { state: "starting", label: "Starting" },
  });
});

test("each lane becomes live only after its callback delivers frames", () => {
  assert.deepEqual(captureSignalReadiness({
    ...base,
    mic_audio_frame_count: 960,
    system_audio_frame_count: 960,
    tap_status: "ok",
  }, null), {
    mic: { state: "ready", label: "Live" },
    system: { state: "ready", label: "Live" },
  });
});

test("a blocked computer route stays actionable even with historical frames", () => {
  const result = captureSignalReadiness({
    ...base,
    mic_audio_frame_count: 960,
    system_audio_frame_count: 48_000,
    tap_status: "blocked",
  }, null);
  assert.equal(result.mic.state, "ready");
  assert.deepEqual(result.system, { state: "issue", label: "Interrupted" });
});

test("quiet computer audio stays calm after historical frames", () => {
  const result = captureSignalReadiness({
    ...base,
    mic_audio_frame_count: 960,
    system_audio_frame_count: 48_000,
    tap_status: "silent",
  }, null);
  assert.equal(result.mic.state, "ready");
  assert.deepEqual(result.system, { state: "quiet", label: "Quiet" });
});

test("a reloaded hosted capture renders interrupted instead of waiting", () => {
  assert.deepEqual(captureSignalReadiness({
    ...base,
    paused: true,
    capture_phase: "interrupted",
    web_durable_audio_status: "interrupted",
  }, null), {
    mic: { state: "issue", label: "Interrupted" },
    system: { state: "off", label: "Not used" },
  });
});

test("healthy durable browser transport is Live when amplitude metering is unavailable", () => {
  const result = captureSignalReadiness({
    ...base,
    mic_audio_frame_count: 0,
    web_durable_audio_status: "healthy",
  }, null);
  assert.deepEqual(result.mic, { state: "ready", label: "Live" });
});
