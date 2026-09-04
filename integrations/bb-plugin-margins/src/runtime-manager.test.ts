import { describe, expect, it } from "vitest";
import { runtimeManagerInternals } from "./runtime-manager.js";

describe("Margins runtime manager", () => {
  it("accepts only the exact release asset with a GitHub sha256 digest", () => {
    expect(
      runtimeManagerInternals.selectAsset(
        {
          assets: [
            {
              name: "margins-0.4.9-aarch64-apple-darwin.tar.gz",
              browser_download_url:
                "https://github.com/byenzyme/margins/releases/download/v0.4.9/margins-0.4.9-aarch64-apple-darwin.tar.gz",
              digest: `sha256:${"a".repeat(64)}`,
              size: 42,
            },
          ],
        },
        "margins-0.4.9-aarch64-apple-darwin.tar.gz",
      ),
    ).toEqual({
      url: "https://github.com/byenzyme/margins/releases/download/v0.4.9/margins-0.4.9-aarch64-apple-darwin.tar.gz",
      digest: "a".repeat(64),
      size: 42,
    });

    expect(
      runtimeManagerInternals.selectAsset(
        {
          assets: [
            {
              name: "margins-0.4.9-aarch64-apple-darwin.tar.gz",
              browser_download_url:
                "https://github.com/byenzyme/margins/releases/download/v0.4.9/margins-0.4.9-aarch64-apple-darwin.tar.gz",
              digest: "sha256:not-a-digest",
              size: 42,
            },
          ],
        },
        "margins-0.4.9-aarch64-apple-darwin.tar.gz",
      ),
    ).toBeNull();

    expect(
      runtimeManagerInternals.selectAsset(
        {
          assets: [
            {
              name: "margins-0.4.9-aarch64-apple-darwin.tar.gz",
              browser_download_url: "https://downloads.example.test/margins.tar.gz",
              digest: `sha256:${"a".repeat(64)}`,
              size: 42,
            },
          ],
        },
        "margins-0.4.9-aarch64-apple-darwin.tar.gz",
      ),
    ).toBeNull();
  });

  it("supports only the native recording target shipped by the first release", () => {
    expect(runtimeManagerInternals.targetName("darwin", "arm64")).toBe(
      "aarch64-apple-darwin",
    );
    expect(runtimeManagerInternals.targetName("linux", "x64")).toBeNull();
    expect(runtimeManagerInternals.targetName("darwin", "x64")).toBeNull();
  });

  it("hashes downloaded bytes before extraction", () => {
    expect(runtimeManagerInternals.sha256(new TextEncoder().encode("margins"))).toBe(
      "950aded1ca0bbe3fd906b07899d1c0e67972082e5aab3633e945a87fb6e3c3ab",
    );
  });
});
