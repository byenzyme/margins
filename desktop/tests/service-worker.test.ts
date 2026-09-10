import assert from "node:assert/strict";
import { createHash, webcrypto } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

import {
  DEV_SERVICE_WORKER_SOURCE,
  generateServiceWorker,
  type PrecacheBuildEntry,
  renderServiceWorker,
} from "../build/service-worker.ts";

const ORIGIN = "https://margins.example";
const SHELL_HTML = '<script src="/assets/app.js"></script><link href="/assets/app.css">';
const VALID_TOKEN_SCRIPT = `<script>window.__MARGINS_TOKEN__="${"a".repeat(64)}";</script>`;
const BODIES: Record<string, { body: string; contentType: string }> = {
  "/": { body: SHELL_HTML, contentType: "text/html" },
  "/assets/app.js": { body: "console.log('app')", contentType: "application/javascript" },
  "/assets/app.css": { body: "body { color: black }", contentType: "text/css" },
  "/manifest.webmanifest": { body: "{}", contentType: "application/manifest+json" },
  "/logo.png": { body: "png-logo", contentType: "image/png" },
  "/icon-192.png": { body: "png-icon", contentType: "image/png" },
  "/margo/welcome.png": { body: "png-welcome", contentType: "image/png" },
  "/margo/mark.png": { body: "png-mark", contentType: "image/png" },
};

type WorkerEvent = {
  request?: Request;
  response?: Promise<Response>;
  lifetime?: Promise<unknown>;
  respondWith(value: Promise<Response>): void;
  waitUntil(value: Promise<unknown>): void;
};

type CacheDouble = ReturnType<typeof makeCache>;

function digest(body: string): string {
  return createHash("sha256").update(body).digest("hex");
}

function buildEntries(bodies = BODIES): PrecacheBuildEntry[] {
  return Object.entries(bodies).map(([url, value]) => ({
    url,
    contentType: value.contentType,
    digest: digest(value.body),
    revisionDigest: digest(value.body),
  }));
}

function requestUrl(request: RequestInfo): string {
  return request instanceof Request ? request.url : new URL(String(request), ORIGIN).href;
}

function makeCache(
  initial: Record<string, { body: string; contentType: string }> = {},
  putGate?: Promise<void>,
) {
  const entries = new Map(
    Object.entries(initial).map(([url, value]) => [
      new URL(url, ORIGIN).href,
      new Response(value.body, { headers: { "content-type": value.contentType } }),
    ]),
  );
  const putInputs: Response[] = [];
  return {
    entries,
    putInputs,
    async put(request: Request, response: Response) {
      if (putGate) await putGate;
      putInputs.push(response);
      const body = await response.arrayBuffer();
      entries.set(request.url, new Response(body, {
        status: response.status,
        headers: response.headers,
      }));
    },
    async match(request: RequestInfo) {
      return entries.get(requestUrl(request))?.clone();
    },
    async keys() {
      return [...entries.keys()].map(url => new Request(url));
    },
    async delete(request: RequestInfo) {
      return entries.delete(requestUrl(request));
    },
  };
}

function deferred() {
  let release!: () => void;
  const promise = new Promise<void>(resolve => { release = resolve; });
  return { promise, release };
}

async function loadWorker(options: {
  entries?: PrecacheBuildEntry[];
  initialCurrent?: Record<string, { body: string; contentType: string }>;
  otherCaches?: Record<string, Record<string, { body: string; contentType: string }>>;
  cachePutGate?: Promise<void>;
  fetchResponse?: (request: Request, defaultFetch: (request: Request) => Response) => Response;
} = {}) {
  const template = await readFile(new URL("../service-worker.template.js", import.meta.url), "utf8");
  const rendered = renderServiceWorker(template, options.entries ?? buildEntries());
  const cacheName = `margins-static-${rendered.revision}`;
  const listeners = new Map<string, (event: WorkerEvent) => void>();
  const namedCaches = new Map<string, CacheDouble>();
  if (options.initialCurrent) {
    namedCaches.set(cacheName, makeCache(options.initialCurrent, options.cachePutGate));
  }
  for (const [name, values] of Object.entries(options.otherCaches ?? {})) {
    namedCaches.set(name, makeCache(values));
  }

  const defaultFetch = (request: Request) => {
    const path = new URL(request.url).pathname;
    const value = BODIES[path];
    return value
      ? new Response(path === "/" ? `${value.body}${VALID_TOKEN_SCRIPT}` : value.body, {
          status: 200,
          headers: { "content-type": value.contentType },
        })
      : new Response(SHELL_HTML, { status: 200, headers: { "content-type": "text/html" } });
  };
  const caches = {
    async match(request: RequestInfo) {
      for (const cache of namedCaches.values()) {
        const match = await cache.match(request);
        if (match) return match;
      }
      return undefined;
    },
    async open(name: string) {
      let cache = namedCaches.get(name);
      if (!cache) {
        cache = makeCache({}, name === cacheName ? options.cachePutGate : undefined);
        namedCaches.set(name, cache);
      }
      return cache;
    },
    async keys() { return [...namedCaches.keys()]; },
    async delete(name: string) { return namedCaches.delete(name); },
  };
  const context = vm.createContext({
    URL,
    Request,
    Response,
    Uint8Array,
    TextEncoder,
    crypto: webcrypto,
    location: { origin: ORIGIN },
    caches,
    fetch: async (request: Request) =>
      (options.fetchResponse ?? ((_request, fallback) => fallback(_request)))(request, defaultFetch),
    self: {
      addEventListener(type: string, listener: (event: WorkerEvent) => void) {
        listeners.set(type, listener);
      },
    },
  });
  vm.runInContext(rendered.source, context, { filename: "generated/sw.js" });

  function lifecycle(type: "install" | "activate") {
    const event: WorkerEvent = {
      respondWith() { throw new Error(`${type} cannot respond`); },
      waitUntil(value) { this.lifetime = value; },
    };
    listeners.get(type)?.(event);
    assert.ok(event.lifetime);
    return event.lifetime;
  }
  function fetchEvent(path: string, method = "GET") {
    const event: WorkerEvent = {
      request: new Request(new URL(path, ORIGIN), { method }),
      respondWith(value) { this.response = value; },
      waitUntil(value) { this.lifetime = value; },
    };
    listeners.get("fetch")?.(event);
    return event;
  }
  return { cacheName, fetchEvent, lifecycle, namedCaches, rendered };
}

test("a changed production graph changes both worker bytes and cache revision", async () => {
  const template = await readFile(new URL("../service-worker.template.js", import.meta.url), "utf8");
  const first = renderServiceWorker(template, buildEntries());
  const changedBodies = { ...BODIES, "/assets/app.js": { ...BODIES["/assets/app.js"], body: "changed" } };
  const second = renderServiceWorker(template, buildEntries(changedBodies));

  assert.notEqual(first.revision, second.revision);
  assert.notEqual(first.source, second.source);
  assert.match(first.source, new RegExp(`BUILD_REVISION = '${first.revision}'`));
  assert.match(second.source, new RegExp(`BUILD_REVISION = '${second.revision}'`));
  assert.match(first.source, /CACHE_NAME = `\$\{CACHE_PREFIX\}\$\{BUILD_REVISION\}`/);
});

test("revision ordering is locale-independent and deterministic", async () => {
  const template = await readFile(new URL("../service-worker.template.js", import.meta.url), "utf8");
  const entries = [
    { url: "/ä", contentType: "text/plain", digest: "3", revisionDigest: "3" },
    { url: "/A", contentType: "text/plain", digest: "1", revisionDigest: "1" },
    { url: "/z", contentType: "text/plain", digest: "2", revisionDigest: "2" },
  ];
  const forward = renderServiceWorker(template, entries);
  const reverse = renderServiceWorker(template, [...entries].reverse());

  assert.equal(forward.revision, reverse.revision);
  assert.equal(forward.source, reverse.source);
  assert.ok(forward.source.indexOf('"url": "/A"') < forward.source.indexOf('"url": "/z"'));
  assert.ok(forward.source.indexOf('"url": "/z"') < forward.source.indexOf('"url": "/ä"'));
});

test("generation includes every required public and bundled frontend asset", async () => {
  const publicDir = new URL("../public", import.meta.url).pathname;
  const templatePath = new URL("../service-worker.template.js", import.meta.url).pathname;
  const generated = await generateServiceWorker({
    "index.html": { type: "asset", source: SHELL_HTML },
    "assets/app.js": { type: "chunk", code: BODIES["/assets/app.js"].body },
    "assets/app.css": { type: "asset", source: BODIES["/assets/app.css"].body },
  }, publicDir, templatePath);
  for (const url of [
    "/manifest.webmanifest",
    "/logo.png",
    "/icon-192.png",
    "/icon-192-maskable.png",
    "/icon-512.png",
    "/icon-512-maskable.png",
    "/margo/welcome.png",
    "/margo/mark.png",
  ]) {
    assert.match(generated.source, new RegExp(`"url": "${url.replace(".", "\\.")}"`));
  }
});

test("first install precaches the complete embedded graph", async () => {
  const worker = await loadWorker();
  await worker.lifecycle("install");
  const cache = worker.namedCaches.get(worker.cacheName)!;
  assert.deepEqual(
    [...cache.entries.keys()].sort(),
    Object.keys(BODIES).map(path => `${ORIGIN}${path}`).sort(),
  );
});

test("200 text/html SPA fallback for a missing JavaScript asset rejects installation and retains v2", async () => {
  const worker = await loadWorker({
    otherCaches: { "margins-static-v2": { "/": BODIES["/"] } },
    fetchResponse(request, fallback) {
      if (new URL(request.url).pathname === "/assets/app.js") {
        return new Response(SHELL_HTML, { status: 200, headers: { "content-type": "text/html" } });
      }
      return fallback(request);
    },
  });

  await assert.rejects(worker.lifecycle("install"), /Unexpected content type.*app\.js.*text\/html/);
  assert.equal(worker.namedCaches.has("margins-static-v2"), true);
});

test("shell normalization rejects an adversarial script-breaking token payload", async () => {
  const adversarialShell = `${SHELL_HTML}${VALID_TOKEN_SCRIPT}<script>globalThis.pwned=true</script>`;
  const worker = await loadWorker({
    fetchResponse(request, fallback) {
      if (new URL(request.url).pathname === "/") {
        return new Response(adversarialShell, {
          status: 200,
          headers: { "content-type": "text/html" },
        });
      }
      return fallback(request);
    },
  });

  await assert.rejects(worker.lifecycle("install"), /Unexpected content digest for application shell/);
});

test("activation removes prior Margins revisions but preserves unrelated caches", async () => {
  const worker = await loadWorker({
    otherCaches: {
      "margins-static-v2": { "/": BODIES["/"] },
      "other-product-cache": { "/logo.png": BODIES["/logo.png"] },
    },
  });
  await worker.lifecycle("install");
  await worker.lifecycle("activate");

  assert.equal(worker.namedCaches.has("margins-static-v2"), false);
  assert.equal(worker.namedCaches.has(worker.cacheName), true);
  assert.equal(worker.namedCaches.has("other-product-cache"), true);
});

test("precache entries are immutable cache-first and never independently revalidated", async () => {
  let fetches = 0;
  const worker = await loadWorker({
    initialCurrent: BODIES,
    fetchResponse(request, fallback) {
      fetches += 1;
      return fallback(request);
    },
  });
  for (const path of ["/", "/assets/app.js", "/logo.png", "/margo/welcome.png", "/margo/mark.png"]) {
    const event = worker.fetchEvent(path);
    assert.ok(event.response);
    await (await event.response).arrayBuffer();
    await event.lifetime;
  }
  assert.equal(fetches, 0);
});

test("client consumption before delayed cache recovery cannot consume its cache clone", async () => {
  const gate = deferred();
  const worker = await loadWorker({ cachePutGate: gate.promise });
  const event = worker.fetchEvent("/assets/app.js");

  assert.equal(await (await event.response!).text(), BODIES["/assets/app.js"].body);
  gate.release();
  await event.lifetime;
  const cache = worker.namedCaches.get(worker.cacheName)!;
  assert.equal(cache.entries.has(`${ORIGIN}/assets/app.js`), true);
  assert.equal(cache.putInputs.at(-1)!.bodyUsed, true);
});

test("API, websocket, cross-origin, non-GET, and unknown paths are not intercepted", async () => {
  const worker = await loadWorker();
  const events = [
    worker.fetchEvent("/api/sessions"),
    worker.fetchEvent("/ws/live"),
    worker.fetchEvent("https://other.example/assets/app.js"),
    worker.fetchEvent("/assets/app.js", "POST"),
    worker.fetchEvent("/not-in-this-build.png"),
  ];
  for (const event of events) {
    assert.equal(event.response, undefined);
    assert.equal(event.lifetime, undefined);
  }
});

async function runDevWorker(cacheNames: string[]) {
  const listeners = new Map<string, (event: WorkerEvent) => void>();
  const deleted: string[] = [];
  let claims = 0;
  let navigations = 0;
  let skipWaiting = 0;
  const self = {
    addEventListener(type: string, listener: (event: WorkerEvent) => void) {
      listeners.set(type, listener);
    },
    async skipWaiting() { skipWaiting += 1; },
    clients: {
      async claim() { claims += 1; },
      async matchAll() {
        return [{
          url: `${ORIGIN}/`,
          async navigate() { navigations += 1; },
        }];
      },
    },
  };
  vm.runInContext(DEV_SERVICE_WORKER_SOURCE, vm.createContext({
    self,
    caches: {
      async keys() { return cacheNames; },
      async delete(name: string) { deleted.push(name); return true; },
    },
  }), { filename: "generated/dev-sw.js" });

  async function dispatch(type: "install" | "activate") {
    const event: WorkerEvent = {
      respondWith() { throw new Error(`${type} cannot respond`); },
      waitUntil(value) { this.lifetime = value; },
    };
    listeners.get(type)?.(event);
    assert.ok(event.lifetime);
    await event.lifetime;
  }
  await dispatch("install");
  await dispatch("activate");
  return { claims, deleted, listeners, navigations, skipWaiting };
}

test("development worker is stable on fresh origins and cleans old production control once", async () => {
  const html = await readFile(new URL("../index.html", import.meta.url), "utf8");
  assert.match(html, /import\.meta\.env\.PROD/);
  assert.match(html, /register\('\/sw\.js', \{ updateViaCache: 'none' \}\)/);
  assert.doesNotMatch(html, /controllerchange|location\.reload\(\)/);
  assert.doesNotMatch(DEV_SERVICE_WORKER_SOURCE, /registration\.unregister/);

  const fresh = await runDevWorker([]);
  assert.equal(fresh.skipWaiting, 1);
  assert.equal(fresh.claims, 1);
  assert.equal(fresh.navigations, 0);
  assert.deepEqual(fresh.deleted, []);
  assert.equal(fresh.listeners.has("fetch"), false);

  const upgraded = await runDevWorker(["margins-static-old", "other-product-cache"]);
  assert.equal(upgraded.claims, 1);
  assert.equal(upgraded.navigations, 1);
  assert.deepEqual(upgraded.deleted, ["margins-static-old"]);
  assert.equal(upgraded.listeners.has("fetch"), false);
});
