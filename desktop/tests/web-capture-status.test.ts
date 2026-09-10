import assert from "node:assert/strict";
import test from "node:test";

import type { RecordingStatus } from "../src/lib/tauri.ts";
import { decorateHostedRecordingStatus } from "../src/lib/web-capture-status.ts";

const now = 2_000_000;
const base: RecordingStatus = {
  is_recording: true,
  paused: false,
  session_name: "meeting-1",
  web_recording_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  elapsed_secs: 20,
  input_device_name: null,
  capture_device: { state: "active", device_name: "Web capture" },
  mic_level: null,
  mic_audio_frame_count: 0,
  spk_level: 0,
  mic_drop_count: 0,
  spk_drop_count: 0,
  mic_gap_ms_total: 0,
  mic_switch_count: 0,
  timeline_reusable: true,
  speaker_silence_secs: 0,
  system_audio_expected: false,
  system_audio_frame_count: 0,
  system_audio_observed: false,
  system_audio_seen: false,
  tap_status: "not_expected",
  tap_warning: null,
  live_transcription_mode: "mic_diarized",
  capture_phase: "recording",
  webm_chunk_count: 4,
  webm_bytes: 64_000,
  webm_last_received_unix_ms: now - 2_000,
  live_pcm_batch_count: 30,
  live_pcm_sample_count: 240_000,
  live_pcm_last_received_unix_ms: now - 500,
  live_pcm_configured: true,
  web_transport_server_unix_ms: now,
  web_owner_lease_active: true,
  web_owner_last_heartbeat_unix_ms: now - 500,
  web_owner_lease_timeout_ms: 8_000,
};

const local = {
  recordingId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  sessionName: "meeting-1",
  micLevel: 0.42,
  micAudioFrameCount: 48_000,
  durableUploadError: null,
  livePcmError: null,
  healthGraceStartedAtMs: now - 20_000,
};

test("healthy durable chunks and live PCM remain independently observable", () => {
  const status = decorateHostedRecordingStatus(base, local, now);
  assert.equal(status.web_capture_owner, "local");
  assert.equal(status.web_durable_audio_status, "healthy");
  assert.equal(status.web_live_pcm_status, "healthy");
  assert.equal(status.mic_level, 0.42);
  assert.equal(status.web_capture_warning, null);
  assert.equal(status.web_live_pcm_warning, null);
});

test("a non-owner tab with fresh transport renders capture in another tab", () => {
  const status = decorateHostedRecordingStatus(base, null, now);
  assert.equal(status.capture_phase, "capturing_elsewhere");
  assert.equal(status.web_capture_owner, "remote");
  assert.equal(status.web_durable_audio_status, "healthy");
  assert.equal(status.mic_level, null, "server must not claim browser amplitude");
  assert.match(status.web_capture_warning || "", /another browser tab/);
});

test("reload owner loss becomes interrupted only after lease and transport expire", () => {
  const status = decorateHostedRecordingStatus({
    ...base,
    web_owner_lease_active: false,
    webm_last_received_unix_ms: now - 10_001,
    live_pcm_last_received_unix_ms: now - 5_001,
  }, null, now);
  assert.equal(status.capture_phase, "interrupted");
  assert.equal(status.web_capture_owner, "absent");
  assert.equal(status.web_durable_audio_status, "interrupted");
  assert.match(status.web_capture_warning || "", /Take control/);
});

test("healthy durable WebM is not conflated with stale live PCM", () => {
  const status = decorateHostedRecordingStatus({
    ...base,
    webm_last_received_unix_ms: now - 1_000,
    live_pcm_last_received_unix_ms: now - 5_001,
  }, local, now);
  assert.equal(status.web_durable_audio_status, "healthy");
  assert.equal(status.web_live_pcm_status, "stale");
  assert.match(status.web_live_pcm_warning || "", /Durable audio is still recording/);
  assert.equal(status.capture_phase, "recording");
});

test("a local owner for another recording ID cannot mask same-name remote ownership", () => {
  const status = decorateHostedRecordingStatus(base, {
    ...local,
    recordingId: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    micLevel: 0.9,
  }, now);
  assert.equal(status.web_capture_owner, "remote");
  assert.equal(status.capture_phase, "capturing_elsewhere");
  assert.equal(status.mic_level, null);
});

test("server-computed monotonic ages take precedence over wall timestamps", () => {
  const status = decorateHostedRecordingStatus({
    ...base,
    webm_last_received_age_ms: 1_000,
    live_pcm_last_received_age_ms: 500,
    webm_last_received_unix_ms: 1,
    live_pcm_last_received_unix_ms: 1,
  }, local, now);
  assert.equal(status.web_durable_audio_status, "healthy");
  assert.equal(status.web_live_pcm_status, "healthy");
});

test("local PCM bridge errors are actionable without failing durable capture", () => {
  const status = decorateHostedRecordingStatus(base, {
    ...local,
    livePcmError: "AudioContext remained suspended",
  }, now);
  assert.equal(status.web_durable_audio_status, "healthy");
  assert.equal(status.web_live_pcm_status, "failed");
  assert.equal(status.capture_phase, "recording");
  assert.match(status.web_live_pcm_warning || "", /AudioContext remained suspended/);
});

test("durable WebM rejection is surfaced as a capture failure even when HTTP transport succeeded", () => {
  const status = decorateHostedRecordingStatus(base, {
    ...local,
    durableUploadError: "Capture authority belongs to another browser operation",
  }, now);
  assert.equal(status.web_durable_audio_status, "failed");
  assert.equal(status.capture_phase, "recording");
  assert.match(status.web_capture_warning || "", /Audio upload failed.*another browser operation/);
});

test("transport freshness uses the server clock rather than browser clock skew", () => {
  const status = decorateHostedRecordingStatus(base, local, now + 86_400_000);
  assert.equal(status.web_durable_audio_status, "healthy");
  assert.equal(status.web_live_pcm_status, "healthy");
});

test("legacy telemetry remains unknown instead of becoming missing or stale", () => {
  const legacy: RecordingStatus = { ...base };
  delete legacy.webm_chunk_count;
  delete legacy.webm_bytes;
  delete legacy.webm_last_received_unix_ms;
  delete legacy.live_pcm_batch_count;
  delete legacy.live_pcm_sample_count;
  delete legacy.live_pcm_last_received_unix_ms;
  delete legacy.live_pcm_configured;
  delete legacy.web_owner_lease_active;
  const localStatus = decorateHostedRecordingStatus(legacy, local, now);
  assert.equal(localStatus.web_durable_audio_status, "unknown");
  assert.equal(localStatus.web_live_pcm_status, "unknown");
  const observerStatus = decorateHostedRecordingStatus(legacy, null, now);
  assert.equal(observerStatus.capture_phase, "recording_unknown");
  assert.equal(observerStatus.web_capture_owner, "unknown");
});

test("resume grace suppresses stale flashes without using backend allocation time", () => {
  const resumed = decorateHostedRecordingStatus({
    ...base,
    elapsed_secs: 600,
    webm_last_received_unix_ms: now - 60_000,
    live_pcm_last_received_unix_ms: now - 60_000,
  }, { ...local, healthGraceStartedAtMs: now - 100 }, now);
  assert.equal(resumed.web_durable_audio_status, "healthy");
  assert.equal(resumed.web_live_pcm_status, "healthy");

  const slowSetup = decorateHostedRecordingStatus({
    ...base,
    elapsed_secs: 600,
    webm_chunk_count: 0,
    webm_bytes: 0,
    webm_last_received_unix_ms: null,
  }, { ...local, healthGraceStartedAtMs: now - 100 }, now);
  assert.equal(slowSetup.web_durable_audio_status, "starting");
});

test("explicit recovery ownership is scoped and honest about empty partial audio", () => {
  const recovered = decorateHostedRecordingStatus({
    ...base,
    webm_chunk_count: 0,
    webm_bytes: 0,
    webm_last_received_unix_ms: null,
  }, {
    ...local,
    authority: "recovery",
    micLevel: null,
    micAudioFrameCount: 0,
  }, now);
  assert.equal(recovered.web_capture_owner, "recovery");
  assert.equal(recovered.capture_phase, "interrupted_recovery_empty");
  assert.equal(recovered.mic_level, null);
});
