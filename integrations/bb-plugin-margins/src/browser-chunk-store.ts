import type { WebChunkTiming } from "../../../desktop/src/lib/web-durable-upload.js";

const DB_NAME = "margins.bb.browser-chunks";
// v2 adds the [storedAtMs, size] index to stores created by v1.
const DB_VERSION = 2;
const STORE = "chunks";
/** [storedAtMs, size] lets the sweep and budget accounting walk keys only,
 * without reading retained audio into memory. */
const META_INDEX = "meta";

/** Per recording, the same ceiling as the in-memory upload queue. Past it a
 * chunk stays memory-only, exactly as before persistence existed: it still
 * uploads normally, but a reload before its acknowledgement loses it and the
 * server records that sequence as a gap. */
export const PERSISTED_CHUNK_BUDGET_BYTES = 64 * 1024 * 1024;
/** A chunk only needs to outlive its acknowledgement. Once the server's
 * 30-minute owner lease lapses the session is finished as incomplete and no
 * longer accepts audio, so anything older than the lease plus two hours (for
 * clock skew and a Stop left pending in a background tab) is raw audio kept
 * for nothing. */
export const PERSISTED_CHUNK_MAX_AGE_MS = (30 + 120) * 60 * 1_000;
const OPEN_DEADLINE_MS = 3_000;
/** A failed open (for example an older tab blocking the upgrade) is retried
 * after this long instead of disabling persistence for the page's lifetime. */
const REOPEN_AFTER_MS = 60_000;

export interface PersistedChunk {
  sessionId: string;
  sequence: number;
  bytes: ArrayBuffer;
  timing: WebChunkTiming;
  storedAtMs: number;
}

export interface BrowserChunkStoreOptions {
  maxBytes?: number;
  maxAgeMs?: number;
  now?: () => number;
  reopenAfterMs?: number;
}

function requestResult<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("IndexedDB request failed"));
  });
}

function transactionDone(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    const fail = () => reject(transaction.error ?? new Error("IndexedDB transaction failed"));
    transaction.onabort = fail;
    transaction.onerror = fail;
  });
}

function sessionRange(sessionId: string) {
  return IDBKeyRange.bound([sessionId, 0], [sessionId, Number.MAX_SAFE_INTEGER]);
}


function isPersistedChunk(value: unknown): value is PersistedChunk {
  const chunk = value as Partial<PersistedChunk> | null;
  return !!chunk && typeof chunk.sessionId === "string" && Number.isSafeInteger(chunk.sequence)
    && Object.prototype.toString.call(chunk.bytes) === "[object ArrayBuffer]" && typeof chunk.storedAtMs === "number"
    && typeof chunk.timing?.capturedStartUnixMs === "number" && typeof chunk.timing?.capturedEndUnixMs === "number";
}

/** Reload-durable copy of browser audio chunks the server has not yet
 * acknowledged, keyed by recording id and global sequence (the server derives
 * the segment from the sequence). Every operation is best effort: when
 * IndexedDB is missing, blocked, or out of quota the capture continues with
 * memory-only retention. Operations run in call order so an acknowledgement can
 * never be overtaken by the write it deletes. */
export class BrowserChunkStore {
  private database: Promise<IDBDatabase | null> | null = null;
  private unavailableUntil = 0;
  private warned = false;
  private readonly reopenAfterMs: number;
  private tail: Promise<unknown> = Promise.resolve();
  // Bytes per recording. Each tab records one recording, so another tab's
  // entries never shrink this page's budget; the age sweep bounds the rest.
  private readonly sizes = new Map<string, Map<number, number>>();
  private readonly degraded = new Set<string>();
  private readonly maxBytes: number;
  private readonly maxAgeMs: number;
  private readonly now: () => number;

  constructor(
    private readonly factory: () => IDBFactory | undefined = () => globalThis.indexedDB,
    options: BrowserChunkStoreOptions = {},
  ) {
    this.maxBytes = Math.max(0, options.maxBytes ?? PERSISTED_CHUNK_BUDGET_BYTES);
    this.maxAgeMs = Math.max(0, options.maxAgeMs ?? PERSISTED_CHUNK_MAX_AGE_MS);
    this.now = options.now ?? Date.now;
    this.reopenAfterMs = Math.max(0, options.reopenAfterMs ?? REOPEN_AFTER_MS);
  }

  /** True after a write failed (quota, eviction) for this recording; it then
   * falls back to memory-only retention for the rest of the capture. */
  isDegraded(sessionId: string) { return this.degraded.has(sessionId); }

  /** Resolves true when the chunk will survive a reload. */
  persist(sessionId: string, sequence: number, chunk: Blob, timing: WebChunkTiming): Promise<boolean> {
    return this.run(false, async (db) => {
      if (this.degraded.has(sessionId)) return false;
      if (this.usedBytes(sessionId) + chunk.size > this.maxBytes) return false;
      try {
        const bytes = await chunk.arrayBuffer();
        const transaction = db.transaction(STORE, "readwrite");
        const entry = { sessionId, sequence, bytes, timing: { ...timing }, storedAtMs: this.now(), size: bytes.byteLength };
        transaction.objectStore(STORE).put(entry);
        await transactionDone(transaction);
        this.track(sessionId, sequence, bytes.byteLength);
        return true;
      } catch {
        this.degraded.add(sessionId);
        return false;
      }
    });
  }

  acknowledge(sessionId: string, sequence: number): Promise<void> {
    return this.run(undefined, async (db) => {
      const transaction = db.transaction(STORE, "readwrite");
      transaction.objectStore(STORE).delete([sessionId, sequence]);
      await transactionDone(transaction);
      this.sizes.get(sessionId)?.delete(sequence);
    });
  }

  /** Drop entries the server has settled: after a segment rotation every
   * sequence below its boundary is either durable or declared missing. */
  forgetBelow(sessionId: string, boundary: number): Promise<void> {
    return this.run(undefined, async (db) => {
      if (boundary <= 0) return;
      const transaction = db.transaction(STORE, "readwrite");
      transaction.objectStore(STORE).delete(IDBKeyRange.bound([sessionId, 0], [sessionId, boundary], false, true));
      await transactionDone(transaction);
      const sizes = this.sizes.get(sessionId);
      for (const sequence of [...sizes?.keys() ?? []]) if (sequence < boundary) sizes!.delete(sequence);
    });
  }

  /** Retained sequences for one recording, ascending, without reading audio. */
  sequences(sessionId: string): Promise<number[]> {
    return this.run([], async (db) => {
      const transaction = db.transaction(STORE, "readonly");
      const keys = await requestResult(transaction.objectStore(STORE).getAllKeys(sessionRange(sessionId)));
      return keys.map((key) => (key as [string, number])[1]).filter(Number.isSafeInteger);
    });
  }

  /** One retained chunk by key, without reading any other audio. */
  get(sessionId: string, sequence: number): Promise<PersistedChunk | null> {
    return this.run(null, async (db) => {
      const transaction = db.transaction(STORE, "readonly");
      const value = await requestResult(transaction.objectStore(STORE).get([sessionId, sequence]));
      return isPersistedChunk(value) ? value : null;
    });
  }

  has(sessionId: string, sequence: number): Promise<boolean> {
    return this.run(false, async (db) => {
      const transaction = db.transaction(STORE, "readonly");
      return await requestResult(transaction.objectStore(STORE).count([sessionId, sequence])) > 0;
    });
  }

  /** Remove abandoned entries. Runs on every plugin page load, not only when
   * a recording starts, so closed tabs do not keep raw audio around. */
  sweep(): Promise<void> {
    return this.run(undefined, (db) => this.sweepDatabase(db));
  }

  /** Unacknowledged chunks for one recording in ascending sequence order. */
  pending(sessionId: string): Promise<PersistedChunk[]> {
    return this.run([], async (db) => {
      const transaction = db.transaction(STORE, "readonly");
      const values = await requestResult(transaction.objectStore(STORE).getAll(sessionRange(sessionId)));
      return values.filter(isPersistedChunk).sort((a, b) => a.sequence - b.sequence);
    });
  }

  /** Drop every retained chunk once a recording is finalized or abandoned. */
  forgetSession(sessionId: string): Promise<void> {
    this.degraded.delete(sessionId);
    return this.run(undefined, async (db) => {
      const transaction = db.transaction(STORE, "readwrite");
      transaction.objectStore(STORE).delete(sessionRange(sessionId));
      await transactionDone(transaction);
      this.sizes.delete(sessionId);
    });
  }

  private usedBytes(sessionId: string) {
    let total = 0;
    for (const size of this.sizes.get(sessionId)?.values() ?? []) total += size;
    return total;
  }

  private track(sessionId: string, sequence: number, size: number) {
    let sizes = this.sizes.get(sessionId);
    if (!sizes) this.sizes.set(sessionId, sizes = new Map());
    sizes.set(sequence, size);
  }

  private run<T>(fallback: T, operation: (db: IDBDatabase) => Promise<T>): Promise<T> {
    const next = this.tail.then(async () => {
      const db = await this.open();
      if (!db) return fallback;
      try { return await operation(db); }
      catch { return fallback; }
    });
    this.tail = next.catch(() => undefined);
    return next;
  }

  private open(): Promise<IDBDatabase | null> {
    if (!this.database && Date.now() < this.unavailableUntil) return Promise.resolve(null);
    const opening = this.database ??= this.openDatabase().then(async (db) => {
      if (db) await this.sweepDatabase(db).catch(() => undefined);
      else {
        if (this.database === opening) this.database = null;
        this.unavailableUntil = Date.now() + this.reopenAfterMs;
        if (!this.warned) {
          this.warned = true;
          console.warn("Margins: browser audio is kept in memory only until IndexedDB becomes available; a reload may lose unsent audio.");
        }
      }
      return db;
    });
    return opening;
  }

  /** A newer page version or the browser closed the connection; reopen on
   * the next operation instead of failing every later write silently. */
  private closed(db: IDBDatabase) {
    db.close();
    void this.database?.then((current) => { if (current === db) this.database = null; });
  }

  private openDatabase(): Promise<IDBDatabase | null> {
    return new Promise((resolve) => {
      let settled = false;
      const finish = (db: IDBDatabase | null) => {
        if (settled) { db?.close(); return; }
        settled = true;
        clearTimeout(timer);
        resolve(db);
      };
      // Some private-browsing modes leave open() pending forever.
      const timer = setTimeout(() => finish(null), OPEN_DEADLINE_MS);
      try {
        const factory = this.factory();
        if (!factory) { finish(null); return; }
        const request = factory.open(DB_NAME, DB_VERSION);
        request.onupgradeneeded = () => {
          const store = request.result.objectStoreNames.contains(STORE)
            ? request.transaction!.objectStore(STORE)
            : request.result.createObjectStore(STORE, { keyPath: ["sessionId", "sequence"] });
          if (!store.indexNames.contains(META_INDEX)) store.createIndex(META_INDEX, ["storedAtMs", "size"]);
          // v1 entries carry no size, so the index would never sweep them.
          const backfill = store.openCursor();
          backfill.onsuccess = () => {
            const cursor = backfill.result;
            if (!cursor) return;
            const value = cursor.value as Partial<PersistedChunk> & { size?: unknown };
            if (typeof value.size !== "number") {
              if (isPersistedChunk(value)) cursor.update({ ...value, size: value.bytes.byteLength });
              else cursor.delete();
            }
            cursor.continue();
          };
        };
        request.onsuccess = () => {
          const db = request.result;
          db.onversionchange = () => this.closed(db);
          db.onclose = () => this.closed(db);
          finish(db);
        };
        request.onerror = () => finish(null);
        request.onblocked = () => finish(null);
      } catch {
        // SecurityError in sandboxed or storage-partitioned contexts.
        finish(null);
      }
    });
  }

  /** Delete abandoned entries and rebuild the byte accounting from keys. */
  private async sweepDatabase(db: IDBDatabase) {
    const cutoff = this.now() - this.maxAgeMs;
    const transaction = db.transaction(STORE, "readwrite");
    const store = transaction.objectStore(STORE);
    const sizes = new Map<string, Map<number, number>>();
    const cursorRequest = store.index(META_INDEX).openKeyCursor();
    cursorRequest.onsuccess = () => {
      const cursor = cursorRequest.result;
      if (!cursor) return;
      const [storedAtMs, size] = cursor.key as [number, number];
      const [sessionId, sequence] = cursor.primaryKey as [string, number];
      if (storedAtMs < cutoff) store.delete(cursor.primaryKey);
      else {
        let session = sizes.get(sessionId);
        if (!session) sizes.set(sessionId, session = new Map());
        session.set(sequence, size);
      }
      cursor.continue();
    };
    await transactionDone(transaction);
    this.sizes.clear();
    for (const [sessionId, session] of sizes) this.sizes.set(sessionId, session);
  }
}
