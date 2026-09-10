export function shouldShowCaptureDeviceToast(
  tapStatus: string | null | undefined,
  tapWarning: string | null | undefined,
): boolean {
  const status = tapStatus || "ok";
  const computerAudioNeedsAttention = status !== "ok" && status !== "not_expected";
  return !computerAudioNeedsAttention && !tapWarning;
}
