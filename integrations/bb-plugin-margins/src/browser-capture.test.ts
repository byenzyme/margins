// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
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

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    vi.useRealTimers();
    sessionStorage.clear();
  });

  function controllerFixture(
    rpc: BrowserCaptureDependencies["rpc"],
    stopRecorder?: (this: MediaRecorder) => void,
  ) {
    sessionStorage.clear();
    const stopTrack = vi.fn();
    const recorder = {
      ondataavailable: null,
      onstop: null,
      start: vi.fn(), pause: vi.fn(), resume: vi.fn(),
      stop: vi.fn(function (this: MediaRecorder) { stopRecorder?.call(this); }),
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

  it("bounds a missing recorder stop event without falsely confirming a saved recording", async () => {
    vi.useFakeTimers();
    const stopInputs: object[] = [];
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginBrowserCapture") return state({}) as never;
      if (method === "stop") {
        stopInputs.push(input);
        return state({ state: "saved", error: null }) as never;
      }
      return state({}) as never;
    };
    const { owner, stopTrack } = controllerFixture(rpc);
    await owner.start("thr-1");

    const stopping = owner.stop();
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(stopInputs).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(5_000);
    await expect(stopping).resolves.toMatchObject({
      state: "needs_attention",
      error: { code: "browser_audio_drain_incomplete" },
    });
    expect(stopInputs).toHaveLength(1);
    expect(sessionStorage.getItem("margins.bb.capture.v1")).not.toBeNull();
  });

  it("releases tracks immediately but does not let server Stop overtake the final durable upload", async () => {
    let resolveUpload!: (response: Response) => void;
    const upload = new Promise<Response>((resolve) => { resolveUpload = resolve; });
    const order: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async () => upload.then((response) => {
      order.push("upload-durable");
      return response;
    })));
    const stopInputs: object[] = [];
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginBrowserCapture") return state({}) as never;
      if (method === "stop") {
        order.push("server-stop");
        stopInputs.push(input);
        return state({ state: "saved", error: null }) as never;
      }
      return state({}) as never;
    };
    const { owner, stopTrack } = controllerFixture(rpc, function () {
      this.ondataavailable?.({ data: new Blob(["final-webm"]) } as BlobEvent);
      this.onstop?.(new Event("stop"));
    });
    await owner.start("thr-1");
    owner.acceptPanel("thr-1", state({}));

    const stopping = owner.stop();
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(owner.active).toBe(false);
    expect(owner.panel("thr-1")).toMatchObject({ state: "saving", recordingId: "rec-1" });
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledOnce());
    expect(stopInputs).toHaveLength(0);

    resolveUpload(new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { "content-type": "application/json" },
    }));
    await expect(stopping).resolves.toMatchObject({ state: "saved", error: null });
    expect(order).toEqual(["upload-durable", "server-stop"]);
    expect(sessionStorage.getItem("margins.bb.capture.v1")).toBeNull();
  });

  it("retains a resolved Stop error and retries with the same operation identity", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { "content-type": "application/json" },
    })));
    const stopInputs: object[] = [];
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginBrowserCapture") return state({}) as never;
      if (method === "stop") {
        stopInputs.push(input);
        return state(stopInputs.length === 1 ? {
          state: "needs_attention",
          error: { code: "offline", message: "project machine offline", retryable: true },
        } : { state: "saved", error: null }) as never;
      }
      return state({}) as never;
    };
    const { owner } = controllerFixture(rpc, function () {
      this.ondataavailable?.({ data: new Blob(["final-webm"]) } as BlobEvent);
      this.onstop?.(new Event("stop"));
    });
    await owner.start("thr-1");

    await expect(owner.stop()).resolves.toMatchObject({
      state: "needs_attention",
      error: { code: "offline" },
      recordingId: "rec-1",
    });
    const retained = JSON.parse(sessionStorage.getItem("margins.bb.capture.v1") || "null");
    expect(retained).toMatchObject({
      recordingId: "rec-1",
      nextSequence: 1,
      pendingControl: { kind: "stop", operationId: expect.any(String) },
    });
    await expect(owner.retryPendingStop()).resolves.toMatchObject({ state: "saved", error: null });
    expect(stopInputs[1]).toMatchObject({ operationId: (stopInputs[0] as { operationId: string }).operationId });
    expect(sessionStorage.getItem("margins.bb.capture.v1")).toBeNull();
  });

  it("does not report saved or discard recovery identity after a failed final upload", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ ok: false, error: "disk full" }), {
      status: 502,
      headers: { "content-type": "application/json" },
    })));
    const stopInputs: object[] = [];
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginBrowserCapture") return state({}) as never;
      if (method === "stop") {
        stopInputs.push(input);
        return state({ state: "saved", error: null }) as never;
      }
      return state({}) as never;
    };
    const { owner, stopTrack } = controllerFixture(rpc, function () {
      this.ondataavailable?.({ data: new Blob(["last-undurable-chunk"]) } as BlobEvent);
      this.onstop?.(new Event("stop"));
    });
    await owner.start("thr-1");

    await expect(owner.stop()).resolves.toMatchObject({
      state: "needs_attention",
      error: { code: "browser_audio_drain_incomplete" },
      recordingId: "rec-1",
    });
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(stopInputs).toHaveLength(1);
    const retained = JSON.parse(sessionStorage.getItem("margins.bb.capture.v1") || "null");
    expect(retained).toMatchObject({
      recordingId: "rec-1",
      pendingControl: { kind: "stop", operationId: (stopInputs[0] as { operationId: string }).operationId },
      stopDrainError: { code: "browser_audio_drain_incomplete" },
    });
    await expect(owner.retryPendingStop()).resolves.toMatchObject({
      state: "needs_attention",
      error: { code: "browser_audio_drain_incomplete" },
    });
    expect(stopInputs[1]).toMatchObject({ operationId: (stopInputs[0] as { operationId: string }).operationId });
    expect(sessionStorage.getItem("margins.bb.capture.v1")).not.toBeNull();
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
    await expect(releaseBrowserMedia({ recorder, uploads, stream })).resolves.toMatchObject({
      errors: [expect.objectContaining({ message: "offline" })],
    });
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(uploads.close).toHaveBeenCalledOnce();
  });
});
