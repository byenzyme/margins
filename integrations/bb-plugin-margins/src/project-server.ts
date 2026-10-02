import { randomUUID } from "node:crypto";
import { execFile as execFileCallback, spawn, type ChildProcess } from "node:child_process";
import { access, lstat, mkdir, readFile, realpath } from "node:fs/promises";
import { createServer } from "node:net";
import { homedir } from "node:os";
import { isAbsolute, join, relative, resolve } from "node:path";
import { promisify } from "node:util";
import { z } from "zod";
import { CAPTURE_PROTOCOL_VERSION, type ConnectedNoteResult, type HostCaptureSnapshot, type HostError, type HostResult, type ProjectTarget, type TranscriptionRequestResult } from "./contracts.js";
import { createRuntimeManager } from "./runtime-manager.js";

const execFile = promisify(execFileCallback);

// Workspace response shapes are checked against the Rust protocol structs by
// protocol-contract.test.ts. Keep the request types tied to these schemas.
const captureLaneSchema = z.strictObject({
  lane_id: z.string(), source_ids: z.array(z.string()), label: z.string().nullable().optional(),
  format: z.strictObject({
    codec: z.enum(["pcm_s16_le", "pcm_f32_le", "opus", "aac_lc"]),
    container: z.enum(["raw", "webm", "ogg", "mp4", "packet_stream"]),
    sample_rate_hz: z.number().int().nonnegative(), channel_count: z.number().int().nonnegative(),
  }),
});
export const workspaceSessionSummarySchema = z.strictObject({
  session_id: z.string(), title: z.string().nullable(), started_at: z.string(),
  capture_lanes: z.array(captureLaneSchema), segment_count: z.number().int().nonnegative(),
  input_finalized: z.boolean(), capture_duration_ms: z.number().int().nonnegative().nullable(),
  capture_finalize_message_id: z.string().nullable(), processing_state: z.string(),
  capture_incomplete: z.boolean().default(false), capture_gaps: z.array(z.strictObject({
    segment_id: z.string(), start_sequence: z.number().int().nonnegative(),
    end_exclusive: z.number().int().nonnegative(), reason: z.string(),
  })).default([]),
});
export const workspaceSessionPageSchema = z.strictObject({
  sessions: z.array(workspaceSessionSummarySchema), next_cursor: z.string().nullable(),
});
export const workspaceTranscriptSchema = z.strictObject({
  session_id: z.string(), body: z.string(), view: z.string(),
  decoded_until_ms: z.number().int().nonnegative(), committed_until_ms: z.number().int().nonnegative(),
  updated_at_unix_ms: z.number().int().nonnegative(), live: z.boolean(), terminal: z.boolean(),
  source_artifact: z.string(),
});
export const workspaceArtifactSchema = z.strictObject({
  artifact_id: z.string(), session_id: z.string(), kind: z.string(), ordinal: z.number().int(),
  size_bytes: z.number().int().nonnegative().nullable(), retention_class: z.string(), created_at: z.string(),
});
export const workspaceMemoSchema = z.strictObject({
  session_id: z.string(), revision: z.string(), lines: z.array(z.strictObject({
    text: z.string(), created_secs: z.number(), edited_secs: z.number().nullable(),
    draft_started_secs: z.number().nullable(), audio_pending_at_mark: z.boolean(),
    block_ordinal: z.number().int().nonnegative().nullable(),
  })), mirror_stale: z.boolean().optional(),
});
export const workspaceNoteAssociationSchema = z.strictObject({
  session_id: z.string(), source_id: z.string(), relative_path: z.string(),
  observed_content_hash: z.string().nullable(), revision: z.number().int().nonnegative(),
  bb_thread_ids: z.array(z.string()), distilled_memo_revision: z.string().nullable(),
});
export const workspaceProcessingJobSchema = z.strictObject({
  job_id: z.string(), session_id: z.string(), operation: z.string(), input_revision: z.string(),
  attempt: z.number().int().nonnegative(), status: z.enum(["queued", "running", "complete", "failed"]), progress: z.number().nullable(),
  result_ref: z.string().nullable(), failure: z.string().nullable(), failed_stage: z.string().nullable(),
});

type WorkspaceSessionPage = z.infer<typeof workspaceSessionPageSchema>;
type WorkspaceSessionSummary = z.infer<typeof workspaceSessionSummarySchema>;
type WorkspaceTranscript = z.infer<typeof workspaceTranscriptSchema>;
type WorkspaceArtifact = z.infer<typeof workspaceArtifactSchema>;
type WorkspaceMemo = z.infer<typeof workspaceMemoSchema>;
type WorkspaceNoteAssociation = z.infer<typeof workspaceNoteAssociationSchema>;
type WorkspaceProcessingJob = z.infer<typeof workspaceProcessingJobSchema>;

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

async function verifyServerCompatibility(baseUrl: string, token: string, workspaceId: string, signal?: AbortSignal): Promise<string> {
  const response = await fetch(`${baseUrl}/v1/capabilities`, {
    signal, headers: { authorization: `Bearer ${token}` },
  });
  const envelope = await response.json().catch(() => null) as {
    ok?: boolean;
    result?: { workspace_id?: string; instance_id?: string; protocol_version?: number; capture_protocol_version?: number };
    error?: { message?: string };
  } | null;
  if (response.status === 404 || (response.ok && !envelope?.ok)) {
    throw new Error("Margins server version doesn't match this plugin; upgrade both");
  }
  if (!response.ok || !envelope?.ok) {
    throw new Error(envelope?.error?.message || `Margins capability check failed (${response.status})`);
  }
  if (envelope.result?.protocol_version !== 1 || envelope.result.capture_protocol_version !== CAPTURE_PROTOCOL_VERSION) {
    throw new Error("Margins server version doesn't match this plugin; upgrade both");
  }
  if (envelope.result.workspace_id !== workspaceId) {
    throw new Error("Margins capability Workspace does not match configured Workspace");
  }
  if (!envelope.result.instance_id) throw new Error("Margins capability response lacks an instance identity");
  return envelope.result.instance_id;
}

function hostError(code: string, message: string, retryable = true): HostError {
  return { code, message, retryable };
}

class WorkspaceRequestError extends Error {
  constructor(readonly code: string, message: string, readonly retryable: boolean) {
    super(message);
  }
}

export function workspaceInstanceDir(dataDir: string, workspaceId: string) {
  return join(dataDir, "workspace-servers", workspaceId);
}

export function marginsCli(): string {
  const configured = process.env.MARGINS_CLI_BIN?.trim();
  if (configured && isAbsolute(configured)) return configured;
  const installed = join(process.env.MARGINS_CLI_BIN_DIR?.trim() || join(homedir(), ".local", "bin"), "margins");
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
  const { stdout } = await execFile(binary, ["workspace", "list", "--json"], { env, timeout: 15_000, maxBuffer: 65_536 });
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
    await execFile(binary, ["workspace", "default", "--set", selected, "--json"], { env, timeout: 15_000, maxBuffer: 65_536 });
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

async function workspaceNoteDestination(workspaceId: string) {
  if (!/^[a-z0-9][a-z0-9-]*$/.test(workspaceId)) throw new Error("Invalid Margins Workspace id");
  const { stdout } = await execFile(marginsCli(), ["--workspace", workspaceId, "workspace", "destination", "--json"], {
    env: { ...process.env, MARGINS_HOME: marginsHome() }, timeout: 5_000, maxBuffer: 65_536,
  });
  const value = JSON.parse(stdout) as { destination?: unknown; home_root?: unknown; home_source_id?: unknown };
  if (typeof value.destination !== "string" || !isAbsolute(value.destination)
    || typeof value.home_root !== "string" || !isAbsolute(value.home_root)
    || typeof value.home_source_id !== "string" || !value.home_source_id) {
    throw new Error("Margins Workspace note destination is unavailable");
  }
  return { destination: value.destination, homeRoot: value.home_root, homeSourceId: value.home_source_id };
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

  async ensureCli(dataDir: string) {
    const cli = marginsCli();
    const stat = await lstat(cli).catch(() => null);
    if (stat?.isFile() && (stat.mode & 0o111) !== 0) return cli;
    if (process.env.MARGINS_CLI_BIN) throw new Error("The configured Margins CLI is not executable");
    await this.runtime.ensureProjectServer({ dataDir });
    const installed = await lstat(cli).catch(() => null);
    if (!installed?.isFile() || (installed.mode & 0o111) === 0) {
      throw new Error("Margins CLI installation did not complete");
    }
    return cli;
  }

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
      const instanceId = await verifyServerCompatibility(baseUrl, remoteToken, workspaceId, signal);
      return { baseUrl, token: remoteToken, workspaceId, instanceId };
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
    await verifyServerCompatibility(baseUrl, token, workspaceId, signal).catch((error) => {
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
  constructor(private readonly manager = new ProjectServerManager()) {}

  async prepareCli(dataDir: string) { await this.manager.ensureCli(dataDir); }

  async listWorkspaceMeetings(target: ProjectTarget, dataDir: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const [listed, active] = await Promise.all([
        this.request<WorkspaceSessionPage>(handle, "sessions?limit=50", "GET"),
        this.request<WorkspaceSessionPage>(handle, "active-sessions", "GET"),
      ]);
      // Ordinary sessions omit empty reservations until audio arrives. Active
      // capture sessions must still be visible so BB can join their memo now.
      const sessionIds = [...new Set([...active.sessions, ...listed.sessions].map((session) => session.session_id))];
      const rows = await Promise.all(sessionIds.map(async (session_id) => {
        const id = encodeURIComponent(session_id);
        const [summary, note] = await Promise.all([
          this.request<WorkspaceSessionSummary>(handle, `sessions/${id}`, "GET"),
          this.request<WorkspaceNoteAssociation | null>(handle, `sessions/${id}/note-association`, "GET"),
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
          captureIncomplete: summary.capture_incomplete ?? false, captureGaps: (summary.capture_gaps ?? []).map((gap) => ({
            segmentId: gap.segment_id, startSequence: gap.start_sequence,
            endExclusive: gap.end_exclusive, reason: gap.reason,
          })),
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
        const active = await this.request<WorkspaceSessionPage>(handle, "active-sessions", "GET");
        candidates = active.sessions.map((session) => session.session_id);
        selected = candidates.length === 1 ? candidates[0] : undefined;
        if (candidates.length === 0) {
          // The service orders ordinary sessions newest first. Keep the saved
          // meeting and its note available after a browser refresh or Stop.
          const recent = await this.request<WorkspaceSessionPage>(handle, "sessions?limit=1", "GET");
          selected = recent.sessions[0]?.session_id;
        }
      }
      if (!selected) return { ok: true as const, meeting: null, candidates };
      const [summary, memo] = await Promise.all([
        this.request<WorkspaceSessionSummary>(handle, `sessions/${encodeURIComponent(selected)}`, "GET"),
        this.request<WorkspaceMemo>(handle, `sessions/${encodeURIComponent(selected)}/memo`, "GET"),
      ]);
      return { ok: true as const, candidates, meeting: {
        sessionId: summary.session_id, title: summary.title, startedAt: summary.started_at,
        inputFinalized: summary.input_finalized, captureIncomplete: summary.capture_incomplete ?? false,
        captureGaps: (summary.capture_gaps ?? []).map((gap) => ({ segmentId: gap.segment_id,
          startSequence: gap.start_sequence, endExclusive: gap.end_exclusive, reason: gap.reason })),
        notepad: { revision: memo.revision, text: memo.lines.map((line) => line.text).join("\n") },
      } };
    } catch (cause) {
      return { ok: false as const, error: hostError("workspace_meeting_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async saveWorkspaceMemo(target: ProjectTarget, dataDir: string, sessionId: string, expectedRevision: string, text: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const summary = await this.request<WorkspaceSessionSummary>(
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
      const transcript = await this.request<WorkspaceTranscript>(handle, `sessions/${encodeURIComponent(sessionId)}/transcript`, "GET");
      return { ok: true as const, body: transcript.body };
    } catch (cause) {
      return { ok: false as const, error: hostError("workspace_transcript_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async noteDestination(target: ProjectTarget, dataDir: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      return { ok: true as const, ...await workspaceNoteDestination(handle.workspaceId) };
    } catch (cause) {
      return { ok: false as const, error: hostError("note_destination_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async linkWorkspaceNote(target: ProjectTarget, dataDir: string, input: {
    sessionId: string; sourceId: string; relativePath: string; expectedRevision: number;
    bbThreadId: string; memoRevision: string;
  }) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const destination = await workspaceNoteDestination(handle.workspaceId);
      if (input.sourceId !== destination.homeSourceId
        || !await associatedNoteFile(await realpath(destination.homeRoot), input.relativePath)) {
        throw new Error("The note must exist inside the selected Workspace Home Source");
      }
      const result = await this.request<WorkspaceNoteAssociation>(handle,
        `sessions/${encodeURIComponent(input.sessionId)}/note-association`, "PUT", {
          request_id: randomUUID(), source_id: input.sourceId, relative_path: input.relativePath,
          observed_content_hash: null, expected_revision: input.expectedRevision,
          bb_thread_id: input.bbThreadId, distilled_memo_revision: input.memoRevision,
        });
      return { ok: true as const, revision: result.revision };
    } catch (cause) {
      return { ok: false as const, error: hostError("note_association_failed", cause instanceof Error ? cause.message : String(cause)) };
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

  async speechSetup(target: ProjectTarget, dataDir: string, retry = false) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const status = await this.request<{ state: "preparing" | "ready" | "failed" | "unavailable"; message: string; progress: number | null }>(
        handle, "speech-setup", retry ? "POST" : "GET");
      return { ok: true as const, ...status };
    } catch (cause) {
      return { ok: false as const, error: hostError("speech_setup_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  async sessionExists(target: ProjectTarget, dataDir: string, recordingId: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      let cursor: string | null = null;
      for (let page = 0; page < 10; page += 1) {
        const suffix: string = cursor ? `&after=${encodeURIComponent(cursor)}` : "";
        const result: WorkspaceSessionPage = await this.request<WorkspaceSessionPage>(
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
    const value = await response.json() as { ok: boolean; result?: T; error?: string | { code?: string; message?: string; retryable?: boolean } };
    const detail = typeof value.error === "string" ? value.error : value.error?.message;
    if (!response.ok || !value.ok) {
      const structured = typeof value.error === "object" ? value.error : undefined;
      throw new WorkspaceRequestError(
        structured?.code || "workspace_request_failed",
        detail || `Margins could not save on the project machine (${response.status})`,
        structured?.retryable ?? true,
      );
    }
    return value.result as T;
  }

  private async transcriptSummary(handle: ServerHandle, recordingId: string) {
    const response = await fetch(`${handle.baseUrl}/v1/workspaces/${handle.workspaceId}/sessions/${recordingId}/transcript`, {
      headers: { authorization: `Bearer ${handle.token}` },
    });
    const envelope = await response.json() as {
      ok?: boolean;
      result?: WorkspaceTranscript;
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
      return { ok: false, error: cause instanceof WorkspaceRequestError
        ? hostError(cause.code, cause.message, cause.retryable)
        : hostError("project_recorder_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  start(target: ProjectTarget, dataDir: string, ownerId: string, name: string, startedAtUnixMs?: number) {
    return this.withHandle(target, dataDir, async (handle) => {
      const snapshot = await this.request<HostCaptureSnapshot>(handle, "browser/sessions", "POST", { name, ownerId,
        ...(startedAtUnixMs !== undefined ? { startedAtUnixMs } : {}) });
      return snapshot;
    });
  }

  read(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string) {
    return this.withHandle(target, dataDir, (handle) => this.snapshot(handle, recordingId, ownerId));
  }

  mutate(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string, command: "heartbeat_web_recording" | "pause_recording" | "resume_recording", expectedNextSequence?: number, segmentTimeUnixMs?: number, recoveredAfterReload?: boolean) {
    return this.withHandle(target, dataDir, async (handle) => {
      const action = command === "heartbeat_web_recording" ? "heartbeat" : command === "pause_recording" ? "pause" : "resume";
      return this.request<HostCaptureSnapshot>(handle, `browser/sessions/${recordingId}/${action}`, "POST",
        command === "pause_recording" ? { ownerId, expectedNextSequence,
          ...(segmentTimeUnixMs !== undefined ? { segmentEndedUnixMs: segmentTimeUnixMs } : {}),
          ...(recoveredAfterReload ? { recoveredAfterReload: true } : {}) }
          : command === "resume_recording" && segmentTimeUnixMs !== undefined
            ? { ownerId, segmentStartedUnixMs: segmentTimeUnixMs } : { ownerId });
    });
  }

  stop(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string, expectedNextSequence: number, segmentEndedUnixMs?: number) {
    return this.withHandle(target, dataDir, async (handle) => {
      await this.request(handle, `browser/sessions/${recordingId}/stop`, "POST", { ownerId, expectedNextSequence,
        ...(segmentEndedUnixMs !== undefined ? { segmentEndedUnixMs } : {}) });
      return null;
    });
  }

  finishIncomplete(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string, expectedNextSequence: number) {
    return this.withHandle(target, dataDir, async (handle) => {
      await this.request(handle, `browser/sessions/${recordingId}/finish-incomplete`, "POST", { ownerId, expectedNextSequence });
      return null;
    });
  }

  async upload(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string, sequence: number, bytesBase64: string,
    capturedStartUnixMs: number, capturedEndUnixMs: number) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const response = await fetch(`${handle.baseUrl}/v1/workspaces/${handle.workspaceId}/browser/sessions/${recordingId}/chunks/${sequence}`, {
        method: "PUT",
        headers: {
          authorization: `Bearer ${handle.token}`,
          "content-type": "application/octet-stream",
          "x-margins-capture-owner": ownerId,
          "X-Margins-Instance-Id": handle.instanceId,
          "X-Margins-Captured-Start-Unix-Ms": String(capturedStartUnixMs),
          "X-Margins-Captured-End-Unix-Ms": String(capturedEndUnixMs),
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
        this.request<WorkspaceSessionPage>(handle, "sessions?limit=100", "GET"),
        this.transcriptSummary(handle, recordingId),
        this.request<WorkspaceMemo>(handle, `sessions/${recordingId}/memo`, "GET"),
        this.request<WorkspaceArtifact[]>(handle, `sessions/${recordingId}/artifacts`, "GET"),
        this.request<WorkspaceNoteAssociation | null>(handle, `sessions/${recordingId}/note-association`, "GET"),
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
      const job = await this.request<WorkspaceProcessingJob>(
        handle, `sessions/${recordingId}/jobs/transcribe`, "POST");
      return { ok: true, status: job.status, attempt: job.attempt };
    } catch (cause) {
      return { ok: false, error: hostError("transcription_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  dispose() { return this.manager.dispose(); }
}
