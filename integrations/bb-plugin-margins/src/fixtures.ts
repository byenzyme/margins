import type { LiveSnapshot } from "./contracts.js";

export function desktopSnapshot(
  status:
    | "idle"
    | "starting"
    | "recording"
    | "paused"
    | "finalizing"
    | "needs_attention" = "recording",
): LiveSnapshot {
  return {
    protocol_version: 1,
    server_unix_ms: 1_800_000_000_100,
    session:
      status === "idle"
        ? null
        : {
            session_id: "customer-call",
            status,
            elapsed_ms: 12_345,
            generation: 2,
          },
    health: {
      capture_phase: status,
      tap_status: "ok",
      system_audio_expected: true,
      system_audio_observed: true,
      transcript_freshness: {
        decoded_until_ms: 11_000,
        committed_until_ms: 10_000,
        updated_at_unix_ms: Date.now(),
        age_ms: 100,
      },
    },
    rolling_transcript: [
      { at_ms: 10_000, text: "[00:10] Sam: hello" },
      { at_ms: 11_000, text: "[00:11] Me: following up" },
    ],
    memo_lines: [{ index: 0, at_ms: 12_345, text: "Follow up on pricing" }],
    notepad_revision: "v1-fixture",
  };
}
