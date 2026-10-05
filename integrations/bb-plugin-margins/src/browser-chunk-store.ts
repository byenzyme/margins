import type { WebChunkTiming } from "../../../desktop/src/lib/web-durable-upload.js";

const DB_NAME = "margins.bb.browser-chunks";
const DB_VERSION = 1;
const STORE = "chunks";

/** Same ceiling as the in-memory upload queue. Past it a chunk stays
 * memory-only, exactly as before persistence existed: it still uploads
 * normally, but a reload before its acknowledgement loses it and the server
 * records that sequence as a gap. */
export const PERSISTED_CHUNK_BUDGET_BYTES = 64 * 1024 * 1024;
/** The server's owner lease is 30 minutes; after it expires the session is
 * finished as incomplete and retained bytes can no longer be appended. Keep a
 * wide margin so a long pause plus a slow reload is never swept early. */
export const PERSISTED_CHUNK_MAX_AGE_MS = 6 * 60 * 60 * 1_000;
const OPEN_DEADLINE_MS = 3_000;

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

function sizeKey(sessionId: string, sequence: number) { return `${sessionId}\u0000${sequence}`; }

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
  private tail: Promise<unknown> = Promise.resolve();
  // Approximate per-page accounting; another tab on the same origin can
  // briefly exceed the budget until its own acknowledgements delete entries.
  private readonly sizes = new Map<string, number>();
  private usedBytes = 0;
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
  }

  /** True after a write failed (quota, eviction) for this recording; it then
   * falls back to memory-only retention for the rest of the capture. */
  isDegraded(sessionId: string) { return this.degraded.has(sessionId); }

  /** Resolves true when the chunk will survive a reload. */
  persist(sessionId: string, sequence: number, chunk: Blob, timing: WebChunkTiming): Promise<boolean> {
    return this.run(false, async (db) => {
      if (this.degraded.has(sessionId)) return false;
      if (this.usedBytes + chunk.size > this.maxBytes) return false;
      try {
        const bytes = await chunk.arrayBuffer();
        const transaction = db.transaction(STORE, "readwrite");
        const entry: PersistedChunk = { sessionId, sequence, bytes, timing: { ...timing }, storedAtMs: this.now() };
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
      this.untrack(sizeKey(sessionId, sequence));
    });
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
      const prefix = sizeKey(sessionId, 0).slice(0, -1);
      for (const key of [...this.sizes.keys()]) if (key.startsWith(prefix)) this.untrack(key);
    });
  }

  private track(sessionId: string, sequence: number, size: number) {
    const key = sizeKey(sessionId, sequence);
    this.untrack(key);
    this.sizes.set(key, size);
    this.usedBytes += size;
  }

  private untrack(key: string) {
    const prior = this.sizes.get(key);
    if (prior === undefined) return;
    this.sizes.delete(key);
    this.usedBytes -= prior;
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
    this.database ??= this.openDatabase().then(async (db) => {
      if (db) await this.sweep(db).catch(() => undefined);
      return db;
    });
    return this.database;
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
          if (!request.result.objectStoreNames.contains(STORE)) {
            request.result.createObjectStore(STORE, { keyPath: ["sessionId", "sequence"] });
          }
        };
        request.onsuccess = () => {
          const db = request.result;
          db.onversionchange = () => db.close();
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

  /** Delete abandoned entries and rebuild the byte accounting. */
  private async sweep(db: IDBDatabase) {
    const cutoff = this.now() - this.maxAgeMs;
    const transaction = db.transaction(STORE, "readwrite");
    const cursorRequest = transaction.objectStore(STORE).openCursor();
    cursorRequest.onsuccess = () => {
      const cursor = cursorRequest.result;
      if (!cursor) return;
      const value = cursor.value as unknown;
      if (!isPersistedChunk(value) || value.storedAtMs < cutoff) cursor.delete();
      else this.track(value.sessionId, value.sequence, value.bytes.byteLength);
      cursor.continue();
    };
    await transactionDone(transaction);
  }
}
