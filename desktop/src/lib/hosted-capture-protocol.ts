import type { HostedCaptureProtocol } from "./tauri";

export const HOSTED_CAPTURE_PROTOCOL_VERSION = 2;

export function hostedCaptureProtocolHeaders(): Record<string, string> {
  return { "X-Margins-Capture-Protocol": String(HOSTED_CAPTURE_PROTOCOL_VERSION) };
}

export function validateHostedCaptureProtocol(protocol: HostedCaptureProtocol): HostedCaptureProtocol {
  if (protocol.version !== HOSTED_CAPTURE_PROTOCOL_VERSION
      || protocol.minimumClientVersion > HOSTED_CAPTURE_PROTOCOL_VERSION) {
    throw new Error(
      `Hosted capture protocol mismatch (browser v${HOSTED_CAPTURE_PROTOCOL_VERSION}, server v${protocol.version}). Reload this tab after deployment completes.`,
    );
  }
  return protocol;
}
