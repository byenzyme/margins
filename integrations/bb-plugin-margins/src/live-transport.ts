import { readFile as defaultReadFile } from "node:fs/promises";
import { homedir, platform } from "node:os";
import { join } from "node:path";
import {
  liveSnapshotSchema,
  type HostOperationResult,
  type LiveError,
} from "./contracts.js";
import { createRuntimeManager, type RuntimeStartResult } from "./runtime-manager.js";

export interface MarginsLiveTransport {
  readSnapshot(input: {
    endpoint?: string;
    sessionId: string;
    signal?: AbortSignal;
  }): Promise<HostOperationResult>;
  ensureRuntime(input: { dataDir: string; signal?: AbortSignal }): Promise<HostOperationResult>;
  start(input: {
    operationId: string;
    name: string;
    projectId?: string;
    signal?: AbortSignal;
  }): Promise<HostOperationResult>;
  pause(input: SessionOperationInput): Promise<HostOperationResult>;
  resume(input: SessionOperationInput): Promise<HostOperationResult>;
  stop(input: SessionOperationInput): Promise<HostOperationResult>;
  updateNotepad(
    input: SessionOperationInput & { expectedNotepadRevision: string; text: string },
  ): Promise<HostOperationResult>;
}

export interface SessionOperationInput {
  operationId: string;
  sessionId: string;
  expectedGeneration: number | null;
  signal?: AbortSignal;
}

interface HttpTransportOptions {
  env?: NodeJS.ProcessEnv;
  fetchImpl?: typeof fetch;
  readFile?: (path: string, encoding: BufferEncoding) => Promise<string>;
  homeDir?: string;
  platform?: NodeJS.Platform;
  startRuntime?: (input: { dataDir: string; signal?: AbortSignal }) => Promise<RuntimeStartResult>;
}

interface DesktopLiveDiscoveryV1 {
  protocol_version: 1;
  base_url: string;
  token: string;
  endpoints: {
    snapshot: string;
    start: string;
    pause: string;
    resume: string;
    stop: string;
    update_notepad: string;
  };
}

const DISCOVERY_FILENAME = "desktop-live.v1.json";
const FALLBACK_ENDPOINTS = {
  snapshot: "/v1/live/snapshot",
  start: "/v1/live/start",
  pause: "/v1/live/pause",
  resume: "/v1/live/resume",
  stop: "/v1/live/stop",
  update_notepad: "/v1/live/notepad",
};
const INSTALL_URL = "https://github.com/byenzyme/margins/releases";

function profileSlug(env: NodeJS.ProcessEnv) {
  const profile = env.MARGINS_PROFILE?.trim() || "default";
  if (profile === "default") return "margins";
  const slug =
    profile
      .split("")
      .map((char) => (/[a-zA-Z0-9_-]/.test(char) ? char.toLowerCase() : "-"))
      .join("")
      .replace(/^-+|-+$/g, "") || "profile";
  return `margins-${slug}`;
}

function defaultDiscoveryPath(env: NodeJS.ProcessEnv, hostPlatform: NodeJS.Platform, home: string) {
  const slug = profileSlug(env);
  if (hostPlatform === "darwin") {
    return join(home, "Library", "Application Support", slug, DISCOVERY_FILENAME);
  }
  const dataHome = env.XDG_DATA_HOME?.trim() || join(home, ".local", "share");
  return join(dataHome, slug, DISCOVERY_FILENAME);
}

function resultError(
  code: string,
  message: string,
  retryable: boolean,
  extras: Partial<LiveError> = {},
): HostOperationResult {
  return {
    ok: false,
    error: {
      code,
      message,
      retryable,
      ...extras,
    },
  };
}

async function readDiscovery(
  env: NodeJS.ProcessEnv,
  readFile: (path: string, encoding: BufferEncoding) => Promise<string>,
  hostPlatform: NodeJS.Platform,
  home: string,
): Promise<DesktopLiveDiscoveryV1 | null> {
  const discoveryPath =
    env.MARGINS_LIVE_DISCOVERY_FILE?.trim() || defaultDiscoveryPath(env, hostPlatform, home);
  try {
    const parsed = JSON.parse(
      await readFile(discoveryPath, "utf8"),
    ) as Partial<DesktopLiveDiscoveryV1>;
    if (
      parsed.protocol_version === 1 &&
      typeof parsed.base_url === "string" &&
      typeof parsed.token === "string" &&
      parsed.endpoints &&
      typeof parsed.endpoints.snapshot === "string"
    ) {
      return {
        protocol_version: 1,
        base_url: parsed.base_url,
        token: parsed.token,
        endpoints: {
          ...FALLBACK_ENDPOINTS,
          ...parsed.endpoints,
        },
      };
    }
  } catch {
    return null;
  }
  return null;
}

async function resolveConnection(
  options: Required<Pick<HttpTransportOptions, "env" | "readFile" | "homeDir" | "platform">>,
) {
  const discovery = await readDiscovery(
    options.env,
    options.readFile,
    options.platform,
    options.homeDir,
  );
  const directUrl = options.env.MARGINS_LIVE_API_URL?.trim();
  const directToken = options.env.MARGINS_LIVE_API_TOKEN?.trim();
  const useDirect = Boolean(directUrl && directToken);
  const baseUrl = (useDirect ? directUrl! : discovery?.base_url || "").replace(/\/+$/, "");
  const token = useDirect ? directToken! : discovery?.token || "";
  if (!baseUrl || !token) return null;
  return {
    baseUrl,
    token,
    endpoints: discovery?.endpoints ?? FALLBACK_ENDPOINTS,
  };
}

function delay(ms: number, signal?: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason);
      return;
    }
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal?.reason);
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

function normalizeError(raw: unknown, fallbackMessage: string): LiveError {
  if (typeof raw !== "object" || raw === null) {
    return {
      code: "request_failed",
      message: fallbackMessage,
      retryable: true,
    };
  }
  const value = raw as Record<string, unknown>;
  return {
    code: typeof value.code === "string" ? value.code : "request_failed",
    message: typeof value.message === "string" ? value.message : fallbackMessage,
    retryable: typeof value.retryable === "boolean" ? value.retryable : true,
  };
}

function mapNetworkError(cause: unknown): HostOperationResult {
  const message = cause instanceof Error ? cause.message : String(cause);
  if (
    message.includes("ECONNREFUSED") ||
    message.includes("fetch failed") ||
    message.includes("ECONNRESET")
  ) {
    return resultError("margins_closed", "Margins is not running on this Mac.", true, {
      state: "runtime_error",
      installUrl: INSTALL_URL,
    });
  }
  return resultError("request_failed", "Something interrupted recording.", true, {
    state: "recoverable_error",
  });
}

function mapStatusError(status: number, raw: unknown): HostOperationResult {
  if (status === 401 || status === 403) {
    return resultError("unauthorized", "Restart Margins to renew its private connection.", false, {
      state: "runtime_auth_error",
      installUrl: INSTALL_URL,
    });
  }
  if (status === 404) {
    const parsed = normalizeError(raw, "Margins is not recording right now.");
    if (parsed.code === "no_active_session") {
      return {
        ok: false,
        error: { ...parsed, state: "ready" },
      };
    }
    return {
      ok: false,
      error: {
        ...parsed,
        state: "runtime_error",
        installUrl: INSTALL_URL,
      },
    };
  }
  if (status === 426) {
    return resultError("update_needed", "Update Margins to use this panel.", false, {
      state: "runtime_error",
      installUrl: INSTALL_URL,
    });
  }
  const parsed = normalizeError(raw, "Something interrupted recording.");
  return { ok: false, error: parsed };
}

function mutationResult(raw: unknown): HostOperationResult {
  if (typeof raw === "object" && raw !== null && "snapshot" in raw) {
    const response = raw as {
      snapshot: unknown;
      idempotent_replay?: unknown;
      stopped_session_id?: unknown;
    };
    const parsed = liveSnapshotSchema.safeParse(response.snapshot);
    if (!parsed.success) {
      return resultError("contract_mismatch", "Margins returned an unfamiliar live state.", false, {
        state: "runtime_error",
        installUrl: INSTALL_URL,
      });
    }
    return {
      ok: true,
      snapshot: parsed.data,
      idempotent_replay:
        typeof response.idempotent_replay === "boolean" ? response.idempotent_replay : undefined,
      stopped_session_id:
        typeof response.stopped_session_id === "string"
          ? response.stopped_session_id
          : response.stopped_session_id === null
            ? null
            : undefined,
    };
  }
  const parsed = liveSnapshotSchema.safeParse(raw);
  if (!parsed.success) {
    return resultError("contract_mismatch", "Margins returned an unfamiliar live state.", false, {
      state: "runtime_error",
      installUrl: INSTALL_URL,
    });
  }
  return {
    ok: true,
    snapshot: parsed.data,
  };
}

export function createHttpMarginsLiveTransport(
  options: HttpTransportOptions = {},
): MarginsLiveTransport {
  const resolvedOptions = {
    env: options.env ?? process.env,
    fetchImpl: options.fetchImpl ?? fetch,
    readFile: options.readFile ?? defaultReadFile,
    homeDir: options.homeDir ?? homedir(),
    platform: options.platform ?? platform(),
    startRuntime:
      options.startRuntime ??
      createRuntimeManager({
        env: options.env,
        fetchImpl: options.fetchImpl,
        homeDir: options.homeDir,
        platform: options.platform,
      }).ensure,
  };

  async function call(
    method: "GET" | "POST",
    input: { signal?: AbortSignal },
    endpointName: keyof DesktopLiveDiscoveryV1["endpoints"],
    body?: unknown,
    query?: URLSearchParams,
  ): Promise<HostOperationResult> {
    const connection = await resolveConnection(resolvedOptions);
    if (!connection) {
      return resultError("margins_closed", "Start Margins on the selected Mac.", true, {
        state: "runtime_error",
        installUrl: INSTALL_URL,
      });
    }
    const endpoint = connection.endpoints[endpointName] ?? FALLBACK_ENDPOINTS[endpointName];
    const url = new URL(`${connection.baseUrl}${endpoint}`);
    query?.forEach((value, key) => url.searchParams.set(key, value));
    try {
      const response = await resolvedOptions.fetchImpl(url, {
        method,
        signal: input.signal,
        headers: {
          accept: "application/json",
          authorization: `Bearer ${connection.token}`,
          ...(body === undefined ? {} : { "content-type": "application/json" }),
        },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
      const raw = (await response.json().catch(() => null)) as unknown;
      if (!response.ok) return mapStatusError(response.status, raw);
      return mutationResult(raw);
    } catch (cause) {
      if (input.signal?.aborted) throw cause;
      return mapNetworkError(cause);
    }
  }

  return {
    readSnapshot(input) {
      const query = new URLSearchParams();
      if (input.sessionId !== "current") query.set("session_id", input.sessionId);
      return call("GET", input, "snapshot", undefined, query);
    },
    async ensureRuntime(input) {
      let started: RuntimeStartResult;
      try {
        started = await resolvedOptions.startRuntime(input);
      } catch (cause) {
        if (input.signal?.aborted) throw cause;
        return mapNetworkError(cause);
      }
      if (started !== "started") {
        const updateNeeded = started === "update_needed";
        const unsupported = started === "unsupported";
        return resultError(unsupported ? "unsupported" : updateNeeded ? "update_needed" : "margins_not_found", unsupported
          ? "Margins recording needs an Apple silicon Mac."
          : updateNeeded
            ? "The bb-ready Margins runtime has not been released yet."
            : "Margins recording is not available on this machine.", false, {
          state: unsupported ? "unsupported_platform" : "runtime_error",
          installUrl: INSTALL_URL,
        });
      }
      for (let attempt = 0; attempt < 20; attempt += 1) {
        if (await resolveConnection(resolvedOptions)) {
          const result = await call("GET", input, "snapshot");
          if (result.ok || result.error.state !== "runtime_error") {
            return result;
          }
        }
        await delay(250, input.signal);
      }
      return resultError("margins_closed", "Margins is starting. Try again in a moment.", true, {
        state: "runtime_error",
      });
    },
    start(input) {
      return call("POST", input, "start", {
        operation_id: input.operationId,
        name: input.name,
        ...(input.projectId ? { project_id: input.projectId } : {}),
      });
    },
    pause(input) {
      return call("POST", input, "pause", {
        operation_id: input.operationId,
        session_id: input.sessionId,
        ...(input.expectedGeneration === null
          ? {}
          : { expected_generation: input.expectedGeneration }),
      });
    },
    resume(input) {
      return call("POST", input, "resume", {
        operation_id: input.operationId,
        session_id: input.sessionId,
        ...(input.expectedGeneration === null
          ? {}
          : { expected_generation: input.expectedGeneration }),
      });
    },
    stop(input) {
      return call("POST", input, "stop", {
        operation_id: input.operationId,
        session_id: input.sessionId,
        ...(input.expectedGeneration === null
          ? {}
          : { expected_generation: input.expectedGeneration }),
      });
    },
    updateNotepad(input) {
      return call("POST", input, "update_notepad", {
        operation_id: input.operationId,
        session_id: input.sessionId,
        ...(input.expectedGeneration === null
          ? {}
          : { expected_generation: input.expectedGeneration }),
        expected_notepad_revision: input.expectedNotepadRevision,
        text: input.text,
      });
    },
  };
}
