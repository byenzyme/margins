// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { ReachabilityDeadline, applyRecorderTransition, hostAcknowledgedCapture, releaseBrowserMedia } from "./browser-capture.js";
import type { PanelState } from "./contracts.js";
import { CAPTURE_DISCONNECT_GRACE_MS } from "./contracts.js";

describe("browser capture ownership", () => {
  const state = (value: Partial<PanelState>): PanelState => ({
    schema: "margins.bb.recording.panel.v2",
    state: "recording", title: "Recording", detail: "Microphone only", sourceLabel: "Microphone only",
    storageLabel: "Saved to this bb project", primaryAction: "pause", primaryLabel: "Pause",
    canStop: true, canEditNotepad: true, ownsRecording: true, recordingId: "rec-1",
    notepad: null, error: null, ...value,
  });

  it("only renews the local lease for a host-acknowledged active recording", () => {
    expect(hostAcknowledgedCapture(state({ state: "recording" }))).toBe(true);
    expect(hostAcknowledgedCapture(state({ state: "paused" }))).toBe(true);
    expect(hostAcknowledgedCapture(state({ state: "needs_attention", error: { code: "offline", message: "offline", retryable: true } }))).toBe(false);
    expect(hostAcknowledgedCapture(state({ state: "recovering" }))).toBe(false);
  });

  it("changes local pause state only after the project acknowledges it", () => {
    const recorder = { pause: vi.fn(), resume: vi.fn() };
    const capture = { recorder, paused: false };
    expect(applyRecorderTransition(capture, state({ state: "needs_attention", error: { code: "offline", message: "offline", retryable: true } }), "paused")).toBe(false);
    expect(recorder.pause).not.toHaveBeenCalled();
    expect(capture.paused).toBe(false);
    expect(applyRecorderTransition(capture, state({ state: "paused" }), "paused")).toBe(true);
    expect(recorder.pause).toHaveBeenCalledOnce();
    expect(capture.paused).toBe(true);
  });

  it("ends local capture at the disconnect deadline unless reachability returns", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000);
    const expired = vi.fn();
    const deadline = new ReachabilityDeadline(CAPTURE_DISCONNECT_GRACE_MS, expired);
    deadline.failed();
    await vi.advanceTimersByTimeAsync(CAPTURE_DISCONNECT_GRACE_MS - 1);
    expect(expired).not.toHaveBeenCalled();
    deadline.acknowledged();
    await vi.advanceTimersByTimeAsync(CAPTURE_DISCONNECT_GRACE_MS + 1);
    expect(expired).not.toHaveBeenCalled();
    deadline.failed();
    await vi.advanceTimersByTimeAsync(CAPTURE_DISCONNECT_GRACE_MS);
    expect(expired).toHaveBeenCalledOnce();
    vi.useRealTimers();
  });

  it("releases microphone tracks even when the final upload cannot flush", async () => {
    const stopTrack = vi.fn();
    const recorder = {
      stop: vi.fn(function (this: { onstop?: () => void }) { this.onstop?.(); }),
      onstop: null,
    } as unknown as MediaRecorder;
    const uploads = { close: vi.fn(async () => { throw new Error("offline"); }) };
    const stream = { getTracks: () => [{ stop: stopTrack }] } as unknown as MediaStream;
    await expect(releaseBrowserMedia({ recorder, uploads, stream })).resolves.toBeUndefined();
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(uploads.close).toHaveBeenCalledOnce();
  });
});
