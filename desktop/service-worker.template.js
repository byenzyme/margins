const BUILD_REVISION = '__MARGINS_BUILD_REVISION__';
const CACHE_PREFIX = 'margins-static-';
const CACHE_NAME = `${CACHE_PREFIX}${BUILD_REVISION}`;
const PRECACHE_ENTRIES = __MARGINS_PRECACHE_ENTRIES__;
const PRECACHE_BY_PATH = new Map(PRECACHE_ENTRIES.map(entry => [entry.url, entry]));

function responseContentType(response) {
  return (response.headers.get('content-type') || '').split(';', 1)[0].trim().toLowerCase();
}

async function responseDigest(response) {
  const digest = await crypto.subtle.digest('SHA-256', await response.arrayBuffer());
  return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('');
}

async function textDigest(text) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('');
}

function shellAssetPaths(html) {
  return [...html.matchAll(/(?:src|href)=["']([^"']+)["']/g)]
    .map(match => new URL(match[1], location.origin))
    .filter(url => url.origin === location.origin && url.pathname !== '/')
    .map(url => `${url.pathname}${url.search}`);
}

async function validateResponse(entry, response) {
  if (response.status !== 200) {
    throw new Error(`Precache request failed for ${entry.url}: ${response.status}`);
  }
  const actualType = responseContentType(response);
  if (actualType !== entry.contentType) {
    throw new Error(`Unexpected content type for ${entry.url}: ${actualType || 'missing'}`);
  }
  if (entry.url === '/') {
    const html = await response.clone().text();
    const referencedAssets = shellAssetPaths(html);
    if (referencedAssets.some(asset => !PRECACHE_BY_PATH.has(asset))) {
      throw new Error('Application shell references a different asset graph');
    }
    const normalized = html.replace(
      /<script>window\.__MARGINS_TOKEN__="[0-9A-Fa-f]{64}";<\/script>/,
      ''
    );
    if (await textDigest(normalized) !== entry.digest) {
      throw new Error('Unexpected content digest for application shell');
    }
  } else if (await responseDigest(response.clone()) !== entry.digest) {
    throw new Error(`Unexpected content digest for ${entry.url}`);
  }
}

async function fetchValidated(entry, request) {
  const response = await fetch(request);
  const cacheCopy = response.clone();
  await validateResponse(entry, response.clone());
  return { response, cacheCopy };
}

async function precacheCurrentBuild() {
  const fetched = await Promise.all(PRECACHE_ENTRIES.map(async entry => {
    const request = new Request(new URL(entry.url, location.origin), { cache: 'reload' });
    return { entry, request, ...await fetchValidated(entry, request) };
  }));
  const cache = await caches.open(CACHE_NAME);
  await Promise.all(fetched.map(({ request, cacheCopy }) => cache.put(request, cacheCopy)));
  const graphUrls = new Set(fetched.map(({ request }) => request.url));
  const existingKeys = await cache.keys();
  await Promise.all(
    existingKeys.filter(key => !graphUrls.has(key.url)).map(key => cache.delete(key))
  );
}

self.addEventListener('install', event => {
  event.waitUntil(precacheCurrentBuild());
});

self.addEventListener('fetch', event => {
  const { request } = event;
  const url = new URL(request.url);
  if (url.origin !== location.origin) return;
  if (url.pathname.startsWith('/api/') || url.pathname.startsWith('/ws/')) return;
  if (request.method !== 'GET') return;

  const path = `${url.pathname}${url.search}`;
  const entry = PRECACHE_BY_PATH.get(path);
  if (!entry) return;

  // Revision assets are immutable cache-first. Network recovery occurs only
  // if this revision's protected cache entry is unexpectedly absent.
  const currentCache = caches.open(CACHE_NAME);
  const cached = currentCache.then(cache => cache.match(request));
  const recovery = cached.then(async match => {
    if (match) return { response: match, cacheCopy: null };
    return fetchValidated(entry, request);
  });
  const cacheRecovery = recovery.then(async ({ cacheCopy }) => {
    if (!cacheCopy) return;
    const cache = await currentCache;
    await cache.put(request, cacheCopy);
  });

  event.waitUntil(cacheRecovery.catch(() => undefined));
  event.respondWith(recovery.then(({ response }) => response));
});

self.addEventListener('activate', event => {
  event.waitUntil(
    caches.keys().then(cacheNames => Promise.all(
      cacheNames
        .filter(cacheName => cacheName.startsWith(CACHE_PREFIX) && cacheName !== CACHE_NAME)
        .map(cacheName => caches.delete(cacheName))
    ))
  );
});
