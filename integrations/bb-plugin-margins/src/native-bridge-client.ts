const STORAGE_KEY = "margins.bb.native-bridge.v1";

export interface CaptureAuthority { instanceId: string; workspaceId: string }
export interface NativeStatus extends CaptureAuthority {
  state: "ready" | "getting_ready" | "recording" | "paused" | "saving" | "saved" | "needs_attention";
  sessionId: string | null;
  transferId: string | null;
  microphoneSamples: number;
  systemSamples: number;
  microphoneDroppedSamples: number;
  systemDroppedSamples: number;
  systemFrames: number;
  systemSilentSamples: number;
  error: string | null;
}
interface Pairing extends CaptureAuthority { token: string; port: number }
type Listener = () => void;

function storedPairing(): Pairing | null {
  try {
    const value = JSON.parse(sessionStorage.getItem(STORAGE_KEY) || "null") as Partial<Pairing> | null;
    return value && typeof value.token === "string"
      && typeof value.instanceId === "string" && typeof value.workspaceId === "string"
      && Number.isInteger(value.port) && Number(value.port) > 0 && Number(value.port) <= 65535 ? value as Pairing : null;
  } catch { return null; }
}

async function bridgeRequest<T>(port: number, path: string, body?: object, token?: string): Promise<T> {
  const controller = new AbortController();
  const deadline = setTimeout(() => controller.abort(), 7_000);
  try {
    const response = await fetch(`http://127.0.0.1:${port}${path}`, {
      method: body === undefined ? "GET" : "POST",
      mode: "cors",
      cache: "no-store",
      signal: controller.signal,
      headers: {
        ...(body === undefined ? {} : { "content-type": "application/json" }),
        ...(token ? { authorization: `Bearer ${token}` } : {}),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const value = await response.json() as T & { error?: string };
    if (!response.ok) throw new Error(value.error || `Mac recorder returned ${response.status}`);
    return value;
  } finally { clearTimeout(deadline); }
}

export class NativeBridgeOwner {
  private pairing: Pairing | null = storedPairing();
  private currentStatus: NativeStatus | null = null;
  private currentError: string | null = null;
  private timer: ReturnType<typeof setInterval> | null = null;
  private subscribers = new Set<Listener>();

  get sessionId() { return this.currentStatus?.sessionId || null; }
  get status() { return this.currentStatus; }
  get connectionError() { return this.currentError; }
  get paired() { return Boolean(this.pairing); }
  subscribe(listener: Listener) {
    this.subscribers.add(listener);
    if (this.pairing && !this.timer) this.startPolling();
    return () => { this.subscribers.delete(listener); };
  }
  private emit() { for (const subscriber of this.subscribers) subscriber(); }
  private startPolling() {
    if (this.timer) clearInterval(this.timer);
    this.timer = setInterval(() => void this.refresh().catch(() => undefined), 2_000);
    void this.refresh().catch(() => undefined);
  }
  async pair(code: string, expected: CaptureAuthority, port = 18765) {
    if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("Enter a valid Mac recorder port.");
    const value = await bridgeRequest<{ token: string; instanceId: string; workspaceId: string; status: NativeStatus }>(port, "/v1/pair", { code });
    if (value.instanceId !== expected.instanceId || value.workspaceId !== expected.workspaceId) {
      throw new Error(`Mac recorder points to ${value.instanceId}/${value.workspaceId}; this BB project uses ${expected.instanceId}/${expected.workspaceId}. Configure both for the same Margins destination.`);
    }
    this.pairing = { token: value.token, instanceId: value.instanceId, workspaceId: value.workspaceId, port };
    this.currentStatus = value.status;
    this.currentError = null;
    sessionStorage.setItem(STORAGE_KEY, JSON.stringify(this.pairing));
    this.emit();
    this.startPolling();
  }
  async verify(expected: CaptureAuthority) {
    if (!this.pairing) throw new Error("Pair the Mac recorder first.");
    if (this.pairing.instanceId !== expected.instanceId || this.pairing.workspaceId !== expected.workspaceId) {
      throw new Error("The BB project recording destination changed. Pair the Mac recorder with this project's Margins destination.");
    }
    const status = await this.refresh();
    if (status.instanceId !== expected.instanceId || status.workspaceId !== expected.workspaceId) {
      throw new Error("The Mac recorder destination changed. Restart and pair the Mac recorder before recording.");
    }
    return status;
  }
  async refresh() {
    if (!this.pairing) throw new Error("Pair the Mac recorder first.");
    try {
      const status = await bridgeRequest<NativeStatus>(this.pairing.port, "/v1/status", undefined, this.pairing.token);
      this.currentStatus = status;
      this.currentError = null;
      this.emit();
      return status;
    } catch (cause) {
      this.currentError = cause instanceof Error ? cause.message : String(cause);
      this.emit();
      throw cause;
    }
  }
  async control(action: "start" | "pause" | "resume" | "stop", title?: string) {
    if (!this.pairing) throw new Error("Pair the Mac recorder first.");
    await bridgeRequest(this.pairing.port, `/v1/${action}`, action === "start" ? { title } : {}, this.pairing.token);
    await this.refresh();
  }
  forget() {
    if (this.currentStatus && ["recording", "paused", "saving", "getting_ready"].includes(this.currentStatus.state)) {
      throw new Error("Stop and save the current recording before disconnecting.");
    }
    this.pairing = null;
    this.currentStatus = null;
    this.currentError = null;
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
    sessionStorage.removeItem(STORAGE_KEY);
    this.emit();
  }
}

export const nativeBridgeOwner = new NativeBridgeOwner();
