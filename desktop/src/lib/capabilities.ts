/**
 * Capabilities — wraps the get_capabilities Tauri/HTTP command and provides
 * a cached accessor for use throughout the app.
 *
 * Shape: { system_audio, obsidian, cli_install, file_dialogs, live_transcription }
 *
 * - On the Tauri path: if the command isn't reachable, return all-true (native
 *   platform assumed fully capable).
 * - On the http path: return all-false defaults on failure (server may not
 *   have native macOS capabilities).
 */

export interface Capabilities {
  system_audio: boolean;
  obsidian: boolean;
  cli_install: boolean;
  file_dialogs: boolean;
  live_transcription: boolean;
}

const ALL_TRUE: Capabilities = {
  system_audio: true,
  obsidian: true,
  cli_install: true,
  file_dialogs: true,
  live_transcription: true,
};

const ALL_FALSE: Capabilities = {
  system_audio: false,
  obsidian: false,
  cli_install: false,
  file_dialogs: false,
  live_transcription: false,
};

let cachedCapabilities: Capabilities | null = null;

/**
 * Fetch and cache capabilities. Callers on the Tauri path receive all-true if
 * the command isn't available. Callers on the http path receive the server
 * response or all-false on error.
 *
 * This is called from within tauri.ts via the resolved backend routing; do not
 * call this directly from render files.
 */
export async function getCapabilities(
  fetcher: (command: string, args?: Record<string, unknown>) => Promise<Capabilities>,
  fallback: Capabilities,
): Promise<Capabilities> {
  if (cachedCapabilities) return cachedCapabilities;
  try {
    const caps = await fetcher("get_capabilities");
    cachedCapabilities = caps;
    return caps;
  } catch {
    cachedCapabilities = fallback;
    return fallback;
  }
}

/**
 * Cached capabilities accessor. Returns the last resolved value, or the
 * provided fallback if capabilities have not been fetched yet.
 */
export function capabilities(fallback: Capabilities = ALL_FALSE): Capabilities {
  return cachedCapabilities ?? fallback;
}

export { ALL_TRUE, ALL_FALSE };
