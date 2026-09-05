import { describe, expect, it, vi } from "vitest";
import { desktopSnapshot } from "./fixtures.js";
import { createHttpMarginsLiveTransport } from "./live-transport.js";

function jsonResponse(body: unknown, init: ResponseInit = {}) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
    ...init,
  });
}

describe("Margins live HTTP transport", () => {
  it("starts the installed compatibility runtime and waits for its private connection file", async () => {
    const startRuntime = vi.fn(async () => "started" as const);
    const fetchImpl = vi.fn(async () => jsonResponse(desktopSnapshot("idle")));
    const transport = createHttpMarginsLiveTransport({
      env: {},
      fetchImpl: fetchImpl as unknown as typeof fetch,
      readFile: vi.fn(async () =>
        JSON.stringify({
          protocol_version: 1,
          base_url: "http://127.0.0.1:49152",
          token: "secret-token",
          endpoints: {
            snapshot: "/v1/live/snapshot",
            start: "/v1/live/start",
            pause: "/v1/live/pause",
            resume: "/v1/live/resume",
            stop: "/v1/live/stop",
            update_notepad: "/v1/live/notepad",
          },
        }),
      ),
      startRuntime,
      homeDir: "/Users/me",
      platform: "darwin",
    });

    await expect(transport.ensureRuntime({ dataDir: "/tmp/margins-plugin" })).resolves.toMatchObject({
      ok: true,
      snapshot: { protocol_version: 1, session: null },
    });
    expect(startRuntime).toHaveBeenCalledOnce();
    expect(String((fetchImpl.mock.calls as unknown as Array<[URL]>)[0]?.[0])).toBe(
      "http://127.0.0.1:49152/v1/live/snapshot",
    );
  });

  it("waits past a stale connection file while Margins starts", async () => {
    const startRuntime = vi.fn(async () => "started" as const);
    const fetchImpl = vi
      .fn()
      .mockRejectedValueOnce(new Error("fetch failed: ECONNREFUSED"))
      .mockResolvedValueOnce(jsonResponse(desktopSnapshot("idle")));
    const transport = createHttpMarginsLiveTransport({
      env: {},
      fetchImpl: fetchImpl as unknown as typeof fetch,
      readFile: vi.fn(async () =>
        JSON.stringify({
          protocol_version: 1,
          base_url: "http://127.0.0.1:49152",
          token: "secret-token",
          endpoints: {
            snapshot: "/v1/live/snapshot",
            start: "/v1/live/start",
            pause: "/v1/live/pause",
            resume: "/v1/live/resume",
            stop: "/v1/live/stop",
            update_notepad: "/v1/live/notepad",
          },
        }),
      ),
      startRuntime,
      homeDir: "/Users/me",
      platform: "darwin",
    });

    await expect(transport.ensureRuntime({ dataDir: "/tmp/margins-plugin" })).resolves.toMatchObject({
      ok: true,
      snapshot: { protocol_version: 1, session: null },
    });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it("discovers the native desktop live endpoint and sends exact V1 request names", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse({
        protocol_version: 1,
        idempotent_replay: false,
        snapshot: desktopSnapshot("recording"),
      }),
    );
    const readFile = vi.fn(async () =>
      JSON.stringify({
        protocol_version: 1,
        base_url: "http://127.0.0.1:49152",
        token: "secret-token",
        endpoints: {
          snapshot: "/v1/live/snapshot",
          start: "/v1/live/start",
          pause: "/v1/live/pause",
          resume: "/v1/live/resume",
          stop: "/v1/live/stop",
          update_notepad: "/v1/live/notepad",
        },
      }),
    );
    const transport = createHttpMarginsLiveTransport({
      env: {},
      fetchImpl: fetchImpl as unknown as typeof fetch,
      readFile,
      homeDir: "/Users/me",
      platform: "darwin",
    });

    await expect(
      transport.start({
        operationId: "op-start",
        name: "Customer call",
        projectId: "proj_123",
      }),
    ).resolves.toMatchObject({
      ok: true,
      snapshot: { protocol_version: 1 },
    });

    expect(readFile).toHaveBeenCalledWith(
      "/Users/me/Library/Application Support/margins/desktop-live.v1.json",
      "utf8",
    );
    expect(fetchImpl).toHaveBeenCalledWith(
      new URL("http://127.0.0.1:49152/v1/live/start"),
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer secret-token",
        }),
        body: JSON.stringify({
          operation_id: "op-start",
          name: "Customer call",
          project_id: "proj_123",
        }),
      }),
    );

    await transport.updateNotepad({
      operationId: "op-notepad",
      sessionId: "customer-call",
      expectedGeneration: 2,
      expectedNotepadRevision: "v1-fixture",
      text: "One\nTwo",
    });
    expect(fetchImpl).toHaveBeenLastCalledWith(
      new URL("http://127.0.0.1:49152/v1/live/notepad"),
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          operation_id: "op-notepad",
          session_id: "customer-call",
          expected_generation: 2,
          expected_notepad_revision: "v1-fixture",
          text: "One\nTwo",
        }),
      }),
    );
  });

  it("uses session_id query only for non-current snapshots", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse(desktopSnapshot("paused")));
    const transport = createHttpMarginsLiveTransport({
      env: {
        MARGINS_LIVE_API_URL: "http://127.0.0.1:49152",
        MARGINS_LIVE_API_TOKEN: "secret-token",
      },
      fetchImpl: fetchImpl as unknown as typeof fetch,
      readFile: vi.fn(async () => {
        throw new Error("no discovery");
      }),
    });

    await transport.readSnapshot({ sessionId: "customer-call" });
    const calls = fetchImpl.mock.calls as unknown as Array<[URL, RequestInit]>;
    expect(String(calls[0]?.[0])).toBe(
      "http://127.0.0.1:49152/v1/live/snapshot?session_id=customer-call",
    );
  });

  it("maps no active session to the ready panel state", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(
        {
          code: "no_active_session",
          message: "No active Margins recording.",
          retryable: false,
        },
        { status: 404 },
      ),
    );
    const transport = createHttpMarginsLiveTransport({
      env: {
        MARGINS_LIVE_API_URL: "http://127.0.0.1:49152",
        MARGINS_LIVE_API_TOKEN: "secret-token",
      },
      fetchImpl: fetchImpl as unknown as typeof fetch,
      readFile: vi.fn(async () => {
        throw new Error("no discovery");
      }),
    });

    await expect(transport.readSnapshot({ sessionId: "current" })).resolves.toEqual({
      ok: false,
      error: {
        code: "no_active_session",
        message: "No active Margins recording.",
        retryable: false,
        state: "ready",
      },
    });
  });

  it("rejects a successful response that does not match the Rust live contract", async () => {
    const transport = createHttpMarginsLiveTransport({
      env: {
        MARGINS_LIVE_API_URL: "http://127.0.0.1:49152",
        MARGINS_LIVE_API_TOKEN: "secret-token",
      },
      fetchImpl: vi.fn(async () => jsonResponse({ protocol_version: 2 })) as unknown as typeof fetch,
      readFile: vi.fn(async () => {
        throw new Error("no discovery");
      }),
    });

    await expect(transport.readSnapshot({ sessionId: "current" })).resolves.toMatchObject({
      ok: false,
      error: {
        code: "contract_mismatch",
        state: "runtime_error",
      },
    });
  });
});
