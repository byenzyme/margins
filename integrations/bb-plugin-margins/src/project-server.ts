import { createHash } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdir, readFile } from "node:fs/promises";
import { createServer } from "node:net";
import { join } from "node:path";
import type { HostCaptureSnapshot, HostError, HostResult, ProjectTarget } from "./contracts.js";
import { CAPTURE_PROTOCOL_VERSION } from "./contracts.js";
import { createRuntimeManager } from "./runtime-manager.js";

interface ServerHandle {
  baseUrl: string;
  token: string;
  child: ChildProcess;
}

interface RecordingStatus {
  is_recording: boolean;
  paused: boolean;
  session_name: string | null;
  web_recording_id?: string | null;
  elapsed_secs: number;
  capture_phase?: string;
}

interface WebNotepadSnapshot { text: string; revision: string }

function hostError(code: string, message: string, retryable = true): HostError {
  return { code, message, retryable };
}

function projectKey(target: ProjectTarget) {
  return createHash("sha256").update(`${target.projectId}\0${target.projectRoot}`).digest("hex").slice(0, 20);
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
    const key = projectKey(target);
    const existing = this.handles.get(key);
    if (existing) return existing;
    const pending = this.start(target, dataDir, key, signal).catch((error) => {
      this.handles.delete(key);
      throw error;
    });
    this.handles.set(key, pending);
    return pending;
  }

  private async start(target: ProjectTarget, dataDir: string, key: string, signal?: AbortSignal) {
    const binary = await this.runtime.ensureProjectServer({ dataDir, signal });
    const instanceDir = join(dataDir, "projects", key);
    await mkdir(instanceDir, { recursive: true });
    const port = await availablePort();
    const child = spawn(binary, [], {
      cwd: target.projectRoot,
      env: {
        ...process.env,
        MARGINS_HOST: "127.0.0.1",
        MARGINS_PORT: String(port),
        MARGINS_DATA_DIR: instanceDir,
        MARGINS_WORK_DIR: target.projectRoot,
      },
      stdio: "ignore",
    });
    const baseUrl = `http://127.0.0.1:${port}`;
    const token = await waitForServer(baseUrl, join(instanceDir, "token"), child, signal).catch((error) => {
      child.kill("SIGTERM");
      throw error;
    });
    child.once("exit", () => this.handles.delete(key));
    return { baseUrl, token, child };
  }

  async dispose() {
    for (const handle of await Promise.allSettled(this.handles.values())) {
      if (handle.status === "fulfilled" && handle.value.child.exitCode === null) handle.value.child.kill("SIGTERM");
    }
    this.handles.clear();
  }
}

export class ProjectMarginsTransport {
  constructor(private readonly manager = new ProjectServerManager()) {}

  private async invoke<T>(handle: ServerHandle, command: string, body: object, signal?: AbortSignal): Promise<T> {
    const response = await fetch(`${handle.baseUrl}/api/invoke/${command}`, {
      method: "POST",
      signal,
      headers: {
        authorization: `Bearer ${handle.token}`,
        "content-type": "application/json",
        "x-margins-capture-protocol": String(CAPTURE_PROTOCOL_VERSION),
      },
      body: JSON.stringify(body),
    });
    if (!response.ok) throw new Error(`Margins could not save on the project machine (${response.status})`);
    const value = await response.json() as { ok: boolean; result?: T; error?: string };
    if (!value.ok) throw new Error(value.error || "Margins could not complete the recording action");
    return value.result as T;
  }

  private async snapshot(handle: ServerHandle, recordingId: string, ownerId: string, signal?: AbortSignal): Promise<HostCaptureSnapshot> {
    const [status, notepad] = await Promise.all([
      this.invoke<RecordingStatus>(handle, "get_web_recording_status", { recordingId, ownerId }, signal),
      this.invoke<WebNotepadSnapshot>(handle, "get_web_recording_notepad", { recordingId, ownerId }, signal),
    ]);
    return {
      recordingId,
      meetingId: status.session_name || recordingId,
      status: status.capture_phase === "finalizing" ? "saving" : status.paused ? "paused" : "recording",
      elapsedMs: Math.max(0, Math.round(status.elapsed_secs * 1000)),
      notepad,
      transcriptAvailable: false,
    };
  }

  private async withHandle(target: ProjectTarget, dataDir: string, action: (handle: ServerHandle) => Promise<HostCaptureSnapshot | null>): Promise<HostResult> {
    try {
      return { ok: true, snapshot: await action(await this.manager.ensure(target, dataDir)) };
    } catch (cause) {
      return { ok: false, error: hostError("project_recorder_unavailable", cause instanceof Error ? cause.message : String(cause)) };
    }
  }

  prepareProject(target: ProjectTarget, dataDir: string) {
    return this.manager.ensure(target, dataDir).then(() => ({ ok: true as const })).catch((cause) => ({
      ok: false as const,
      error: hostError("project_recorder_unavailable", cause instanceof Error ? cause.message : String(cause)),
    }));
  }

  start(target: ProjectTarget, dataDir: string, ownerId: string, name: string) {
    return this.withHandle(target, dataDir, async (handle) => {
      const started = await this.invoke<{ sessionName: string; recordingId: string }>(handle, "start_recording", { name, ownerId });
      return this.snapshot(handle, started.recordingId, ownerId);
    });
  }

  read(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string) {
    return this.withHandle(target, dataDir, (handle) => this.snapshot(handle, recordingId, ownerId));
  }

  mutate(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string, command: "heartbeat_web_recording" | "pause_recording" | "resume_recording") {
    return this.withHandle(target, dataDir, async (handle) => {
      await this.invoke(handle, command, { recordingId, ownerId });
      return this.snapshot(handle, recordingId, ownerId);
    });
  }

  stop(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string) {
    return this.withHandle(target, dataDir, async (handle) => {
      await this.invoke(handle, "stop_recording", { recordingId, ownerId });
      return null;
    });
  }

  updateNotepad(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string, expectedRevision: string, text: string) {
    return this.withHandle(target, dataDir, async (handle) => {
      await this.invoke(handle, "update_web_recording_notepad", { recordingId, ownerId, expectedRevision, text });
      return this.snapshot(handle, recordingId, ownerId);
    });
  }

  async upload(target: ProjectTarget, dataDir: string, recordingId: string, ownerId: string, sequence: number, bytesBase64: string) {
    try {
      const handle = await this.manager.ensure(target, dataDir);
      const response = await fetch(`${handle.baseUrl}/api/audio/chunk`, {
        method: "POST",
        headers: {
          authorization: `Bearer ${handle.token}`,
          "content-type": "application/octet-stream",
          "x-margins-capture-protocol": String(CAPTURE_PROTOCOL_VERSION),
          "x-margins-recording-id": recordingId,
          "x-margins-capture-owner": ownerId,
          "x-margins-chunk-sequence": String(sequence),
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

  dispose() { return this.manager.dispose(); }
}
