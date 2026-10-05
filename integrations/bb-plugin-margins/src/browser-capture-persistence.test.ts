// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { IDBFactory } from "fake-indexeddb";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BrowserCaptureOwner, type BrowserCaptureDependencies } from "./browser-capture.js";
import { BrowserChunkStore } from "./browser-chunk-store.js";
import type { PanelState } from "./contracts.js";

const CAPTURE_KEY = "margins.bb.capture.v1";

const state = (value: Partial<PanelState>): PanelState => ({
  schema: "margins.bb.recording.panel.v2",
  state: "recording", title: "Recording", detail: "Microphone only", sourceLabel: "Microphone only",
  storageLabel: "Saved to this bb project", primaryAction: "pause", primaryLabel: "Pause",
  canStop: true, canEditNotepad: true, ownsRecording: true, recordingId: "rec-1", sessionId: "rec-1",
  notepad: null, lastSessionId: null, error: null, ...value,
});

const ok = () => new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } });

function uploadedBody(options: RequestInit) {
  const body = JSON.parse(String(options.body)) as { sequence: number; bytesBase64: string };
  return { sequence: body.sequence, text: atob(body.bytesBase64) };
}

const refused = () => new Response(JSON.stringify({ ok: false, error: {
  code: "sequence_conflict", message: "audio upload failed (409): lane sequence is already declared discontinuous", retryable: false,
} }), { status: 502, headers: { "content-type": "application/json" } });
const unavailable = () => new Response(JSON.stringify({ ok: false, error: {
  code: "audio_upload_failed", message: "audio upload failed (503)", retryable: true,
} }), { status: 502, headers: { "content-type": "application/json" } });
const chunkTiming = (sequence: number) => ({ capturedStartUnixMs: 1_000 + sequence * 3_000, capturedEndUnixMs: 4_000 + sequence * 3_000 });

async function seed(store: BrowserChunkStore, sessionId: string, sequences: number[]) {
  for (const sequence of sequences) await store.persist(sessionId, sequence, new Blob([`chunk-${sequence}`]), chunkTiming(sequence));
}

function media() {
  const recorder = {
    ondataavailable: null, onstop: null,
    start: vi.fn(), pause: vi.fn(), resume: vi.fn(),
    stop: vi.fn(function (this: MediaRecorder) { this.onstop?.(new Event("stop")); }),
  } as unknown as MediaRecorder;
  const stream = { getTracks: () => [{ stop: vi.fn() }] } as unknown as MediaStream;
  return { recorder, stream };
}

function owner(rpc: BrowserCaptureDependencies["rpc"], chunkStore: BrowserChunkStore, recorder: MediaRecorder, stream: MediaStream,
  acquireMicrophone: () => Promise<MediaStream> = async () => stream) {
  const instance = new BrowserCaptureOwner({
    rpc, acquireMicrophone, createRecorder: () => recorder,
    supportsMime: () => false, heartbeatMs: 100_000, disconnectGraceMs: 100_000, chunkStore, persistedRetryDelaysMs: [0],
  });
  const uninstall = instance.install({ pluginId: "margins" } as never);
  return { owner: instance, uninstall };
}

describe("browser capture chunk persistence", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  it("re-uploads unacknowledged chunks after a reload before rotating the segment, so nothing is declared missing", async () => {
    sessionStorage.clear();
    const factory = new IDBFactory();
    const firstPage = new BrowserChunkStore(() => factory);
    // Page 1: sequence 0 is acknowledged, sequence 1 is still in flight at reload.
    vi.stubGlobal("fetch", vi.fn(async (_url: string, options: RequestInit) => {
      const { sequence } = uploadedBody(options);
      if (sequence === 0) return ok();
      return new Promise<Response>((_resolve, reject) => {
        options.signal?.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
      });
    }));
    const first = media();
    const page1 = owner(async (_plugin, method) => method === "beginProjectCapture"
      ? state({ nextSequence: 0 }) as never : state({}) as never, firstPage, first.recorder, first.stream);
    await page1.owner.startFromProject("proj-1");
    first.recorder.ondataavailable?.({ data: new Blob(["webm-header+0"]) } as BlobEvent);
    first.recorder.ondataavailable?.({ data: new Blob(["webm-cluster-1"]) } as BlobEvent);
    await vi.waitFor(async () => {
      expect((await firstPage.pending("rec-1")).map((chunk) => chunk.sequence)).toEqual([1]);
    });
    expect(JSON.parse(sessionStorage.getItem(CAPTURE_KEY)!)).toMatchObject({ nextSequence: 1, expectedNextSequence: 2 });
    page1.uninstall();

    // Page 2: same tab session storage, same origin IndexedDB, fresh memory.
    const order: string[] = [];
    const received = new Map<number, string>();
    vi.stubGlobal("fetch", vi.fn(async (_url: string, options: RequestInit) => {
      const body = uploadedBody(options);
      order.push(`upload-${body.sequence}`);
      received.set(body.sequence, body.text);
      return ok();
    }));
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      order.push(method);
      if (method === "readCapture") return state({ nextSequence: 1 }) as never;
      if (method === "pause") {
        // The server would declare [1, 2) missing unless sequence 1 arrived first.
        expect(input).toMatchObject({ expectedNextSequence: 2, recoveredAfterReload: true });
        expect(received.get(1)).toBe("webm-cluster-1");
        return state({ state: "paused", nextSequence: 2 }) as never;
      }
      return state({ nextSequence: 2 }) as never;
    };
    const secondPage = new BrowserChunkStore(() => factory);
    const second = media();
    const page2 = owner(rpc, secondPage, second.recorder, second.stream);
    await vi.waitFor(() => expect(page2.owner.active).toBe(true));
    expect(order).toEqual(["readCapture", "upload-1", "pause", "resume"]);
    expect(await secondPage.pending("rec-1")).toEqual([]);
    page2.uninstall();
  });

  it("delivers persisted chunks for a Stop left pending by a reload instead of declaring them lost", async () => {
    const factory = new IDBFactory();
    await new BrowserChunkStore(() => factory).persist("rec-1", 3, new Blob(["final-cluster"]), {
      capturedStartUnixMs: 1_000, capturedEndUnixMs: 4_000,
    });
    sessionStorage.setItem(CAPTURE_KEY, JSON.stringify({
      sessionId: "rec-1", projectId: "proj-1", startedAtMs: 1_000, nextSequence: 3, expectedNextSequence: 4, paused: false,
      pendingControl: { kind: "stop", operationId: "stop-1", segmentTimeUnixMs: 5_000 },
      stopDrainError: { code: "browser_audio_drain_incomplete", message: "Upload timed out", retryable: true },
    }));
    const order: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, options: RequestInit) => {
      order.push(`upload-${uploadedBody(options).sequence}`);
      return ok();
    }));
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method, input) => {
      order.push(method);
      if (method === "stop") {
        expect(input).toMatchObject({ operationId: "stop-1", expectedNextSequence: 4 });
        return state({ state: "saved", error: null, lastSessionId: "rec-1" }) as never;
      }
      return state({}) as never;
    };
    const store = new BrowserChunkStore(() => factory);
    const { recorder, stream } = media();
    const page = owner(rpc, store, recorder, stream);
    await vi.waitFor(() => expect(sessionStorage.getItem(CAPTURE_KEY)).toBeNull());
    expect(order).toEqual(["upload-3", "stop"]);
    expect(page.owner.panel()).toMatchObject({ state: "saved" });
    expect(await store.pending("rec-1")).toEqual([]);
  });

  it("keeps the 'Finish with what was saved' path when nothing was persisted", async () => {
    sessionStorage.setItem(CAPTURE_KEY, JSON.stringify({
      sessionId: "rec-1", startedAtMs: 1_000, nextSequence: 3, expectedNextSequence: 4, paused: false,
      pendingControl: { kind: "stop", operationId: "stop-1" },
      stopDrainError: { code: "browser_audio_drain_incomplete", message: "Upload timed out", retryable: true },
    }));
    const rpc = vi.fn(async () => state({}) as never) as unknown as BrowserCaptureDependencies["rpc"];
    const { recorder, stream } = media();
    const page = owner(rpc, new BrowserChunkStore(() => new IDBFactory()), recorder, stream);
    await vi.waitFor(() => expect(page.owner.canFinishIncomplete).toBe(true));
    expect(page.owner.panel()).toMatchObject({ primaryAction: "finish_incomplete", primaryLabel: "Finish with what was saved" });
    expect(rpc).not.toHaveBeenCalled();
  });

  it("keeps recording memory-only when IndexedDB reports a quota error", async () => {
    sessionStorage.clear();
    const store = new BrowserChunkStore(() => new IDBFactory());
    await store.pending("warm-up");
    vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => {
      throw new DOMException("Quota exceeded", "QuotaExceededError");
    });
    const uploaded: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, options: RequestInit) => {
      uploaded.push(uploadedBody(options).text);
      return ok();
    }));
    const { recorder, stream } = media();
    const page = owner(async () => state({ nextSequence: 0 }) as never, store, recorder, stream);
    await page.owner.startFromProject("proj-1");
    recorder.ondataavailable?.({ data: new Blob(["chunk-0"]) } as BlobEvent);
    recorder.ondataavailable?.({ data: new Blob(["chunk-1"]) } as BlobEvent);
    await vi.waitFor(() => expect(uploaded).toEqual(["chunk-0", "chunk-1"]));
    await vi.waitFor(() => expect(store.isDegraded("rec-1")).toBe(true));
    expect(page.owner.active).toBe(true);
    expect(page.owner.panel()?.error ?? null).toBeNull();
    expect(JSON.parse(sessionStorage.getItem(CAPTURE_KEY)!)).toMatchObject({ nextSequence: 2, expectedNextSequence: 2 });
    page.uninstall();
  });

  it("deletes retained chunks when the recording finalizes", async () => {
    sessionStorage.clear();
    const store = new BrowserChunkStore(() => new IDBFactory());
    // A stale copy whose acknowledgement delete never ran.
    await store.persist("rec-1", 0, new Blob(["stale"]), { capturedStartUnixMs: 1, capturedEndUnixMs: 2 });
    vi.stubGlobal("fetch", vi.fn(async () => ok()));
    const rpc: BrowserCaptureDependencies["rpc"] = async (_plugin, method) => method === "stop"
      ? state({ state: "saved", error: null }) as never
      : state({ nextSequence: 1 }) as never;
    const { recorder, stream } = media();
    const page = owner(rpc, store, recorder, stream);
    await page.owner.startFromProject("proj-1");
    await expect(page.owner.stop()).resolves.toMatchObject({ state: "saved" });
    await vi.waitFor(async () => expect(await store.pending("rec-1")).toEqual([]));
  });

  function recoveringRpc(order: string[], expectedNextSequence: number): BrowserCaptureDependencies["rpc"] {
    return async (_plugin, method, input) => {
      order.push(method);
      if (method === "readCapture") return state({ nextSequence: 1 }) as never;
      if (method === "pause") {
        expect(input).toMatchObject({ expectedNextSequence, recoveredAfterReload: true });
        return state({ state: "paused", nextSequence: expectedNextSequence }) as never;
      }
      return state({ nextSequence: expectedNextSequence }) as never;
    };
  }

  it("drops a chunk the server refuses for good and keeps delivering the rest", async () => {
    const factory = new IDBFactory();
    await seed(new BrowserChunkStore(() => factory), "rec-1", [1, 2, 3]);
    sessionStorage.setItem(CAPTURE_KEY, JSON.stringify({ sessionId: "rec-1", startedAtMs: 1_000,
      nextSequence: 1, expectedNextSequence: 4, paused: false }));
    const order: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, options: RequestInit) => {
      const { sequence } = uploadedBody(options);
      order.push(`upload-${sequence}`);
      return sequence === 1 ? refused() : ok();
    }));
    const store = new BrowserChunkStore(() => factory);
    const { recorder, stream } = media();
    const page = owner(recoveringRpc(order, 4), store, recorder, stream);
    await vi.waitFor(() => expect(page.owner.active).toBe(true));
    // A refusal is not retried; the flush continues in order.
    expect(order).toEqual(["readCapture", "upload-1", "upload-2", "upload-3", "pause", "resume"]);
    expect(await store.sequences("rec-1")).toEqual([]);
    page.uninstall();
  });

  it("does not let entries left by a partial re-upload poison the next reload", async () => {
    const factory = new IDBFactory();
    await seed(new BrowserChunkStore(() => factory), "rec-1", [1, 2, 3]);
    sessionStorage.setItem(CAPTURE_KEY, JSON.stringify({ sessionId: "rec-1", startedAtMs: 1_000,
      nextSequence: 1, expectedNextSequence: 4, paused: false }));
    // Page 2: sequence 2 keeps failing transiently, so 2 and 3 stay retained
    // through the flush and the rotation declares them missing.
    const order: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, options: RequestInit) => {
      const { sequence } = uploadedBody(options);
      order.push(`upload-${sequence}`);
      if (sequence === 2) return unavailable();
      if (sequence === 4) return new Promise<Response>(() => {});
      return ok();
    }));
    const page2Store = new BrowserChunkStore(() => factory);
    const second = media();
    const page2 = owner(recoveringRpc(order, 4), page2Store, second.recorder, second.stream);
    await vi.waitFor(() => expect(page2.owner.active).toBe(true));
    expect(order).toEqual(["readCapture", "upload-1", "upload-2", "upload-2", "pause", "resume"]);
    await vi.waitFor(async () => expect(await page2Store.sequences("rec-1")).toEqual([]));
    // A new chunk is in flight when the page reloads again.
    second.recorder.ondataavailable?.({ data: new Blob(["chunk-4"]) } as BlobEvent);
    await vi.waitFor(async () => expect(await page2Store.sequences("rec-1")).toEqual([4]));
    page2.uninstall();

    // Page 3 re-uploads only the chunk that can still land.
    const order3: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, options: RequestInit) => {
      order3.push(`upload-${uploadedBody(options).sequence}`);
      return ok();
    }));
    const third = media();
    const page3Store = new BrowserChunkStore(() => factory);
    const page3 = owner(recoveringRpc(order3, 5), page3Store, third.recorder, third.stream);
    await vi.waitFor(() => expect(page3.owner.active).toBe(true));
    expect(order3).toEqual(["readCapture", "upload-4", "pause", "resume"]);
    expect(await page3Store.sequences("rec-1")).toEqual([]);
    page3.uninstall();
  });

  it("re-uploads retained chunks even when the microphone is denied after reload", async () => {
    const factory = new IDBFactory();
    await seed(new BrowserChunkStore(() => factory), "rec-1", [1]);
    sessionStorage.setItem(CAPTURE_KEY, JSON.stringify({ sessionId: "rec-1", startedAtMs: 1_000,
      nextSequence: 1, expectedNextSequence: 2, paused: false }));
    const uploads: number[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, options: RequestInit) => {
      uploads.push(uploadedBody(options).sequence);
      return ok();
    }));
    const store = new BrowserChunkStore(() => factory);
    const { recorder, stream } = media();
    const denied = vi.fn(async (): Promise<MediaStream> => { throw new DOMException("denied", "NotAllowedError"); });
    owner(recoveringRpc([], 2), store, recorder, stream, denied);
    await vi.waitFor(() => expect(denied).toHaveBeenCalled());
    expect(uploads).toEqual([1]);
    expect(await store.sequences("rec-1")).toEqual([]);
  });

  function pendingStop(extra: object = {}) {
    sessionStorage.setItem(CAPTURE_KEY, JSON.stringify({
      sessionId: "rec-1", startedAtMs: 1_000, nextSequence: 3, expectedNextSequence: 4, paused: false,
      pendingControl: { kind: "stop", operationId: "stop-1" },
      stopDrainError: { code: "browser_audio_drain_incomplete", message: "Upload timed out", retryable: true },
      ...extra,
    }));
  }

  it("offers 'Finish with what was saved' when the persisted chunk is refused", async () => {
    const factory = new IDBFactory();
    await seed(new BrowserChunkStore(() => factory), "rec-1", [3]);
    pendingStop();
    vi.stubGlobal("fetch", vi.fn(async () => refused()));
    const rpc = vi.fn(async () => state({}) as never) as unknown as BrowserCaptureDependencies["rpc"];
    const store = new BrowserChunkStore(() => factory);
    const { recorder, stream } = media();
    const page = owner(rpc, store, recorder, stream);
    await vi.waitFor(() => expect(page.owner.canFinishIncomplete).toBe(true));
    expect(page.owner.panel()).toMatchObject({ primaryAction: "finish_incomplete", primaryLabel: "Finish with what was saved" });
    expect(await store.sequences("rec-1")).toEqual([]);
    expect(rpc).not.toHaveBeenCalled();
  });

  it("bounds Stop retries while persisted audio cannot be uploaded, then offers Finish", async () => {
    const factory = new IDBFactory();
    await seed(new BrowserChunkStore(() => factory), "rec-1", [3]);
    pendingStop();
    vi.stubGlobal("fetch", vi.fn(async () => unavailable()));
    const rpc = vi.fn(async () => state({}) as never) as unknown as BrowserCaptureDependencies["rpc"];
    const store = new BrowserChunkStore(() => factory);
    const { recorder, stream } = media();
    const page = owner(rpc, store, recorder, stream);
    await vi.waitFor(() => expect(page.owner.panel()).toMatchObject({ primaryAction: "retry" }));
    expect(page.owner.canFinishIncomplete).toBe(false);
    await page.owner.retryPendingStop();
    expect(page.owner.panel()).toMatchObject({ primaryAction: "retry" });
    await page.owner.retryPendingStop();
    expect(page.owner.canFinishIncomplete).toBe(true);
    expect(page.owner.panel()).toMatchObject({ primaryAction: "finish_incomplete" });
    expect(rpc).not.toHaveBeenCalled();
  });

  it("clears retained audio when a meeting is discarded and sweeps abandoned audio on page load", async () => {
    const factory = new IDBFactory();
    let now = 1_000;
    const store = new BrowserChunkStore(() => factory, { maxAgeMs: 10_000, now: () => now });
    await seed(store, "abandoned", [0]);
    now = 20_000;
    await seed(store, "discarded", [0, 1]);
    await seed(store, "fresh", [0]);
    const { recorder, stream } = media();
    const page = owner(async () => state({}) as never, store, recorder, stream);
    page.owner.discardRetainedAudio("discarded");
    await vi.waitFor(async () => expect(await store.sequences("discarded")).toEqual([]));
    expect(await store.sequences("abandoned")).toEqual([]);
    expect(await store.sequences("fresh")).toEqual([0]);
  });
});
