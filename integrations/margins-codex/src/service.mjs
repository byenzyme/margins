import { readFile } from "node:fs/promises";
import { randomUUID } from "node:crypto";

function configuredUrl(value) {
  if (!value) throw new Error("Set MARGINS_CODEX_URL to the Margins service origin");
  const url = new URL(value);
  const loopback = url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
  if (url.protocol !== "https:" && !loopback) {
    throw new Error("Margins service must use HTTPS or loopback HTTP");
  }
  if (url.username || url.password || url.search || url.hash || url.pathname !== "/") {
    throw new Error("Margins service URL must contain only an origin");
  }
  return url.origin;
}

export class MarginsService {
  constructor({ url, workspace, token, tokenFile, fetchImpl = fetch }) {
    this.origin = configuredUrl(url);
    if (!workspace || !/^[A-Za-z0-9_-]+$/.test(workspace)) {
      throw new Error("MARGINS_CODEX_WORKSPACE must be a Workspace ID");
    }
    if (!token && !tokenFile) throw new Error("Set MARGINS_CODEX_TOKEN or MARGINS_CODEX_TOKEN_FILE");
    this.workspace = workspace;
    this.token = token;
    this.tokenFile = tokenFile;
    this.fetchImpl = fetchImpl;
    this.instance = null;
  }

  static fromEnv(env = process.env) {
    return new MarginsService({
      url: env.MARGINS_CODEX_URL,
      workspace: env.MARGINS_CODEX_WORKSPACE,
      token: env.MARGINS_CODEX_TOKEN,
      tokenFile: env.MARGINS_CODEX_TOKEN_FILE,
    });
  }

  async credential() {
    const value = this.tokenFile ? await readFile(this.tokenFile, "utf8") : this.token;
    if (!value?.trim()) throw new Error("Margins credential is empty");
    return value.trim();
  }

  async request(path, method = "GET", body) {
    const credential = await this.credential();
    const response = await this.fetchImpl(`${this.origin}/${path}`, {
      method,
      signal: AbortSignal.timeout(10_000),
      headers: {
        authorization: `Bearer ${credential}`,
        ...(this.instance ? { "X-Margins-Instance-Id": this.instance } : {}),
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    let envelope;
    try { envelope = await response.json(); }
    catch { throw new Error(`Margins service returned invalid JSON (${response.status})`); }
    if (!response.ok || envelope?.ok !== true) {
      const message = envelope?.error?.message || envelope?.error || `request failed (${response.status})`;
      throw new Error(typeof message === "string" ? message : `request failed (${response.status})`);
    }
    return envelope.result;
  }

  async capabilities() {
    const result = await this.request("v1/capabilities");
    if (result.workspace_id !== this.workspace || !result.instance_id) {
      throw new Error("Margins service identity does not match the configured Workspace");
    }
    this.instance = result.instance_id;
    return result;
  }

  async scoped(path, method = "GET", body) {
    if (!this.instance) await this.capabilities();
    return this.request(`v1/workspaces/${encodeURIComponent(this.workspace)}/${path}`, method, body);
  }

  async sessions(limit = 20) {
    return this.scoped(`sessions?limit=${limit}`);
  }

  async currentSession() {
    const active = await this.scoped("active-sessions");
    const candidates = active.sessions.map((session) => session.session_id);
    const own = await this.scoped("current");
    const current = candidates.includes(own) ? own : candidates.length === 1 ? candidates[0] : null;
    return { current_session_id: current, candidates };
  }

  async exactSummary(sessionId) {
    return this.scoped(`sessions/${encodeURIComponent(sessionId)}`);
  }

  async summary(sessionId) {
    const page = await this.sessions(100);
    return page.sessions.find((session) => session.session_id === sessionId) ?? null;
  }

  async transcript(sessionId) {
    return this.scoped(`sessions/${encodeURIComponent(sessionId)}/transcript`);
  }

  async liveMeeting(sessionId) {
    const summary = await this.exactSummary(sessionId);
    let transcript = null;
    try { transcript = await this.transcript(sessionId); }
    catch (error) {
      // The native bridge uploads audio while recording. CoreML checkpoints
      // are optional, so no transcript before finalization is expected too.
      if (!/No aligned transcript or capture context found/.test(String(error?.message ?? error))) throw error;
    }
    return {
      session_id: sessionId,
      title: summary.title ?? null,
      input_finalized: summary.input_finalized,
      segment_count: summary.segment_count ?? 0,
      processing_state: summary.processing_state ?? "none",
      capture_duration_ms: summary.capture_duration_ms ?? null,
      transcript: transcript && {
        body: transcript.body,
        live: transcript.live,
        terminal: transcript.terminal,
        view: transcript.view,
        updated_at_unix_ms: transcript.updated_at_unix_ms,
      },
    };
  }

  async memo(sessionId) {
    return this.scoped(`sessions/${encodeURIComponent(sessionId)}/memo`);
  }

  async saveMemo({ sessionId, expectedRevision, text, observedAtMs, paused = false }) {
    if (!expectedRevision) throw new Error("Read the memo first and pass its revision");
    if (!Number.isSafeInteger(observedAtMs) || observedAtMs < 0) {
      throw new Error("observedAtMs must be a nonnegative integer");
    }
    return this.scoped(`sessions/${encodeURIComponent(sessionId)}/memo`, "PUT", {
      request_id: randomUUID(), expected_revision: expectedRevision,
      observed_at_ms: observedAtMs, paused, text,
    });
  }
}
