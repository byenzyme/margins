import assert from "node:assert/strict";
import test from "node:test";
import { createServer } from "vite";

import {
  activeHostedPreflightStatus,
  runHostedStartPreflight,
} from "../src/lib/hosted-start-preflight.ts";
import type { RecordingStatus, WebRecordingRecoveryStatus } from "../src/lib/tauri.ts";

const recovery: WebRecordingRecoveryStatus = {
  recording_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  session_name: "same-name",
  elapsed_secs: 20,
  finalization_error: "retained",
  owner_lease_active: false,
  recovery_phase: "failed",
};

function backendStatus(recordingId: string): RecordingStatus {
  return {
    is_recording: true,
    paused: false,
    session_name: "same-name",
    web_recording_id: recordingId,
    web_recoveries: [recovery],
  } as RecordingStatus;
}

test("HTTP start preflight accepts a backend-shaped retained recovery response", () => {
  assert.equal(
    activeHostedPreflightStatus(backendStatus(recovery.recording_id), []),
    null,
  );
});

test("HTTP start preflight still blocks a distinct active recording", () => {
  const activeB = backendStatus("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
  assert.equal(activeHostedPreflightStatus(activeB, [recovery])?.web_recording_id, activeB.web_recording_id);
});

test("actual async HTTP preflight integrates backend-shaped recovery status without blocking Start B", async () => {
  const calls: string[] = [];
  const active = await runHostedStartPreflight(
    async () => {
      calls.push("status");
      return backendStatus(recovery.recording_id);
    },
    async () => {
      calls.push("recoveries");
      return [recovery];
    },
  );
  assert.equal(active, null);
  assert.deepEqual(calls.sort(), ["recoveries", "status"]);
});

test("startRecording HTTP preflight reaches microphone acquisition when global status is retained A", async () => {
  const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const originalFetch = globalThis.fetch;
  let microphoneRequests = 0;

  class FakeMediaRecorder {
    static isTypeSupported(mimeType: string): boolean {
      return mimeType === "audio/webm;codecs=opus";
    }
  }

  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { isSecureContext: true, MediaRecorder: FakeMediaRecorder },
  });
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {
      mediaDevices: {
        getUserMedia: async () => {
          microphoneRequests += 1;
          const denied = new Error("test permission stop");
          denied.name = "NotAllowedError";
          throw denied;
        },
      },
    },
  });
  globalThis.fetch = async input => {
    const url = String(input);
    const result = url.endsWith("/get_recording_status")
      ? backendStatus(recovery.recording_id)
      : url.endsWith("/list_web_recording_recoveries")
        ? [recovery]
        : assert.fail(`unexpected HTTP command: ${url}`);
    return new Response(JSON.stringify({ ok: true, result }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  };

  const vite = await createServer({ appType: "custom", server: { middlewareMode: true } });
  try {
    const { startRecording } = await vite.ssrLoadModule("/src/lib/http-backend.ts") as {
      startRecording: (name: string) => Promise<string>;
    };
    await assert.rejects(
      startRecording("new-active-b"),
      (error: Error & { category?: string }) => error.category === "permission-denied",
    );
    assert.equal(microphoneRequests, 1);
  } finally {
    await vite.close();
    globalThis.fetch = originalFetch;
    if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
    else Reflect.deleteProperty(globalThis, "window");
    if (originalNavigator) Object.defineProperty(globalThis, "navigator", originalNavigator);
    else Reflect.deleteProperty(globalThis, "navigator");
  }
});
