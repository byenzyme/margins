import { randomUUID } from "node:crypto";
import { execFile as execFileCallback, spawn, type ChildProcess } from "node:child_process";
import { access, lstat, mkdir, readFile, realpath } from "node:fs/promises";
import { createServer } from "node:net";
import { homedir } from "node:os";
import { isAbsolute, join, relative, resolve } from "node:path";
import { promisify } from "node:util";
import type { ConnectedNoteResult, HostCaptureSnapshot, HostError, HostResult, ProjectTarget, TranscriptionRequestResult } from "./contracts.js";
import { createRuntimeManager } from "./runtime-manager.js";

const execFile = promisify(execFileCallback);

interface ServerHandle {
  baseUrl: string;
  token: string;
  workspaceId: string;
  instanceId: string;
  child?: ChildProcess;
}

interface AsrRuntimeConfig {
  serverPath: string;
  modelDir: string;
  ortLibraryPath: string;
}

export async function readAsrRuntimeConfig(dataDir: string): Promise<AsrRuntimeConfig | null> {
  const path = join(dataDir, "asr-runtime.json");
  const raw = await readFile(path, "utf8").catch((error: NodeJS.ErrnoException) => {
    if (error.code === "ENOENT") return null;
    throw error;
  });
  if (raw === null) return null;
  let value: unknown;
  try { value = JSON.parse(raw); }
  catch { throw new Error("Invalid Margins ASR runtime configuration"); }
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Invalid Margins ASR runtime configuration");
  }
  const config = value as Record<string, unknown>;
  for (const key of ["serverPath", "modelDir", "ortLibraryPath"] as const) {
    if (typeof config[key] !== "string" || !isAbsolute(config[key])) {
      throw new Error(`Margins ASR runtime ${key} must be an absolute path`);
    }
  }
  const selected = config as unknown as AsrRuntimeConfig;
  const [server, model, ort] = await Promise.all([
    lstat(selected.serverPath), lstat(selected.modelDir), lstat(selected.ortLibraryPath),
  ]);
  if (!server.isFile() || (server.mode & 0o111) === 0 || !model.isDirectory() || !ort.isFile()) {
    throw new Error("Margins ASR runtime files are not ready");
  }
  return selected;
}

function hostError(code: string, message: string, retryable = true): HostError {
  return { code, message, retryable };
}

export function workspaceInstanceDir(dataDir: string, workspaceId: string) {
  return join(dataDir, "workspace-servers", workspaceId);
}

export function marginsCli(): string {
  const configured = process.env.MARGINS_CLI_BIN?.trim();
  if (configured && isAbsolute(configured)) return configured;
  const installed = join(homedir(), ".local", "bin", "margins");
  return installed;
}

async function localHomeRoot(workspaceId: string): Promise<string | null> {
  const binary = marginsCli();
  const executable = await lstat(binary).catch(() => null);
  if (!executable?.isFile() || (executable.mode & 0o111) === 0) return null;
  try {
    const { stdout } = await execFile(binary, ["--workspace", workspaceId, "workspace", "destination", "--json"], {
      env: { ...process.env, MARGINS_HOME: marginsHome() }, timeout: 5_000, maxBuffer: 65_536,
    });
    const destination = JSON.parse(stdout) as { home_root?: unknown };
    return typeof destination.home_root === "string" && isAbsolute(destination.home_root)
      ? await realpath(destination.home_root) : null;
  } catch { return null; }
}

async function associatedNoteFile(homeRoot: string | null, relativePath: string | undefined): Promise<string | null> {
  if (!homeRoot || !relativePath || isAbsolute(relativePath)) return null;
  try {
    const file = await realpath(resolve(homeRoot, relativePath));
    const within = relative(homeRoot, file);
    return within && within !== ".." && !within.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) && !isAbsolute(within)
      && (await lstat(file)).isFile() ? file : null;
  } catch { return null; }
}

export function marginsHome(): string {
  return process.env.MARGINS_HOME?.trim() || join(homedir(), ".margins");
}

export function pendingWorkspaceSetupMarker(workspaceId: string): string {
  return join(marginsHome(), "pending-workspace-setup", workspaceId);
}

export async function workspaceOptions(): Promise<{ defaultWorkspaceId: string | null; workspaces: Array<{ id: string; name: string | null }>; autoSelected: boolean }> {
  const binary = marginsCli();
  const env = { ...process.env, MARGINS_HOME: marginsHome() };
  const { stdout } = await execFile(binary, ["workspace", "list", "--json"], { env, timeout: 5_000, maxBuffer: 65_536 });
  const listing = JSON.parse(stdout) as { default_workspace?: unknown; workspaces?: unknown };
  if (!Array.isArray(listing.workspaces) || !listing.workspaces.every((item) => item && typeof item === "object"
    && typeof item.id === "string" && (item.name === null || typeof item.name === "string"))) {
    throw new Error("Margins Workspace list is unavailable.");
  }
  const listed = listing.workspaces as Array<{ id: string; name: string | null }>;
  const pending = await Promise.all(listed.map(async (item) =>
    await access(pendingWorkspaceSetupMarker(item.id)).then(() => true, () => false)));
  const workspaces = listed.filter((_item, index) => !pending[index]);
  let defaultWorkspaceId = typeof listing.default_workspace === "string" ? listing.default_workspace : null;
  if (defaultWorkspaceId && !workspaces.some((item) => item.id === defaultWorkspaceId)) defaultWorkspaceId = null;
  let autoSelected = false;
  if (!defaultWorkspaceId && workspaces.length === 1) {
    const selected = workspaces[0]!.id;
    await execFile(binary, ["workspace", "default", "--set", selected, "--json"], { env, timeout: 5_000, maxBuffer: 65_536 });
    defaultWorkspaceId = selected;
    autoSelected = true;
  }
  return { defaultWorkspaceId, workspaces, autoSelected };
}

export async function workspacePaths(workspaceId: string): Promise<{ notes: string; recordings: string }> {
  if (!/^[a-z0-9][a-z0-9-]*$/.test(workspaceId)) throw new Error("Invalid Margins Workspace id");
  const binary = marginsCli();
  const env = { ...process.env, MARGINS_HOME: marginsHome() };
  const [destinationResult, sourcesResult] = await Promise.all([
    execFile(binary, ["--workspace", workspaceId, "workspace", "destination", "--json"], { env, timeout: 5_000, maxBuffer: 65_536 }),
    execFile(binary, ["--workspace", workspaceId, "source", "list", "--json"], { env, timeout: 5_000, maxBuffer: 65_536 }),
  ]);
  const destination = JSON.parse(destinationResult.stdout) as { destination?: unknown };
  const sources = JSON.parse(sourcesResult.stdout) as unknown;
  const capture = Array.isArray(sources) ? sources.find((item) => item && typeof item === "object" && item.kind === "captures") : null;
  if (typeof destination.destination !== "string" || !isAbsolute(destination.destination)
    || !capture || typeof capture.path !== "string" || !isAbsolute(capture.path)) {
    throw new Error("Margins Workspace destinations are unavailable.");
  }
  return { notes: destination.destination, recordings: capture.path };
}

export async function resolveWorkspaceId(target: ProjectTarget): Promise<string> {
  if (target.workspaceId) {
    if (!/^[a-z0-9][a-z0-9-]*$/.test(target.workspaceId)) throw new Error("Invalid Margins Workspace id");
    return target.workspaceId;
  }
  const options = await workspaceOptions();
  if (!options.defaultWorkspaceId) throw new Error(options.workspaces.length
    ? "Choose a Margins Workspace from the picker."
    : "Set up a Margins Workspace to store meetings and notes.");
  return options.defaultWorkspaceId;
}

async function availablePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") {
        server.close();
        reject(new Error("Could not reserve a local recording port"));
        return;
      }
      server.close((error) => error ? reject(error) : resolve(address.port));
    });
  });
}

async function waitForServer(baseUrl: string, tokenPath: string, child: ChildProcess, signal?: AbortSignal) {
  const deadline = Date.now() + 12_000;
  while (Date.now() < deadline) {
    if (signal?.aborted) throw signal.reason;
    if (child.exitCode !== null) throw new Error("Margins stopped while getting recording ready");
    const token = await readFile(tokenPath, "utf8").catch(() => "");
    if (token.trim()) {
      const healthy = await fetch(`${baseUrl}/health`, { signal }).then((response) => response.ok).catch(() => false);
      if (healthy) return token.trim();
    }
    await new Promise((resolve) => setTimeout(resolve, 80));
  }
  throw new Error("Margins did not become ready on the project machine");
}

export class ProjectServerManager {
  private readonly handles = new Map<string, Promise<ServerHandle>>();
  private readonly runtime = createRuntimeManager();

  async ensure(target: ProjectTarget, dataDir: string, signal?: AbortSignal): Promise<ServerHandle> {
    const workspaceId = await resolveWorkspaceId(target);
    const key = workspaceId;
    const existing = this.handles.get(key);
    if (existing) return existing;
    const pending = this.start(target, dataDir, key, workspaceId, signal).catch((error) => {
      this.handles.delete(key);
      throw error;
    });
    this.handles.set(key, pending);
    return pending;
  }

  private async start(target: ProjectTarget, dataDir: string, key: string, workspaceId: string, signal?: AbortSignal) {
    const remoteUrl = process.env.MARGINS_BB_REMOTE_URL?.trim();
    const remoteToken = process.env.MARGINS_BB_REMOTE_TOKEN?.trim();
    if (remoteUrl || remoteToken) {
      if (!remoteUrl || !remoteToken) {
        throw new Error("remote Margins selection requires URL and token together");
      }
      const parsed = new URL(remoteUrl);
      const loopback = parsed.protocol === "http:"
        && ["127.0.0.1", "localhost", "::1"].includes(parsed.hostname);
      if (parsed.protocol !== "https:" && !loopback) {
        throw new Error("remote Margins requires HTTPS or loopback HTTP");
      }
      const baseUrl = remoteUrl.replace(/\/$/, "");
      const response = await fetch(`${baseUrl}/v1/capabilities`, {
        signal,
        headers: { authorization: `Bearer ${remoteToken}` },
      });
      const envelope = await response.json() as {
        ok?: boolean;
        result?: { workspace_id?: string; instance_id?: string };
        error?: { message?: string };
      };
      if (!response.ok || !envelope.ok) {
        throw new Error(envelope.error?.message || `remote Margins capability check failed (${response.status})`);
      }
      if (envelope.result?.workspace_id !== workspaceId) {
        throw new Error("remote Margins capability Workspace does not match configured Workspace");
      }
      if (!envelope.result.instance_id) throw new Error("remote Margins capability response lacks an instance identity");
      return { baseUrl, token: remoteToken, workspaceId, instanceId: envelope.result.instance_id };
    }
    const asrRuntime = await readAsrRuntimeConfig(dataDir);
    const binary = asrRuntime?.serverPath ?? await this.runtime.ensureProjectServer({ dataDir, signal });
    const instanceDir = workspaceInstanceDir(dataDir, workspaceId);
    await mkdir(instanceDir, { recursive: true });
    const port = await availablePort();
    const child = spawn(binary, [], {
      cwd: marginsHome(),
      env: {
        ...process.env,
        MARGINS_HOST: "127.0.0.1",
        MARGINS_PORT: String(port),
        MARGINS_DATA_DIR: instanceDir,
        MARGINS_WORK_DIR: marginsHome(),
        MARGINS_BB_CAPTURE_WORKSPACE: "1",
        MARGINS_HOME: marginsHome(),
        MARGINS_INSTANCE_ID: `bb-host-${target.hostId}`,
        MARGINS_WORKSPACE: workspaceId,
        ...(asrRuntime ? {
          MARGINS_PARAKEET_MODEL_DIR: asrRuntime.modelDir,
          MARGINS_PARAKEET_MODEL_KIND: "tdt",
          ORT_DYLIB_PATH: asrRuntime.ortLibraryPath,
        } : {}),
      },
      stdio: "ignore",
    });
    const baseUrl = `http://127.0.0.1:${port}`;
    const token = await waitForServer(baseUrl, join(instanceDir, "token"), child, signal).catch((error) => {
      child.kill("SIGTERM");
      throw error;
    });
    child.once("exit", () => this.handles.delete(key));
    return { baseUrl, token, workspaceId, instanceId: `bb-host-${target.hostId}`, child };
  }

  async dispose() {
    for (const handle of await Promise.allSettled(this.handles.values())) {
      if (handle.status === "fulfilled" && handle.value.child?.exitCode === null) handle.value.child.kill("SIGTERM");
    }
    this.handles.clear();
  }
}

export class ProjectMarginsTransport {
  private readonly browserSessions = new Set<string>();
  constructor(private readonly manager = new ProjectServerManager()) {}

  private browserSessionKey(handle: ServerHandle, sessionId: string) {
    return `${handle.instanceId}:${handle.workspaceId}:${sessionId}`;
  }

  async listWorkspaceMeetings(target: ProjectTarget, dataDir: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const listed = await this.request<{ sessions: Array<{ session_id: string }> }>(handle, "sessions?limit=50", "GET");
      const rows = await Promise.all(listed.sessions.map(async ({ session_id }) => {
        const id = encodeURIComponent(session_id);
        const [summary, note] = await Promise.all([
          this.request<{ session_id: string; title: string | null; started_at: string; input_finalized: boolean;
            capture_duration_ms?: number | null; segment_count?: number;
            capture_lanes?: Array<{ label?: string | null; source_ids?: string[] }> }>(handle, `sessions/${id}`, "GET"),
          this.request<{ relative_path: string; bb_thread_ids?: string[]; distilled_memo_revision?: string | null } | null>(handle, `sessions/${id}/note-association`, "GET"),
        ]);
        return { summary, note };
      }));
      const homeRoot = handle.child && rows.some(({ note }) => note?.relative_path)
        ? await localHomeRoot(handle.workspaceId) : null;
      const meetings = await Promise.all(rows.map(async ({ summary, note }) => {
        const noteFilePath = await associatedNoteFile(homeRoot, note?.relative_path);
        const laneLabels = (summary.capture_lanes || []).flatMap((lane) => [lane.label || "", ...(lane.source_ids || [])]).join(" ").toLowerCase();
        const audioSource = laneLabels.includes("system") || laneLabels.includes("computer") || (summary.capture_lanes?.length || 0) > 1
          ? "Microphone + computer audio" : summary.segment_count === 0 ? null : "Microphone";
        return { sessionId: summary.session_id, title: summary.title, startedAt: summary.started_at,
          inputFinalized: summary.input_finalized, durationMs: summary.capture_duration_ms ?? null, audioSource,
          notePath: note?.relative_path || null,
          noteFile: noteFilePath ? { hostId: target.hostId, path: noteFilePath } : null,
          threadIds: note?.bb_thread_ids || [], distilledMemoRevision: note?.distilled_memo_revision || null };
      }));
      return { ok: true as const, meetings };
    } catch (cause) {
      return { ok: false as const, error: hostError("workspace_meetings_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async readWorkspaceMeeting(target: ProjectTarget, dataDir: string, sessionId?: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      let selected = sessionId;
      let candidates = sessionId ? [sessionId] : [];
      if (!selected) {
        const active = await this.request<{ sessions: Array<{ session_id: string }> }>(handle, "active-sessions", "GET");
        candidates = active.sessions.map((session) => session.session_id);
        selected = candidates.length === 1 ? candidates[0] : undefined;
        if (candidates.length === 0) {
          // The service orders ordinary sessions newest first. Keep the saved
          // meeting and its note available after a browser refresh or Stop.
          const recent = await this.request<{ sessions: Array<{ session_id: string }> }>(handle, "sessions?limit=1", "GET");
          selected = recent.sessions[0]?.session_id;
        }
      }
      if (!selected) return { ok: true as const, meeting: null, candidates };
      const [summary, memo] = await Promise.all([
        this.request<{ session_id: string; title: string | null; started_at: string; input_finalized: boolean }>(handle, `sessions/${encodeURIComponent(selected)}`, "GET"),
        this.request<{ revision: string; lines: Array<{ text: string }> }>(handle, `sessions/${encodeURIComponent(selected)}/memo`, "GET"),
      ]);
      return { ok: true as const, candidates, meeting: {
        sessionId: summary.session_id, title: summary.title, startedAt: summary.started_at,
        inputFinalized: summary.input_finalized,
        notepad: { revision: memo.revision, text: memo.lines.map((line) => line.text).join("\n") },
      } };
    } catch (cause) {
      return { ok: false as const, error: hostError("workspace_meeting_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async saveWorkspaceMemo(target: ProjectTarget, dataDir: string, sessionId: string, expectedRevision: string, text: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const summary = await this.request<{ started_at: string; input_finalized: boolean; capture_duration_ms: number | null }>(
        handle, `sessions/${encodeURIComponent(sessionId)}`, "GET",
      );
      const started = Date.parse(summary.started_at);
      if (!Number.isFinite(started)) throw new Error("Meeting start time is invalid");
      const observedAtMs = summary.input_finalized && summary.capture_duration_ms !== null
        ? summary.capture_duration_ms : Math.max(0, Date.now() - started);
      await this.request(handle, `sessions/${encodeURIComponent(sessionId)}/memo`, "PUT", {
        request_id: randomUUID(), expected_revision: expectedRevision,
        observed_at_ms: Math.floor(observedAtMs), paused: false, text,
      });
      return this.readWorkspaceMeeting(target, dataDir, sessionId);
    } catch (cause) {
      return { ok: false as const, error: hostError("workspace_memo_save_failed", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async renameWorkspaceMeeting(target: ProjectTarget, dataDir: string, sessionId: string, title: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      await this.request(handle, `sessions/${encodeURIComponent(sessionId)}/title`, "PUT", { request_id: randomUUID(), title });
      return { ok: true as const };
    } catch (cause) {
      return { ok: false as const, error: hostError("workspace_meeting_rename_failed", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async discardWorkspaceMeeting(target: ProjectTarget, dataDir: string, sessionId: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      await this.request(handle, `sessions/${encodeURIComponent(sessionId)}`, "DELETE");
      return { ok: true as const };
    } catch (cause) {
      return { ok: false as const, error: hostError("workspace_meeting_discard_failed", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async readWorkspaceTranscript(target: ProjectTarget, dataDir: string, sessionId: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const transcript = await this.request<{ body: string }>(handle, `sessions/${encodeURIComponent(sessionId)}/transcript`, "GET");
      return { ok: true as const, body: transcript.body };
    } catch (cause) {
      return { ok: false as const, error: hostError("workspace_transcript_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async authority(target: ProjectTarget, dataDir: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      return { ok: true as const, instanceId: handle.instanceId, workspaceId: handle.workspaceId };
    } catch (cause) {
      return { ok: false as const, error: hostError("project_recorder_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async sessionExists(target: ProjectTarget, dataDir: string, recordingId: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      let cursor: string | null = null;
      for (let page = 0; page < 10; page += 1) {
        const suffix: string = cursor ? `&after=${encodeURIComponent(cursor)}` : "";
        const result: { sessions: Array<{ session_id: string }>; next_cursor: string | null } = await this.request<{
          sessions: Array<{ session_id: string }>; next_cursor: string | null;
        }>(
          handle, `sessions?limit=100${suffix}`, "GET",
        );
        if (result.sessions.some((session) => session.session_id === recordingId)) return { ok: true as const, found: true };
        if (!result.next_cursor) return { ok: true as const, found: false };
        cursor = result.next_cursor;
      }
      return { ok: false as const, error: hostError("session_lookup_incomplete", "Margins could not verify the Mac session within the first 1,000 sessions") };
    } catch (cause) {
      return { ok: false as const, error: hostError("session_lookup_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async relayWorkspaceHttp(target: ProjectTarget, dataDir: string, input: {
    method: "GET" | "POST" | "PUT"; path: string; bodyBase64: string;
    contentType?: string; producerToken?: string; instanceId?: string;
  }) {
    const handle = await this.manager.ensure(target, dataDir);
    const [pathname = "", query, ...extra] = input.path.split("?");
    const scopedPrefix = `v1/workspaces/${handle.workspaceId}/`;
    const safePath = extra.length === 0 && (pathname === "v1/capabilities" || pathname.startsWith(scopedPrefix))
      && /^[a-zA-Z0-9/_-]+$/.test(pathname) && !pathname.includes("//")
      && (query === undefined || pathname === `${scopedPrefix}sessions` && /^limit=\d{1,3}(?:&after=[A-Za-z0-9_-]{1,200})?$/.test(query));
    if (!safePath) throw new Error("Menu relay path is outside its Workspace");
    const bytes = Buffer.from(input.bodyBase64, "base64");
    if (bytes.length > 1_500_000 || bytes.toString("base64") !== input.bodyBase64) throw new Error("Invalid Menu relay body");
    const response = await fetch(`${handle.baseUrl}/${input.path}`, {
      method: input.method,
      headers: {
        authorization: `Bearer ${handle.token}`,
        ...(input.contentType ? { "content-type": input.contentType } : {}),
        ...(input.producerToken ? { "X-Margins-Producer-Token": input.producerToken } : {}),
        ...(input.instanceId ? { "X-Margins-Instance-Id": input.instanceId } : {}),
      },
      body: input.method === "GET" ? undefined : bytes,
    });
    const body = Buffer.from(await response.arrayBuffer());
    if (body.length > 3_000_000) throw new Error("Menu relay response is too large");
    return { status: response.status, bodyBase64: body.toString("base64") };
  }

  private async request<T>(handle: ServerHandle, path: string, method: "GET" | "POST" | "PUT" | "DELETE", body?: object, signal?: AbortSignal): Promise<T> {
    const response = await fetch(`${handle.baseUrl}/v1/workspaces/${handle.workspaceId}/${path}`, {
      method,
      signal,
      headers: {
        authorization: `Bearer ${handle.token}`,
        "content-type": "application/json",
        "X-Margins-Instance-Id": handle.instanceId,
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const value = await response.json() as { ok: boolean; result?: T; error?: string | { message?: string } };
    const detail = typeof value.error === "string" ? value.error : value.error?.message;
    if (!response.ok) throw new Error(detail || `Margins could not save on the project machine (${response.status})`);
    if (!value.ok) throw new Error(detail || "Margins could not complete the recording action");
    return value.result as T;
  }

  private async transcriptSummary(handle: ServerHandle, recordingId: string) {
    const response = await fetch(`${handle.baseUrl}/v1/workspaces/${handle.workspaceId}/sessions/${recordingId}/transcript`, {
      headers: { authorization: `Bearer ${handle.token}` },
    });
    const envelope = await response.json() as {
      ok?: boolean;
      result?: { terminal: boolean; live: boolean; updated_at_unix_ms: number; decoded_until_ms: number; body: string };
      error?: { code?: string; message?: string };
    };
    if (response.status === 422 && envelope.error?.code === "invalid_request"
      && envelope.error.message === `No aligned transcript or capture context found for '${recordingId}'.`) {
      return null;
    }
    if (!response.ok || !envelope.ok || !envelope.result) {
      throw new Error(envelope.error?.message || `Margins transcript lookup failed (${response.status})`);
    }
    // A finalized browser capture can publish a terminal, memo-only live
    // checkpoint when Chrome did not deliver live PCM. Request durable-audio
    // transcription instead of treating that empty checkpoint as speech.
    const speechLine = String(envelope.result.body || "").split("\n")
      .some((line) => /^\[\d{2}:\d{2}(?::\d{2})?\]\s+(?!memo:)/.test(line));
    if (envelope.result.decoded_until_ms === 0 && !speechLine) return null;
    return envelope.result;
  }

  private async snapshot(handle: ServerHandle, recordingId: string, ownerId: string, signal?: AbortSignal): Promise<HostCaptureSnapshot> {
    return this.request<HostCaptureSnapshot>(handle, `browser/sessions/${recordingId}/snapshot?ownerId=${encodeURIComponent(ownerId)}`, "GET", undefined, signal);
  }

  private async withHandle(target: ProjectTarget, dataDir: string, action: (handle: ServerHandle) => Promise<HostCaptureSnapshot | null>): Promise<HostResult> {
    try {
      return { ok: true, snapshot: await action(await this.manager.ensure(target, dataDir)) };
    } catch (cause) {
      return { ok: false, error: hostError("project_recorder_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  start(target: ProjectTarget, dataDir: string, ownerId: string, name: string) {
    return this.withHandle(target, dataDir, async (handle) => {
      const snapshot = await this.request<HostCaptureSnapshot>(handle, "browser/sessions", "POST", { name, ownerId });
      this.browserSessions.add(this.browserSessionKey(handle, snapshot.sessionId));
      return snapshot;
    });
  }

  read(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string) {
    return this.withHandle(target, dataDir, (handle) => this.snapshot(handle, recordingId, ownerId));
  }

  mutate(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string, command: "heartbeat_web_recording" | "pause_recording" | "resume_recording") {
    return this.withHandle(target, dataDir, async (handle) => {
      const action = command === "heartbeat_web_recording" ? "heartbeat" : command === "pause_recording" ? "pause" : "resume";
      return this.request<HostCaptureSnapshot>(handle, `browser/sessions/${recordingId}/${action}`, "POST", { ownerId });
    });
  }

  stop(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string) {
    return this.withHandle(target, dataDir, async (handle) => {
      await this.request(handle, `browser/sessions/${recordingId}/stop`, "POST", { ownerId });
      return null;
    });
  }

  updateNotepad(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string, expectedRevision: string, text: string) {
    return this.withHandle(target, dataDir, async (handle) => {
      return this.request<HostCaptureSnapshot>(handle, `browser/sessions/${recordingId}/notepad`, "PUT", { ownerId, expectedRevision, text });
    });
  }

  async upload(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string, sequence: number, bytesBase64: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const response = await fetch(`${handle.baseUrl}/v1/workspaces/${handle.workspaceId}/browser/sessions/${recordingId}/chunks/${sequence}`, {
        method: "PUT",
        headers: {
          authorization: `Bearer ${handle.token}`,
          "content-type": "application/octet-stream",
          "x-margins-capture-owner": ownerId,
        },
        body: Buffer.from(bytesBase64, "base64"),
      });
      if (!response.ok) throw new Error(`audio upload failed (${response.status})`);
      const value = await response.json() as { ok: boolean; error?: string };
      if (!value.ok) throw new Error(value.error || "audio upload failed");
      return { ok: true as const };
    } catch (cause) {
      return { ok: false as const, error: hostError("audio_upload_failed", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async connectedNoteContext(target: ProjectTarget, dataDir: string, recordingId: string): Promise<ConnectedNoteResult> {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const [sessions, transcript, memo, artifacts, noteAssociation] = await Promise.all([
        this.request<{ sessions: Array<{ session_id: string; title: string | null }> }>(handle, "sessions?limit=100", "GET"),
        this.transcriptSummary(handle, recordingId),
        this.request<{ revision: string; lines: unknown[] }>(handle, `sessions/${recordingId}/memo`, "GET"),
        this.request<Array<{ artifact_id: string; kind: string; retention_class: string }>>(handle, `sessions/${recordingId}/artifacts`, "GET"),
        this.request<{ source_id: string; relative_path: string; revision: number } | null>(handle, `sessions/${recordingId}/note-association`, "GET"),
      ]);
      const summary = sessions.sessions.find((candidate) => candidate.session_id === recordingId);
      if (!summary) throw new Error("Pinned Margins session is not visible in the selected Workspace");
      return { ok: true, context: {
        schema: "margins.bb.connected-note-context.v1",
        instanceId: handle.instanceId,
        workspaceId: handle.workspaceId,
        sessionId: recordingId,
        title: summary.title,
        transcript: transcript
          ? { available: true, terminal: transcript.terminal, live: transcript.live, updatedAtUnixMs: transcript.updated_at_unix_ms }
          : { available: false, terminal: false, live: false, updatedAtUnixMs: 0 },
        memo: { revision: memo.revision, lineCount: memo.lines.length },
        artifacts: artifacts.map((artifact) => ({ artifactId: artifact.artifact_id, kind: artifact.kind, retentionClass: artifact.retention_class })),
        noteAssociation: noteAssociation ? { sourceId: noteAssociation.source_id, relativePath: noteAssociation.relative_path, revision: noteAssociation.revision } : null,
        instructions: "Pin this exact session before recall. If transcript.available is false, obtain or wait for transcription of this exact session before writing a grounded note. Fetch transcript/artifacts from Margins, but read and write ordinary note bytes only through the existing project Source; link only the Source-relative reference after writing.",
      } };
    } catch (cause) {
      return { ok: false, error: hostError("connected_note_context_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async requestTranscription(target: ProjectTarget, dataDir: string, recordingId: string): Promise<TranscriptionRequestResult> {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      if (handle.child && this.browserSessions.has(this.browserSessionKey(handle, recordingId))) {
        await this.transcribeHostedBrowserSession(handle, recordingId);
        return { ok: true, status: "complete", attempt: 1 };
      }
      let job: { status: "queued" | "running" | "complete" | "failed"; attempt: number };
      try {
        job = await this.request(handle, `sessions/${recordingId}/jobs/transcribe`, "POST");
      } catch (error) {
        // A browser session reopened after a worker restart is absent from the
        // in-memory set, but still has finalized local WAV segments.
        if (!handle.child || !(error instanceof Error) || !error.message.includes("session has no capture authority state")) throw error;
        await this.transcribeHostedBrowserSession(handle, recordingId);
        return { ok: true, status: "complete", attempt: 1 };
      }
      return { ok: true, status: job.status, attempt: job.attempt };
    } catch (cause) {
      return { ok: false, error: hostError("transcription_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  private async transcribeHostedBrowserSession(handle: ServerHandle, sessionId: string) {
    const response = await fetch(`${handle.baseUrl}/api/invoke/transcribe_hosted_browser_session`, {
      method: "POST", headers: { authorization: `Bearer ${handle.token}`, "content-type": "application/json" },
      body: JSON.stringify({ name: sessionId }),
    });
    const result = await response.json() as { ok?: boolean; error?: string };
    if (!response.ok || !result.ok) throw new Error(result.error || `Hosted browser transcription failed (${response.status})`);
  }

  dispose() { return this.manager.dispose(); }
}
