import type { RecordingStatus } from "./tauri";

const WEBM_START_GRACE_MS = 8_000;
const WEBM_STALE_MS = 10_000;
const PCM_START_GRACE_MS = 5_000;
const PCM_STALE_MS = 5_000;

export interface LocalWebCaptureSnapshot {
  recordingId: string;
  sessionName: string;
  micLevel: number | null;
  micAudioFrameCount: number;
  durableUploadError: string | null;
  livePcmError: string | null;
  healthGraceStartedAtMs: number;
  authority?: "capture" | "recovery";
}

function ageMs(nowMs: number, timestamp: number | null | undefined): number | null {
  return typeof timestamp === "number" && timestamp > 0
    ? Math.max(0, nowMs - timestamp)
    : null;
}

function preferredAgeMs(
  monotonicAge: number | null | undefined,
  transportNowMs: number,
  timestamp: number | null | undefined,
): number | null {
  return typeof monotonicAge === "number" && monotonicAge >= 0
    ? monotonicAge
    : ageMs(transportNowMs, timestamp);
}

/**
 * Merge server-observable transport facts with browser-local capture
 * ownership. The backend deliberately never claims a browser mic amplitude.
 */
export function decorateHostedRecordingStatus(
  status: RecordingStatus,
  local: LocalWebCaptureSnapshot | null,
  nowMs = Date.now(),
): RecordingStatus {
  if (!status.is_recording || !status.session_name) return status;

  if (!local || local.recordingId !== status.web_recording_id) {
    const transportNowMs = status.web_transport_server_unix_ms || nowMs;
    const webmAge = preferredAgeMs(status.webm_last_received_age_ms, transportNowMs, status.webm_last_received_unix_ms);
    const pcmAge = preferredAgeMs(status.live_pcm_last_received_age_ms, transportNowMs, status.live_pcm_last_received_unix_ms);
    const webmKnown = typeof status.webm_chunk_count === "number"
      && typeof status.webm_bytes === "number"
      && (status.webm_chunk_count === 0 || typeof status.webm_last_received_unix_ms === "number");
    const pcmKnown = typeof status.live_pcm_batch_count === "number"
      && typeof status.live_pcm_sample_count === "number"
      && (status.live_pcm_batch_count === 0 || typeof status.live_pcm_last_received_unix_ms === "number");
    const freshTransport = (webmKnown && (status.webm_chunk_count || 0) > 0 && webmAge != null && webmAge <= WEBM_STALE_MS)
      || (pcmKnown && (status.live_pcm_batch_count || 0) > 0 && pcmAge != null && pcmAge <= PCM_STALE_MS);
    if (status.web_owner_lease_active === true || freshTransport) {
      return {
        ...status,
        capture_phase: "capturing_elsewhere",
        mic_level: null,
        mic_audio_frame_count: 0,
        web_capture_owner: "remote",
        web_durable_audio_status: !webmKnown
          ? "unknown"
          : status.webm_chunk_count === 0
            ? "starting"
            : webmAge != null && webmAge <= WEBM_STALE_MS ? "healthy" : "stale",
        web_live_pcm_status: !pcmKnown
          ? "unknown"
          : status.live_pcm_batch_count === 0
            ? "starting"
            : pcmAge != null && pcmAge <= PCM_STALE_MS ? "healthy" : "stale",
        web_capture_warning: "This recording is capturing in another browser tab. Return to that tab to pause or finish it.",
        web_live_pcm_warning: null,
      };
    }
    if (typeof status.web_owner_lease_active !== "boolean") {
      return {
        ...status,
        capture_phase: "recording_unknown",
        mic_level: null,
        mic_audio_frame_count: 0,
        web_capture_owner: "unknown",
        web_durable_audio_status: "unknown",
        web_live_pcm_status: "unknown",
        web_capture_warning: "Capture ownership telemetry is unavailable from this server version. Use the original recording tab.",
        web_live_pcm_warning: null,
      };
    }
    return {
      ...status,
      capture_phase: "interrupted",
      mic_level: null,
      mic_audio_frame_count: 0,
      web_capture_owner: "absent",
      web_durable_audio_status: "interrupted",
      web_live_pcm_status: "interrupted",
      web_capture_warning: (status.webm_bytes || 0) > 0
        ? "The previous browser capture owner stopped responding. Take control to finish the audio already received or discard this partial capture."
        : "The previous browser capture owner stopped before any durable audio arrived. Take control to discard this empty partial capture.",
      web_live_pcm_warning: "Live transcription stopped when this browser's microphone bridge was interrupted.",
    };
  }

  if (status.paused && local.authority !== "recovery") {
    return {
      ...status,
      mic_level: local.micLevel,
      mic_audio_frame_count: local.micAudioFrameCount,
      web_capture_owner: "local",
      web_durable_audio_status: "paused",
      web_live_pcm_status: "paused",
      web_capture_warning: null,
      web_live_pcm_warning: null,
    };
  }

  const localElapsedMs = Math.max(0, nowMs - local.healthGraceStartedAtMs);
  const transportNowMs = status.web_transport_server_unix_ms || nowMs;
  const webmAge = preferredAgeMs(status.webm_last_received_age_ms, transportNowMs, status.webm_last_received_unix_ms);
  const pcmAge = preferredAgeMs(status.live_pcm_last_received_age_ms, transportNowMs, status.live_pcm_last_received_unix_ms);

  const webmKnown = typeof status.webm_chunk_count === "number"
    && typeof status.webm_bytes === "number"
    && (status.webm_chunk_count === 0 || typeof status.webm_last_received_unix_ms === "number");
  const pcmKnown = typeof status.live_pcm_batch_count === "number"
    && typeof status.live_pcm_sample_count === "number"
    && typeof status.live_pcm_configured === "boolean"
    && (status.live_pcm_batch_count === 0 || typeof status.live_pcm_last_received_unix_ms === "number");
  const durableStatus = !webmKnown
    ? "unknown"
    : local.durableUploadError
    ? "failed"
    : (status.webm_chunk_count || 0) === 0
      ? (localElapsedMs <= WEBM_START_GRACE_MS ? "starting" : "missing")
      : webmAge != null && webmAge > WEBM_STALE_MS
        ? (localElapsedMs <= WEBM_START_GRACE_MS ? "healthy" : "stale")
        : "healthy";

  const pcmStatus = !pcmKnown
    ? "unknown"
    : status.live_pcm_configured === false
    ? "unavailable"
    : local.livePcmError
      ? "failed"
      : (status.live_pcm_batch_count || 0) === 0
        ? (localElapsedMs <= PCM_START_GRACE_MS ? "starting" : "missing")
        : pcmAge != null && pcmAge > PCM_STALE_MS
          ? (localElapsedMs <= PCM_START_GRACE_MS ? "healthy" : "stale")
          : "healthy";

  const durableWarning = local.durableUploadError
    ? `Audio upload failed: ${local.durableUploadError}`
    : durableStatus === "missing"
      ? "No durable audio chunks have reached the server. Keep this tab open and check the connection."
      : durableStatus === "stale"
        ? "Durable audio uploads have stopped. Keep this tab open and check the connection."
        : null;
  const pcmWarning = local.livePcmError
    ? `Live transcription microphone bridge failed: ${local.livePcmError}. Durable audio is still recording.`
    : pcmStatus === "missing"
      ? "Live transcription is receiving no microphone PCM. Durable audio is still recording; reload is not a recovery."
      : pcmStatus === "stale"
        ? "Live transcription microphone PCM is stale. Durable audio is still recording; check this tab's connection."
        : pcmStatus === "unavailable"
          ? "Live transcription is unavailable on this server. Durable audio is still recording."
          : null;

  return {
    ...status,
    capture_phase: local.authority === "recovery"
      ? ((status.webm_bytes || 0) > 0 ? "interrupted_recovery" : "interrupted_recovery_empty")
      : status.capture_phase,
    mic_level: local.micLevel,
    mic_audio_frame_count: local.micAudioFrameCount,
    web_capture_owner: local.authority === "recovery" ? "recovery" : "local",
    web_durable_audio_status: durableStatus,
    web_live_pcm_status: pcmStatus,
    web_capture_warning: status.web_finalization_error || durableWarning,
    web_live_pcm_warning: pcmWarning,
  };
}
