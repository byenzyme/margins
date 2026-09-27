// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { NativeBridgeOwner, type NativeStatus } from "./native-bridge-client.js";

const ready = {
  state: "ready", instanceId: "linux-one", workspaceId: "practice", sessionId: null, transferId: null,
  microphoneSamples: 0, systemSamples: 0, microphoneDroppedSamples: 0,
  systemDroppedSamples: 0, systemFrames: 0, systemSilentSamples: 0, error: null,
};

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); sessionStorage.clear(); });

describe("Mac PWA bridge destination", () => {
  it("warns after three seconds of silent Mac input and clears when speech arrives", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000);
    let status: NativeStatus = { ...ready, state: "recording", sessionId: "meeting-1", micPeak: 0 };
    vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit) => new Response(JSON.stringify(
      init.method === "GET" ? status : { token: "local-secret", instanceId: "linux-one", workspaceId: "practice", status: ready },
    ), { status: 200 })));
    const owner = new NativeBridgeOwner();
    await owner.pair("one-time-code", { instanceId: "linux-one", workspaceId: "practice" });
    await owner.refresh();
    expect(owner.noAudioWarning).toBe(false);
    await vi.advanceTimersByTimeAsync(3_100);
    expect(owner.noAudioWarning).toBe(true);
    status = { ...status, micPeak: 0.3 };
    await owner.refresh();
    expect(owner.noAudioWarning).toBe(false);
    status = { ...ready, state: "ready", sessionId: null, micPeak: 0 };
    await owner.refresh();
    owner.forget();
  });
  it("rejects a paired bridge pointed at another Workspace before storing its token", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({
      token: "local-secret", instanceId: "linux-one", workspaceId: "other", status: { ...ready, workspaceId: "other" },
    }), { status: 200 })));
    const owner = new NativeBridgeOwner();
    await expect(owner.pair("one-time-code", { instanceId: "linux-one", workspaceId: "practice" })).rejects.toThrow("Configure both for the same Margins destination");
    expect(owner.paired).toBe(false);
    expect(sessionStorage.length).toBe(0);
  });

  it("checks the live destination again before starting capture", async () => {
    const fetchMock = vi.fn(async (_url: string, init: RequestInit) => {
      const status = init.method === "GET" ? { ...ready, instanceId: "linux-two" } : ready;
      return new Response(JSON.stringify(init.method === "GET" ? status : {
        token: "local-secret", instanceId: "linux-one", workspaceId: "practice", status,
      }), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);
    const owner = new NativeBridgeOwner();
    await owner.pair("one-time-code", { instanceId: "linux-one", workspaceId: "practice" });
    expect(sessionStorage.getItem("margins.bb.native-bridge.v1")).not.toContain("threadId");
    await expect(owner.verify({ instanceId: "linux-one", workspaceId: "practice" })).rejects.toThrow("destination changed");
    expect(fetchMock.mock.calls.some(([url]) => String(url).endsWith("/v1/start"))).toBe(false);
    owner.forget();
  });

  it("treats an accepted asynchronous Start as accepted if the next status fetch fails", async () => {
    const fetchMock = vi.fn(async (url: string, init: RequestInit) => {
      if (String(url).endsWith("/v1/status")) throw new TypeError("Failed to fetch");
      return new Response(JSON.stringify(String(url).endsWith("/v1/start") ? {} : {
        token: "local-secret", instanceId: "linux-one", workspaceId: "practice", status: ready,
      }), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);
    const owner = new NativeBridgeOwner();
    await owner.pair("one-time-code", { instanceId: "linux-one", workspaceId: "practice" });
    await expect(owner.control("start")).resolves.toBeUndefined();
    expect(fetchMock.mock.calls.filter(([url]) => String(url).endsWith("/v1/start"))).toHaveLength(1);
    owner.forget();
  });
});
