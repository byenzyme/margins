import assert from "node:assert/strict";
import test from "node:test";

import { systemAudioReadinessAfterRecordingStatus } from "../src/lib/audio-readiness.ts";

test("clears persisted readiness when expected live audio delivers no frames", () => {
  assert.equal(systemAudioReadinessAfterRecordingStatus(true, {
    is_recording: true,
    system_audio_expected: true,
    system_audio_observed: false,
    tap_status: "blocked",
  }), false);
});

test("keeps readiness while delivery is connecting or system audio is not expected", () => {
  assert.equal(systemAudioReadinessAfterRecordingStatus(true, {
    is_recording: true,
    system_audio_expected: true,
    system_audio_observed: false,
    tap_status: "connecting",
  }), true);
  assert.equal(systemAudioReadinessAfterRecordingStatus(true, {
    is_recording: true,
    system_audio_expected: false,
    system_audio_observed: false,
    tap_status: "not_expected",
  }), true);
});

test("never promotes a persisted false value", () => {
  assert.equal(systemAudioReadinessAfterRecordingStatus(false, {
    is_recording: true,
    system_audio_expected: true,
    system_audio_observed: true,
    tap_status: "ok",
  }), false);
});
