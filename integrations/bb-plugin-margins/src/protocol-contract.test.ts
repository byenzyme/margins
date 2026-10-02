import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { CAPTURE_PROTOCOL_VERSION } from "./contracts.js";

describe("hosted capture protocol", () => {
  it("matches the existing Margins project recording service", async () => {
    const rustPath = fileURLToPath(new URL("../../../crates/public/margins-server/src/lib.rs", import.meta.url));
    const source = await readFile(rustPath, "utf8");
    const rustVersion = source.match(/HOSTED_CAPTURE_PROTOCOL_VERSION:\s*u8\s*=\s*(\d+)/)?.[1];
    expect(rustVersion, "Rust capture protocol constant").toBe(String(CAPTURE_PROTOCOL_VERSION));
    expect(CAPTURE_PROTOCOL_VERSION).toBe(3);
  });
});
