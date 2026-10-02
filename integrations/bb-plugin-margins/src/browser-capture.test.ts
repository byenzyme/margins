// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { BrowserCaptureOwner, ReachabilityDeadline, hostAcknowledgedCapture, releaseBrowserMedia, type BrowserCaptureDependencies } from "./browser-capture.js";
import type { PanelState } from "./contracts.js";
import { CAPTURE_DISCONNECT_GRACE_MS } from "./contracts.js";

describe("browser capture ownership", () => {
  const state = (value: Partial<PanelState>): PanelState => ({
    schema: "margins.bb.recording.panel.v2",
    state: "recording", title: "Recording", detail: "Microphone only", sourceLabel: "Microphone only",
    storageLabel: "Saved to this bb project", primaryAction: "pause", primaryLabel: "Pause",
    canStop: true, canEditNotepad: true, ownsRecording: true, recordingId: "rec-1", sessionId: "rec-1",
    notepad: null, lastSessionId: null, error: null, ...value,
  });

  it("only renews the local lease for a host-acknowledged active recording", () => {
    expect(hostAcknowledgedCapture(state({ state: "recording" }))).toBe(true);
    expect(hostAcknowledgedCapture(state({ state: "paused" }))).toBe(true);
    expect(hostAcknowledgedCapture(state({ state: "needs_attention", error: { code: "offline", message: "offline", retryable: true } }))).toBe(false);
    expect(hostAcknowledgedCapture(state({ state: "recovering" }))).toBe(false);
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

  it("stops local production before a hung pause control request settles", async () => {
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method) => {
      if (method === "beginProjectCapture") return state({}) as never;
      return new Promise(() => {});
    };
    const { owner, recorder } = controllerFixture(rpc, function () { this.onstop?.(new Event("stop")); });
    await owner.startFromProject("proj-1");
    void owner.pause();
    expect(recorder.stop).toHaveBeenCalledOnce();
    expect(recorder.pause).not.toHaveBeenCalled();
  });

  it("drains a standalone WebM before Pause and starts a new recorder after Resume", async () => {
    const order: string[] = [];
    const uploaded: number[] = [];
    let releaseFirstUpload!: () => void;
    const firstUpload = new Promise<void>((resolve) => { releaseFirstUpload = resolve; });
    vi.stubGlobal("fetch", vi.fn(async (_url: string, options: RequestInit) => {
      const input = JSON.parse(String(options.body)) as { sequence: number };
      uploaded.push(input.sequence);
      if (input.sequence === 0) await firstUpload;
      order.push(`ack-${input.sequence}`);
      return new Response(JSON.stringify({ ok: true }), {
        status: 200, headers: { "content-type": "application/json" },
      });
    }));
    const recorders: MediaRecorder[] = [];
    const recorderStreams: MediaStream[] = [];
    const stream = { getTracks: () => [{ stop: vi.fn() }] } as unknown as MediaStream;
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginProjectCapture") return state({}) as never;
      if (method === "pause") {
        order.push(`pause-${(input as { expectedNextSequence: number }).expectedNextSequence}`);
        return state({ state: "paused" }) as never;
      }
      if (method === "resume") {
        order.push("resume-ack");
        return state({ state: "recording" }) as never;
      }
      if (method === "stop") {
        order.push(`stop-${(input as { expectedNextSequence: number }).expectedNextSequence}`);
        return state({ state: "saved", recordingId: null, ownsRecording: false }) as never;
      }
      return state({}) as never;
    };
    const owner = new BrowserCaptureOwner({
      rpc, acquireMicrophone: async () => stream,
      createRecorder: (mediaStream) => {
        recorderStreams.push(mediaStream);
        const segment = recorders.length;
        const recorder = {
          ondataavailable: null, onstop: null,
          start: vi.fn(() => { order.push(`start-${segment}`); }),
          pause: vi.fn(), resume: vi.fn(),
          stop: vi.fn(function (this: MediaRecorder) {
            order.push(`recorder-stop-${segment}`);
            this.ondataavailable?.({ data: new Blob([`webm-header-segment-${segment}`]) } as BlobEvent);
            this.onstop?.(new Event("stop"));
          }),
        } as unknown as MediaRecorder;
        recorders.push(recorder);
        return recorder;
      },
      supportsMime: () => false, heartbeatMs: 100_000, disconnectGraceMs: 100_000,
    });
    owner.install({ pluginId: "margins" } as never);
    await owner.startFromProject("proj-1");
    // The first recorder only checks permission before server allocation.
    const firstSegment = recorders[1]!;
    expect(firstSegment.start).toHaveBeenCalledOnce();
    const pausing = owner.pause();
    await vi.waitFor(() => expect(uploaded).toEqual([0]));
    expect(order).not.toContain("pause-1");
    releaseFirstUpload();
    await expect(pausing).resolves.toMatchObject({ state: "paused", error: null });
    expect(order).toEqual(["start-1", "recorder-stop-1", "ack-0", "pause-1"]);
    expect(firstSegment.pause).not.toHaveBeenCalled();

    await expect(owner.resume()).resolves.toMatchObject({ state: "recording", error: null });
    const secondSegment = recorders[2]!;
    expect(secondSegment).not.toBe(firstSegment);
    expect(secondSegment.start).toHaveBeenCalledOnce();
    expect(recorderStreams[2]).toBe(stream);
    expect(order.slice(-2)).toEqual(["resume-ack", "start-2"]);

    await expect(owner.stop()).resolves.toMatchObject({ state: "saved", error: null });
    expect(uploaded).toEqual([0, 1]);
    expect(firstSegment.stop).toHaveBeenCalledOnce();
    expect(secondSegment.stop).toHaveBeenCalledOnce();
    expect(order.slice(-3)).toEqual(["recorder-stop-2", "ack-1", "stop-2"]);
  });

  it("replays a missing acknowledged chunk before closing a paused segment", async () => {
    const order: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, options: RequestInit) => {
      const { sequence } = JSON.parse(String(options.body)) as { sequence: number };
      order.push(`upload-${sequence}`);
      return new Response(JSON.stringify({ ok: true }), {
        status: 200, headers: { "content-type": "application/json" },
      });
    }));
    let pauseCalls = 0;
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginProjectCapture") return state({}) as never;
      if (method === "pause") {
        pauseCalls += 1;
        order.push(`pause-${(input as { expectedNextSequence: number }).expectedNextSequence}`);
        return state(pauseCalls === 1 ? {
          state: "needs_attention",
          error: { code: "browser_chunk_gap", message: "Missing browser audio sequence 0; retry the chunk before Stop.", retryable: true },
        } : { state: "paused" }) as never;
      }
      return state({ state: "saved", recordingId: null }) as never;
    };
    const { owner, recorder } = controllerFixture(rpc, function () {
      this.ondataavailable?.({ data: new Blob(["complete-webm"]) } as BlobEvent);
      this.onstop?.(new Event("stop"));
    });
    await owner.startFromProject("proj-1");
    await expect(owner.pause()).resolves.toMatchObject({ state: "paused", error: null });
    expect(order).toEqual(["upload-0", "pause-1", "upload-0", "pause-1"]);
    expect(recorder.stop).toHaveBeenCalledOnce();
    await owner.stop();
    expect(recorder.stop).toHaveBeenCalledOnce();
  });

  it("leaves Pause unacknowledged and visibly incomplete if its final upload cannot drain", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ ok: false, error: "disk full" }), {
      status: 502, headers: { "content-type": "application/json" },
    })));
    const pauseInputs: object[] = [];
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginProjectCapture") return state({}) as never;
      if (method === "pause") pauseInputs.push(input);
      return state({}) as never;
    };
    const { owner } = controllerFixture(rpc, function () {
      this.ondataavailable?.({ data: new Blob(["last-webm"]) } as BlobEvent);
      this.onstop?.(new Event("stop"));
    });
    await owner.startFromProject("proj-1");
    await expect(owner.pause()).resolves.toMatchObject({
      state: "needs_attention", title: "Recording incomplete",
      error: { code: "browser_audio_drain_incomplete" },
    });
    expect(pauseInputs).toHaveLength(0);
    expect(owner.active).toBe(true);
  });

  it("keeps the session overlay live while another thread shows a ready panel", async () => {
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method) =>
      method === "beginProjectCapture" ? state({}) as never : state({}) as never;
    const { owner } = controllerFixture(rpc);
    await owner.startFromProject("proj-1");
    owner.acceptPanel("thr-1", state({}));
    owner.acceptPanel("thr-2", state({ state: "ready", recordingId: null, ownsRecording: false }));
    expect(owner.panel()).toMatchObject({ state: "recording", recordingId: "rec-1" });
    expect(owner.panel("thr-2")).toMatchObject({ state: "ready" });
  });

  it("shows a silence warning after three seconds of browser capture", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000);
    const { owner } = controllerFixture(async () => state({}) as never);
    await owner.startFromProject("proj-1");
    expect(owner.noAudioWarning).toBe(false);
    vi.setSystemTime(4_100);
    expect(owner.noAudioWarning).toBe(true);
  });

  it("replaces the overlay state when Stop returns a saved panel without a recording id", async () => {
    const { owner } = controllerFixture(async () => state({}) as never);
    await owner.startFromProject("proj-1");
    owner.acceptPanel("rec-1", state({ state: "saving" }));
    owner.acceptPanel("rec-1", state({ state: "saved", recordingId: null, ownsRecording: false, canStop: false }));
    expect(owner.panel()).toMatchObject({ state: "saved" });
  });

  it("releases tracks immediately when Stop transport and recorder stop event both hang", async () => {
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method) => {
      if (method === "beginProjectCapture") return state({}) as never;
      return new Promise(() => {});
    };
    const { owner, stopTrack } = controllerFixture(rpc);
    await owner.startFromProject("proj-1");
    void owner.stop();
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(owner.active).toBe(false);
  });

  it("bounds a missing recorder stop event without falsely confirming a saved recording", async () => {
    vi.useFakeTimers();
    const stopInputs: object[] = [];
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginProjectCapture") return state({}) as never;
      if (method === "stop") {
        stopInputs.push(input);
        return state({ state: "saved", error: null }) as never;
      }
      return state({}) as never;
    };
    const { owner, stopTrack } = controllerFixture(rpc);
    await owner.startFromProject("proj-1");

    const stopping = owner.stop();
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(stopInputs).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(5_000);
    await expect(stopping).resolves.toMatchObject({
      state: "needs_attention",
      error: { code: "browser_audio_drain_incomplete" },
    });
    expect(stopInputs).toHaveLength(0);
    expect(sessionStorage.getItem("margins.bb.capture.v1")).not.toBeNull();
  });

  it("releases tracks immediately but does not let server Stop overtake the final durable upload", async () => {
    let resolveUpload!: (response: Response) => void;
    const upload = new Promise<Response>((resolve) => { resolveUpload = resolve; });
    const order: string[] = [];
    const uploadFetch = vi.fn(async (_url: string, _options: RequestInit) => upload.then((response) => {
      order.push("upload-durable");
      return response;
    }));
    vi.stubGlobal("fetch", uploadFetch);
    const stopInputs: object[] = [];
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginProjectCapture") return state({}) as never;
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
    await owner.startFromProject("proj-1");
    owner.acceptPanel("thr-1", state({}));

    const stopping = owner.stop();
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(owner.active).toBe(false);
    expect(owner.panel("thr-1")).toMatchObject({ state: "saving", recordingId: "rec-1" });
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledOnce());
    const uploadBody = JSON.parse(String(uploadFetch.mock.calls[0]?.[1]?.body));
    expect(uploadBody).toMatchObject({ sessionId: "rec-1" });
    expect(uploadBody).not.toHaveProperty("threadId");
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
      if (method === "beginProjectCapture") return state({}) as never;
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
    await owner.startFromProject("proj-1");

    await expect(owner.stop()).resolves.toMatchObject({
      state: "needs_attention",
      error: { code: "offline" },
      recordingId: "rec-1",
    });
    const retained = JSON.parse(sessionStorage.getItem("margins.bb.capture.v1") || "null");
    expect(retained).toMatchObject({
      sessionId: "rec-1",
      nextSequence: 1,
      pendingControl: { kind: "stop", operationId: expect.any(String) },
    });
    expect(retained).not.toHaveProperty("threadId");
    await expect(owner.retryPendingStop()).resolves.toMatchObject({ state: "saved", error: null });
    expect(stopInputs[1]).toMatchObject({ operationId: (stopInputs[0] as { operationId: string }).operationId });
    expect(sessionStorage.getItem("margins.bb.capture.v1")).toBeNull();
  });

  it("replays a server-reported missing sequence before retrying Stop", async () => {
    const order: string[] = [];
    const uploaded: Array<{ sequence: number; bytesBase64: string }> = [];
    const uploadFetch = vi.fn(async (_url: string, options: RequestInit) => {
      const body = JSON.parse(String(options.body)) as { sequence: number; bytesBase64: string };
      uploaded.push(body);
      order.push(`upload-${body.sequence}`);
      return new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    });
    vi.stubGlobal("fetch", uploadFetch);
    const stopInputs: Array<{ operationId: string; expectedNextSequence: number }> = [];
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginProjectCapture") return state({}) as never;
      if (method === "stop") {
        stopInputs.push(input as { operationId: string; expectedNextSequence: number });
        const gap = stopInputs.length === 1;
        order.push(gap ? "stop-gap" : "stop-saved");
        return state(gap ? {
          state: "needs_attention",
          error: {
            code: "browser_chunk_gap",
            message: "Missing browser audio sequence 0; retry the chunk before Stop.",
            retryable: true,
          },
        } : { state: "saved", error: null }) as never;
      }
      return state({}) as never;
    };
    const { owner } = controllerFixture(rpc, function () {
      this.ondataavailable?.({ data: new Blob(["final-webm"]) } as BlobEvent);
      this.onstop?.(new Event("stop"));
    });
    await owner.startFromProject("proj-1");

    await expect(owner.stop()).resolves.toMatchObject({
      state: "needs_attention",
      error: { code: "browser_chunk_gap" },
      recordingId: "rec-1",
    });
    expect(order).toEqual(["upload-0", "stop-gap"]);
    expect(sessionStorage.getItem("margins.bb.capture.v1")).not.toBeNull();
    await expect(owner.retryPendingStop()).resolves.toMatchObject({ state: "saved", error: null });

    expect(order).toEqual(["upload-0", "stop-gap", "upload-0", "stop-saved"]);
    expect(uploaded[1]).toEqual(uploaded[0]);
    expect(stopInputs).toHaveLength(2);
    expect(stopInputs[1]).toEqual(stopInputs[0]);
    expect(stopInputs[0]?.expectedNextSequence).toBe(1);
    expect(sessionStorage.getItem("margins.bb.capture.v1")).toBeNull();
  });

  it("does not report saved or discard recovery identity after a failed final upload", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ ok: false, error: "disk full" }), {
      status: 502,
      headers: { "content-type": "application/json" },
    })));
    const stopInputs: object[] = [];
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      if (method === "beginProjectCapture") return state({}) as never;
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
    await owner.startFromProject("proj-1");

    await expect(owner.stop()).resolves.toMatchObject({
      state: "needs_attention",
      error: { code: "browser_audio_drain_incomplete" },
      recordingId: "rec-1",
    });
    expect(stopTrack).toHaveBeenCalledOnce();
    expect(stopInputs).toHaveLength(0);
    const retained = JSON.parse(sessionStorage.getItem("margins.bb.capture.v1") || "null");
    expect(retained).toMatchObject({
      sessionId: "rec-1",
      pendingControl: { kind: "stop", operationId: expect.any(String) },
      stopDrainError: { code: "browser_audio_drain_incomplete" },
    });
    await expect(owner.retryPendingStop()).resolves.toMatchObject({
      state: "needs_attention",
      error: { code: "browser_audio_drain_incomplete" },
    });
    expect(stopInputs).toHaveLength(0);
    expect(sessionStorage.getItem("margins.bb.capture.v1")).not.toBeNull();
  });

  it("allows only one hung heartbeat and still expires the local producer", async () => {
    vi.useFakeTimers();
    let heartbeatCalls = 0;
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method) => {
      if (method === "beginProjectCapture") return state({}) as never;
      if (method === "heartbeat") heartbeatCalls += 1;
      return new Promise(() => {});
    };
    const { owner, stopTrack } = controllerFixture(rpc);
    await owner.startFromProject("proj-1");
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
