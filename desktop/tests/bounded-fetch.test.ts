import assert from "node:assert/strict";
import test from "node:test";

import { fetchJsonWithDeadline } from "../src/lib/bounded-fetch.ts";

test("deadline includes a response body that never settles", async () => {
  let aborted = false;
  const fetchImpl = (async (_input: RequestInfo | URL, init?: RequestInit) => {
    init?.signal?.addEventListener("abort", () => { aborted = true; }, { once: true });
    return {
      ok: true,
      status: 200,
      json: () => new Promise(() => {}),
    } as Response;
  }) as typeof fetch;
  await assert.rejects(
    fetchJsonWithDeadline("http://local.test", {}, { fetchImpl, timeoutMs: 5 }),
    /timed out after 5ms/,
  );
  assert.equal(aborted, true);
});

test("deadline includes headers that never arrive", async () => {
  let aborted = false;
  const fetchImpl = ((_input: RequestInfo | URL, init?: RequestInit) => new Promise<Response>(() => {
    init?.signal?.addEventListener("abort", () => { aborted = true; }, { once: true });
  })) as typeof fetch;
  await assert.rejects(
    fetchJsonWithDeadline("http://local.test", {}, { fetchImpl, timeoutMs: 5 }),
    /timed out after 5ms/,
  );
  assert.equal(aborted, true);
});
