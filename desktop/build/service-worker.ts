import { createHash } from "node:crypto";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";

export const DEV_SERVICE_WORKER_SOURCE = `
self.addEventListener('install', event => {
  event.waitUntil(self.skipWaiting());
});
self.addEventListener('activate', event => {
  event.waitUntil((async () => {
    const names = await caches.keys();
    const hadProductionState = names.some(name => name.startsWith('margins-static-'));
    await Promise.all(
      names.filter(name => name.startsWith('margins-static-')).map(name => caches.delete(name))
    );
    await self.clients.claim();
    if (hadProductionState) {
      const clients = await self.clients.matchAll({ type: 'window' });
      await Promise.all(clients.map(client => client.navigate(client.url)));
    }
  })());
});
`;

type BundleEntry = {
  type: "asset" | "chunk";
  source?: string | Uint8Array;
  code?: string;
};

export type PrecacheBuildEntry = {
  url: string;
  contentType: string;
  digest: string | null;
  revisionDigest: string;
};

function sha256(content: string | Uint8Array): string {
  return createHash("sha256").update(content).digest("hex");
}

function contentType(fileName: string): string {
  switch (path.extname(fileName)) {
    case ".html": return "text/html";
    case ".js":
    case ".mjs": return "application/javascript";
    case ".css": return "text/css";
    case ".json": return "application/json";
    case ".webmanifest": return "application/manifest+json";
    case ".png": return "image/png";
    case ".svg": return "image/svg+xml";
    case ".ico": return "image/x-icon";
    case ".woff": return "font/woff";
    case ".woff2": return "font/woff2";
    default: return "application/octet-stream";
  }
}

async function publicFiles(root: string, prefix = ""): Promise<string[]> {
  const files: string[] = [];
  for (const entry of await readdir(path.join(root, prefix), { withFileTypes: true })) {
    const relative = path.posix.join(prefix, entry.name);
    if (entry.isDirectory()) files.push(...await publicFiles(root, relative));
    else if (entry.isFile()) files.push(relative);
  }
  return files;
}

export function renderServiceWorker(
  template: string,
  buildEntries: PrecacheBuildEntry[],
): { revision: string; source: string } {
  const entries = [...buildEntries].sort((left, right) =>
    left.url < right.url ? -1 : left.url > right.url ? 1 : 0
  );
  const revision = sha256(JSON.stringify(entries));
  const precacheEntries = entries.map(({ revisionDigest: _revisionDigest, ...entry }) => entry);
  const source = template
    .replace("__MARGINS_BUILD_REVISION__", revision)
    .replace("__MARGINS_PRECACHE_ENTRIES__", JSON.stringify(precacheEntries, null, 2));
  if (source.includes("__MARGINS_BUILD_REVISION__") || source.includes("__MARGINS_PRECACHE_ENTRIES__")) {
    throw new Error("Service-worker template placeholders were not replaced exactly once");
  }
  return { revision, source };
}

export async function generateServiceWorker(
  bundle: Record<string, BundleEntry>,
  publicDir: string,
  templatePath: string,
): Promise<{ revision: string; source: string }> {
  const entries: PrecacheBuildEntry[] = [];
  for (const [fileName, output] of Object.entries(bundle)) {
    const content = output.type === "chunk" ? output.code : output.source;
    if (content === undefined) continue;
    const bytes = typeof content === "string" ? content : new Uint8Array(content);
    entries.push({
      url: fileName === "index.html" ? "/" : `/${fileName}`,
      contentType: contentType(fileName),
      digest: sha256(bytes),
      revisionDigest: sha256(bytes),
    });
  }

  for (const fileName of await publicFiles(publicDir)) {
    const bytes = await readFile(path.join(publicDir, fileName));
    entries.push({
      url: `/${fileName}`,
      contentType: contentType(fileName),
      digest: sha256(bytes),
      revisionDigest: sha256(bytes),
    });
  }

  if (!entries.some(entry => entry.url === "/")) {
    throw new Error("Production bundle has no application shell");
  }
  if (entries.length > 64) {
    throw new Error(`Production asset graph has ${entries.length} entries; cache limit is 64`);
  }
  return renderServiceWorker(await readFile(templatePath, "utf8"), entries);
}
