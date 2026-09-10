// Virtual/loopback device names the app creates internally for system-audio
// capture. These must never appear in the microphone picker as selectable
// inputs — they are capture infrastructure, not real microphones.
const TAP_DEVICE_NAMES = new Set(["margins-tap"]);

export function isTapDevice(name: string): boolean {
  return TAP_DEVICE_NAMES.has(name);
}
