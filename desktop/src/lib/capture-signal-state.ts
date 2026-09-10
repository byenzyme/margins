import type { CaptureDeviceState } from "./tauri";

export type CaptureSignalState = "starting" | "ready" | "quiet" | "paused" | "off" | "issue";

export interface CaptureSignalLane {
  state: CaptureSignalState;
  label: string;
}

export interface CaptureSignalStatus {
  is_recording: boolean;
  paused?: boolean;
  capture_device?: CaptureDeviceState | null;
  mic_audio_frame_count?: number;
  system_audio_expected?: boolean;
  system_audio_frame_count?: number;
  tap_status?: string;
  capture_phase?: string;
  web_durable_audio_status?: string;
  web_live_pcm_status?: string;
}

export function captureSignalReadiness(
  status: CaptureSignalStatus,
  startup: { phase: "starting" | "failed"; cancelRequested?: boolean } | null,
): { mic: CaptureSignalLane; system: CaptureSignalLane } {
  if (startup?.phase === "starting") {
    const label = startup.cancelRequested ? "Stopping" : "Starting";
    return {
      mic: { state: "starting", label },
      system: { state: "starting", label },
    };
  }

  if (status.capture_phase === "interrupted" || status.web_durable_audio_status === "interrupted") {
    return {
      mic: { state: "issue", label: "Interrupted" },
      system: { state: "off", label: "Not used" },
    };
  }

  if (status.capture_phase === "capturing_elsewhere") {
    return {
      mic: { state: "ready", label: "Other tab" },
      system: { state: "off", label: "Not used" },
    };
  }

  if (status.capture_phase?.startsWith("interrupted_recovery")) {
    return {
      mic: { state: "issue", label: "Interrupted" },
      system: { state: "off", label: "Not used" },
    };
  }

  if (status.paused) {
    return {
      mic: { state: "paused", label: "Paused" },
      system: { state: "paused", label: "Paused" },
    };
  }

  if (!status.is_recording) {
    return {
      mic: { state: "off", label: "Not live" },
      system: { state: "off", label: "Not live" },
    };
  }

  const captureDevice = status.capture_device;
  const mic = captureDevice?.state === "holding"
    ? { state: "issue" as const, label: "No input" }
    : captureDevice?.state === "switching"
      ? { state: "starting" as const, label: "Switching" }
      : status.web_durable_audio_status === "healthy" || (status.mic_audio_frame_count || 0) > 0
        ? { state: "ready" as const, label: "Live" }
        : { state: "starting" as const, label: "Waiting" };

  let system: CaptureSignalLane;
  if (!status.system_audio_expected) {
    system = { state: "off", label: "Not used" };
  } else if (status.tap_status === "blocked") {
    system = { state: "issue", label: "Interrupted" };
  } else if ((status.system_audio_frame_count || 0) > 0) {
    system = status.tap_status === "quiet" || status.tap_status === "silent"
      ? { state: "quiet", label: "Quiet" }
      : { state: "ready", label: "Live" };
  } else {
    system = { state: "starting", label: "Waiting" };
  }

  return { mic, system };
}
