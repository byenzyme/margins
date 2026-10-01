import { createRequire } from "node:module";

// The SDK's host test bundle still contains CommonJS dependencies. Vitest runs
// its test modules as ESM, so give that bundle Node's normal require loader.
Object.assign(globalThis, { require: createRequire(import.meta.url) });
