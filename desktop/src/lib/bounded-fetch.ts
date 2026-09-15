export interface BoundedFetchOptions {
  timeoutMs?: number;
  fetchImpl?: typeof fetch;
}

function asError(value: unknown): Error {
  return value instanceof Error ? value : new Error(String(value));
}

/**
 * Bound the complete HTTP exchange, including consumption of the response
 * body. Aborting is only a local resource hint: callers must retain request
 * identity and reconcile because the peer may already have committed.
 */
export async function fetchJsonWithDeadline<T>(
  input: RequestInfo | URL,
  init: RequestInit,
  options: BoundedFetchOptions = {},
): Promise<T> {
  const timeoutMs = Math.max(1, options.timeoutMs ?? 10_000);
  const controller = new AbortController();
  const upstream = init.signal;
  const abortFromUpstream = () => controller.abort(upstream?.reason);
  if (upstream?.aborted) abortFromUpstream();
  else upstream?.addEventListener("abort", abortFromUpstream, { once: true });

  let timer: ReturnType<typeof setTimeout> | null = null;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      const error = new Error(`Request timed out after ${timeoutMs}ms`);
      controller.abort(error);
      reject(error);
    }, timeoutMs);
  });
  const exchange = (async () => {
    const response = await (options.fetchImpl ?? fetch)(input, { ...init, signal: controller.signal });
    const body = await response.json();
    if (!response.ok) throw new Error(`Request failed (${response.status})`);
    return body as T;
  })();
  void exchange.catch(() => undefined);
  try {
    return await Promise.race([exchange, deadline]);
  } catch (error) {
    throw asError(error);
  } finally {
    if (timer) clearTimeout(timer);
    upstream?.removeEventListener("abort", abortFromUpstream);
  }
}
