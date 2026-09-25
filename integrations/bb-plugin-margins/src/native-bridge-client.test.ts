// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { NativeBridgeOwner } from "./native-bridge-client.js";

const ready = {
  state: "ready", instanceId: "linux-one", workspaceId: "practice", sessionId: null, transferId: null,
  microphoneSamples: 0, systemSamples: 0, microphoneDroppedSamples: 0,
  systemDroppedSamples: 0, systemFrames: 0, systemSilentSamples: 0, error: null,
};

afterEach(() => { vi.unstubAllGlobals(); sessionStorage.clear(); });

describe("Mac PWA bridge destination", () => {
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
});
