export type WebMicrophoneFailureCategory =
  | "permission-denied"
  | "policy-blocked"
  | "no-device"
  | "selected-device-unavailable"
  | "hardware-unavailable"
  | "aborted"
  | "unsupported"
  | "insecure-context";

export type WebMicrophonePermissionState =
  | "granted"
  | "denied"
  | "prompt"
  | "unknown"
  | "unsupported";

export interface WebMicrophoneEnvironment {
  standalone: boolean;
  ios: boolean;
}

export interface WebMicrophoneDevice {
  deviceId: string;
  label: string;
}

const DEVICE_STORAGE_KEY = "margins.web.microphoneDeviceId";
const MAX_DEVICE_ID_LENGTH = 1024;

export class WebMicrophoneFailure extends Error {
  readonly category: WebMicrophoneFailureCategory;

  constructor(category: WebMicrophoneFailureCategory, message?: string) {
    super(message ?? category);
    this.name = "WebMicrophoneFailure";
    this.category = category;
  }
}

export function isWebMicrophoneFailure(value: unknown): value is WebMicrophoneFailure {
  return value instanceof WebMicrophoneFailure
    || (value instanceof Error
      && value.name === "WebMicrophoneFailure"
      && typeof (value as Partial<WebMicrophoneFailure>).category === "string");
}

export function classifyWebMicrophoneFailure(
  error: unknown,
  options: { selectedDevice?: boolean } = {},
): WebMicrophoneFailure {
  if (isWebMicrophoneFailure(error)) return error;

  const name = error instanceof DOMException || error instanceof Error ? error.name : "";
  switch (name) {
    case "NotAllowedError":
      return new WebMicrophoneFailure("permission-denied");
    case "SecurityError":
      return new WebMicrophoneFailure("policy-blocked");
    case "NotFoundError":
    case "DevicesNotFoundError":
      return new WebMicrophoneFailure(
        options.selectedDevice ? "selected-device-unavailable" : "no-device",
        undefined,
      );
    case "OverconstrainedError":
    case "ConstraintNotSatisfiedError":
      return new WebMicrophoneFailure("selected-device-unavailable");
    case "NotReadableError":
    case "TrackStartError":
      return new WebMicrophoneFailure("hardware-unavailable");
    case "AbortError":
      return new WebMicrophoneFailure("aborted");
    default:
      return new WebMicrophoneFailure("hardware-unavailable");
  }
}

export function webMicrophoneEnvironment(): WebMicrophoneEnvironment {
  if (typeof window === "undefined" || typeof navigator === "undefined") {
    return { standalone: false, ios: false };
  }
  const navigatorWithStandalone = navigator as Navigator & { standalone?: boolean };
  const ios = /iPad|iPhone|iPod/.test(navigator.userAgent)
    || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
  return {
    standalone: window.matchMedia?.("(display-mode: standalone)").matches === true
      || navigatorWithStandalone.standalone === true,
    ios,
  };
}

export function webMicrophoneRecoveryGuidance(environment: WebMicrophoneEnvironment): string {
  if (environment.ios && environment.standalone) {
    return "Open iOS Settings, find Margins in the installed apps list, allow Microphone access, then return here. If it remains blocked, open Margins in Safari and allow microphone access for this site.";
  }
  if (environment.ios) {
    return "In Safari, open this site's Website Settings, set Microphone to Allow, then return to Margins and try again.";
  }
  if (environment.standalone) {
    return "Open your browser's site permissions for Margins from the installed app menu, allow Microphone, then return here and check again.";
  }
  return "Open this site's permissions from the address bar, allow Microphone, then return to Margins and try again.";
}

export function webMicrophonePolicyBlockedGuidance(environment: WebMicrophoneEnvironment): string {
  const context = environment.standalone
    ? "Try opening Margins directly in a browser tab as a top-level HTTPS page."
    : "Open Margins directly as a top-level HTTPS page.";
  return `The browser, an administrator, or an embedding Permissions Policy has disabled microphone access. ${context} Site permission changes alone may not override this policy.`;
}

export function selectWebRecorderMimeType(
  isTypeSupported: (mimeType: string) => boolean,
): string | null {
  return ["audio/webm;codecs=opus", "audio/webm"].find(isTypeSupported) ?? null;
}

export function webMicrophoneSupported(): boolean {
  return typeof window !== "undefined"
    && typeof navigator !== "undefined"
    && typeof navigator.mediaDevices?.getUserMedia === "function"
    && typeof window.MediaRecorder !== "undefined"
    && typeof window.MediaRecorder.isTypeSupported === "function"
    && selectWebRecorderMimeType(mimeType => window.MediaRecorder.isTypeSupported(mimeType)) !== null;
}

export function webMicrophoneConstraints(deviceId?: string | null): MediaStreamConstraints {
  return deviceId
    ? { audio: { deviceId: { exact: deviceId } } }
    : { audio: true };
}

export async function acquireWebMicrophone(deviceId?: string | null): Promise<MediaStream> {
  if (typeof window === "undefined" || !window.isSecureContext) {
    throw new WebMicrophoneFailure("insecure-context");
  }
  if (!webMicrophoneSupported()) {
    throw new WebMicrophoneFailure("unsupported");
  }
  try {
    return await navigator.mediaDevices.getUserMedia(webMicrophoneConstraints(deviceId));
  } catch (error) {
    throw classifyWebMicrophoneFailure(error, { selectedDevice: Boolean(deviceId) });
  }
}

export async function enumerateWebMicrophones(): Promise<WebMicrophoneDevice[]> {
  if (typeof navigator === "undefined" || !navigator.mediaDevices?.enumerateDevices) return [];
  const devices = await navigator.mediaDevices.enumerateDevices();
  const seen = new Set<string>();
  const microphones: WebMicrophoneDevice[] = [];
  for (const device of devices) {
    if (device.kind !== "audioinput"
      || !device.deviceId
      || device.deviceId === "default"
      || seen.has(device.deviceId)) continue;
    seen.add(device.deviceId);
    microphones.push({
      deviceId: device.deviceId,
      label: device.label.trim() || `Microphone ${microphones.length + 1}`,
    });
  }
  return microphones;
}

export function getPreferredWebMicrophoneDeviceId(): string | null {
  if (typeof window === "undefined") return null;
  try {
    const value = window.localStorage.getItem(DEVICE_STORAGE_KEY);
    return value && value.trim().length > 0 && value.length <= MAX_DEVICE_ID_LENGTH ? value : null;
  } catch {
    return null;
  }
}

export function setPreferredWebMicrophoneDeviceId(deviceId: string | null): void {
  if (typeof window === "undefined") return;
  try {
    if (deviceId && deviceId.trim().length > 0 && deviceId.length <= MAX_DEVICE_ID_LENGTH) {
      window.localStorage.setItem(DEVICE_STORAGE_KEY, deviceId);
    } else {
      window.localStorage.removeItem(DEVICE_STORAGE_KEY);
    }
  } catch {
    // Storage may be unavailable in private/locked-down contexts. The current
    // page still uses the selected device until it is reloaded.
  }
}

type PermissionSubscriber = (state: WebMicrophonePermissionState) => void;

class WebMicrophonePermissionController {
  private current: WebMicrophonePermissionState = "unknown";
  private lastFailure: WebMicrophoneFailureCategory | null = null;
  private permissionStatus: PermissionStatus | null = null;
  private readonly subscribers = new Set<PermissionSubscriber>();
  private listeningForVisibility = false;

  get state(): WebMicrophonePermissionState {
    return this.current;
  }

  get failureCategory(): WebMicrophoneFailureCategory | null {
    return this.lastFailure;
  }

  private setState(state: WebMicrophonePermissionState): WebMicrophonePermissionState {
    if (this.current === state) return state;
    this.current = state;
    for (const subscriber of this.subscribers) subscriber(state);
    return state;
  }

  private onPermissionChange = () => {
    if (this.permissionStatus) this.setState(this.permissionStatus.state);
  };

  private onVisibilityChange = () => {
    if (document.visibilityState === "visible") void this.refresh();
  };

  async refresh(): Promise<WebMicrophonePermissionState> {
    if (typeof window !== "undefined" && !window.isSecureContext) return this.setState("unsupported");
    if (!webMicrophoneSupported()) return this.setState("unsupported");
    if (!navigator.permissions?.query) return this.current === "granted" || this.current === "denied"
      ? this.current
      : this.setState("unknown");
    try {
      const status = await navigator.permissions.query({ name: "microphone" } as PermissionDescriptor);
      if (this.permissionStatus !== status) {
        this.permissionStatus?.removeEventListener("change", this.onPermissionChange);
        this.permissionStatus = status;
        status.addEventListener("change", this.onPermissionChange);
      }
      return this.setState(status.state);
    } catch {
      // Safari (notably iOS) does not implement microphone permission queries.
      // Permission is still requestable and must never be gated on this API.
      return this.current === "granted" || this.current === "denied"
        ? this.current
        : this.setState("unknown");
    }
  }

  async request(): Promise<WebMicrophonePermissionState> {
    let stream: MediaStream | null = null;
    try {
      stream = await acquireWebMicrophone();
      this.lastFailure = null;
      return this.setState("granted");
    } catch (error) {
      const failure = classifyWebMicrophoneFailure(error);
      this.lastFailure = failure.category;
      if (failure.category === "permission-denied" || failure.category === "policy-blocked") {
        return this.setState("denied");
      }
      if (failure.category === "unsupported" || failure.category === "insecure-context") {
        return this.setState("unsupported");
      }
      throw failure;
    } finally {
      stream?.getTracks().forEach(track => track.stop());
    }
  }

  noteGranted(): void {
    this.lastFailure = null;
    this.setState("granted");
  }

  noteFailure(failure: WebMicrophoneFailure): void {
    this.lastFailure = failure.category;
    if (failure.category === "permission-denied" || failure.category === "policy-blocked") {
      this.setState("denied");
    }
  }

  subscribe(subscriber: PermissionSubscriber): () => void {
    this.subscribers.add(subscriber);
    subscriber(this.current);
    if (typeof document !== "undefined" && !this.listeningForVisibility) {
      document.addEventListener("visibilitychange", this.onVisibilityChange);
      this.listeningForVisibility = true;
    }
    void this.refresh();
    return () => {
      this.subscribers.delete(subscriber);
      if (this.subscribers.size === 0 && this.listeningForVisibility) {
        document.removeEventListener("visibilitychange", this.onVisibilityChange);
        this.listeningForVisibility = false;
      }
    };
  }
}

export const webMicrophonePermission = new WebMicrophonePermissionController();

export function subscribeWebMicrophoneDevices(callback: () => void): () => void {
  const mediaDevices = typeof navigator !== "undefined" ? navigator.mediaDevices : undefined;
  if (!mediaDevices?.addEventListener) return () => {};
  mediaDevices.addEventListener("devicechange", callback);
  return () => mediaDevices.removeEventListener("devicechange", callback);
}
