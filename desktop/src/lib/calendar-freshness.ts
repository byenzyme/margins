import type { EvidenceFreshness } from "./tauri";

export function calendarFreshnessLabel(freshness: EvidenceFreshness): string | null {
  if (!freshness.stale) return null;
  switch (freshness.status) {
    case "needs_auth":
      return "Calendar connection needs attention";
    case "error":
      return "Calendar refresh failed; showing the last available event";
    case "stale":
      return "Calendar refresh required; showing the last available event";
    default:
      return "Calendar evidence may be stale";
  }
}
