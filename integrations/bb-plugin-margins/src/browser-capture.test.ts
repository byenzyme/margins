// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { BrowserCaptureOwner, ReachabilityDeadline, applyRecorderTransition, hostAcknowledgedCapture, releaseBrowserMedia, type BrowserCaptureDependencies } from "./browser-capture.js";
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

  it("uses the last acknowledgement as a continuing disconnect deadline", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000);
    const expired = vi.fn();
    const deadline = new ReachabilityDeadline(CAPTURE_DISCONNECT_GRACE_MS, expired);
    deadline.failed();
    await vi.advanceTimersByTimeAsync(CAPTURE_DISCONNECT_GRACE_MS - 1);
    expect(expired).not.toHaveBeenCalled();
    deadline.acknowledged();
    await vi.advanceTimersByTimeAsync(CAPTURE_DISCONNECT_GRACE_MS);
    expect(expired).toHaveBeenCalledOnce();
    vi.useRealTimers();
  });

  function controllerFixture(rpc: BrowserCaptureDependencies["rpc"]) {
    sessionStorage.clear();
    const stopTrack = vi.fn();
    const recorder = {
      ondataavailable: null,
      onstop: null,
      start: vi.fn(), pause: vi.fn(), resume: vi.fn(),
      stop: vi.fn(),
    } as unknown as MediaRecorder;
    const stream = { getTracks: () => [{ stop: stopTrack }] } as unknown as MediaStream;
    const owner = new BrowserCaptureOwner({
      rpc,
      acquireMicrophone: async () => stream,
      createRecorder: () => recorder,
      supportsMime: () => false,
      heartbeatMs: 10,
      disconnectGraceMs: 100,
    });
    owner.install({ pluginId: "margins" } as never);
    return { owner, recorder, stopTrack };
  }

  it("pauses local production before a hung control request settles", async () => {
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method) => {
      if (method === "beginBrowserCapture") return state({}) as never;
      return new Promise(() => {});
    };
    const { owner, recorder } = controllerFixture(rpc);
    await owner.start("thr-1");
    void owner.pause();
    expect(recorder.pause).toHaveBeenCalledOnce();
  });

  it("releases tracks immediately when Stop transport and recorder stop event both hang", async () => {
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method) => {
      if (method === "beginBrowserCapture") return state({}) as never;
      return new Promise(() => {});
    };
    const { owner, stopTrack } = controllerFixture(rpc);
    await owner.start("thr-1");
    void owner.stop();
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(owner.active).toBe(false);
  });

  it("allows only one hung heartbeat and still expires the local producer", async () => {
    vi.useFakeTimers();
    let heartbeatCalls = 0;
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method) => {
      if (method === "beginBrowserCapture") return state({}) as never;
      if (method === "heartbeat") heartbeatCalls += 1;
      return new Promise(() => {});
    };
    const { owner, stopTrack } = controllerFixture(rpc);
    await owner.start("thr-1");
    await vi.advanceTimersByTimeAsync(99);
    expect(heartbeatCalls).toBe(1);
    expect(stopTrack).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(owner.active).toBe(false);
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
