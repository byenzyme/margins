export interface ScopedWebAuthority {
  recordingId: string;
  sessionName: string;
  ownerId: string;
}

export type HostedAuthorityFailure = "missing" | "owner" | "transient";

export function classifyHostedAuthorityFailure(error: unknown): HostedAuthorityFailure {
  const message = String(error);
  if (/No (?:active )?(?:web|hosted) recording for ID/.test(message)) return "missing";
  if (/Capture authority .* belongs to another browser operation/.test(message)
    || /still has a live browser owner or fresh audio transport/.test(message)) return "owner";
  return "transient";
}

export function missingHostedRecoveryAuthorityIds(
  authorityIds: Iterable<string>,
  discoveredRecoveryIds: Iterable<string>,
): string[] {
  const discovered = new Set(discoveredRecoveryIds);
  return [...authorityIds].filter(recordingId => !discovered.has(recordingId));
}

export async function reconcileHostedAuthorityFailure(
  initialError: unknown,
  reclaim: () => Promise<void>,
  invalidate: (error: unknown) => void,
): Promise<"reclaimed" | "invalidated" | "retry"> {
  let error = initialError;
  let disposition = classifyHostedAuthorityFailure(error);
  if (disposition === "owner") {
    try {
      await reclaim();
      return "reclaimed";
    } catch (reclaimError) {
      error = reclaimError;
      disposition = classifyHostedAuthorityFailure(reclaimError);
    }
  }
  if (disposition === "missing" || disposition === "owner") {
    invalidate(error);
    return "invalidated";
  }
  return "retry";
}

/** Resolve a recovery capability without ever guessing between same-name
 * captures. An explicit recording ID is authoritative; display-name fallback
 * is permitted only when it has exactly one match. */
export function selectWebRecoveryAuthority<T extends ScopedWebAuthority>(
  authorities: ReadonlyMap<string, T>,
  recordingId?: string | null,
  sessionName?: string | null,
): T | null {
  if (recordingId) {
    const authority = authorities.get(recordingId) ?? null;
    return authority && (!sessionName || authority.sessionName === sessionName) ? authority : null;
  }
  const matches = [...authorities.values()].filter(authority => !sessionName || authority.sessionName === sessionName);
  return matches.length === 1 ? matches[0] : null;
}
