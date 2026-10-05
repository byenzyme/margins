import "fake-indexeddb/auto";
import { IDBFactory } from "fake-indexeddb";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BrowserChunkStore } from "./browser-chunk-store.js";

const timing = (start: number) => ({ capturedStartUnixMs: start, capturedEndUnixMs: start + 3_000 });
const text = (bytes: ArrayBuffer) => new TextDecoder().decode(bytes);

describe("browser chunk store", () => {
  afterEach(() => vi.restoreAllMocks());

  it("survives a new page instance and returns chunks in sequence order", async () => {
    const factory = new IDBFactory();
    const before = new BrowserChunkStore(() => factory);
    await before.persist("rec-1", 2, new Blob(["two"]), timing(2));
    await before.persist("rec-1", 1, new Blob(["one"]), timing(1));
    await before.persist("rec-2", 1, new Blob(["other"]), timing(1));

    const after = new BrowserChunkStore(() => factory);
    const pending = await after.pending("rec-1");
    expect(pending.map((chunk) => [chunk.sequence, text(chunk.bytes)])).toEqual([[1, "one"], [2, "two"]]);
    expect(pending[0]!.timing).toEqual(timing(1));
  });

  it("deletes a chunk once acknowledged, even when the write is still in flight", async () => {
    const store = new BrowserChunkStore(() => new IDBFactory());
    void store.persist("rec-1", 0, new Blob(["zero"]), timing(0));
    await store.acknowledge("rec-1", 0);
    expect(await store.pending("rec-1")).toEqual([]);
  });

  it("keeps chunks past the byte budget memory-only and admits more after acknowledgements", async () => {
    const store = new BrowserChunkStore(() => new IDBFactory(), { maxBytes: 8 });
    expect(await store.persist("rec-1", 0, new Blob(["12345"]), timing(0))).toBe(true);
    expect(await store.persist("rec-1", 1, new Blob(["67890"]), timing(1))).toBe(false);
    expect(store.isDegraded("rec-1")).toBe(false);
    await store.acknowledge("rec-1", 0);
    expect(await store.persist("rec-1", 2, new Blob(["abcde"]), timing(2))).toBe(true);
    expect((await store.pending("rec-1")).map((chunk) => chunk.sequence)).toEqual([2]);
  });

  it("falls back to memory-only retention after a quota error", async () => {
    const store = new BrowserChunkStore(() => new IDBFactory());
    await store.pending("rec-1");
    const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => {
      throw new DOMException("Quota exceeded", "QuotaExceededError");
    });
    expect(await store.persist("rec-1", 0, new Blob(["zero"]), timing(0))).toBe(false);
    expect(store.isDegraded("rec-1")).toBe(true);
    put.mockRestore();
    // The degraded recording does not resume partial persistence.
    expect(await store.persist("rec-1", 1, new Blob(["one"]), timing(1))).toBe(false);
    expect(await store.pending("rec-1")).toEqual([]);
    // Other recordings are unaffected.
    expect(await store.persist("rec-2", 0, new Blob(["zero"]), timing(0))).toBe(true);
  });

  it("is a no-op when IndexedDB is unavailable", async () => {
    const missing = new BrowserChunkStore(() => undefined);
    expect(await missing.persist("rec-1", 0, new Blob(["zero"]), timing(0))).toBe(false);
    expect(await missing.pending("rec-1")).toEqual([]);
    await expect(missing.acknowledge("rec-1", 0)).resolves.toBeUndefined();
    await expect(missing.forgetSession("rec-1")).resolves.toBeUndefined();

    const blocked = new BrowserChunkStore(() => { throw new DOMException("denied", "SecurityError"); });
    expect(await blocked.persist("rec-1", 0, new Blob(["zero"]), timing(0))).toBe(false);
  });

  it("forgets a finalized recording and sweeps abandoned ones by age", async () => {
    const factory = new IDBFactory();
    let now = 1_000;
    const store = new BrowserChunkStore(() => factory, { maxAgeMs: 10_000, now: () => now });
    await store.persist("done", 0, new Blob(["a"]), timing(0));
    await store.persist("abandoned", 0, new Blob(["b"]), timing(0));
    now = 6_000;
    await store.persist("fresh", 0, new Blob(["c"]), timing(0));
    await store.forgetSession("done");
    expect(await store.pending("done")).toEqual([]);

    now = 12_000;
    const reloaded = new BrowserChunkStore(() => factory, { maxAgeMs: 10_000, now: () => now });
    expect(await reloaded.pending("abandoned")).toEqual([]);
    expect((await reloaded.pending("fresh")).map((chunk) => text(chunk.bytes))).toEqual(["c"]);
  });
});
