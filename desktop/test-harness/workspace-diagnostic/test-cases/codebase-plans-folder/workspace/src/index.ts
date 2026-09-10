import { resolveConflicts } from "./sync/conflicts";

export async function syncNow() {
  const result = await resolveConflicts();
  return result.ok ? "ok" : "warn";
}
