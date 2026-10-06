import { execFile as execFileCallback } from "node:child_process";
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { describe, expect, it, vi } from "vitest";
import { createRuntimeManager, runtimeManagerInternals } from "./runtime-manager.js";

const execFile = promisify(execFileCallback);

async function archiveFixture(root: string, executables: string[]) {
  const unpacked = join(root, "unpacked");
  await mkdir(unpacked);
  for (const name of executables) {
    const path = join(unpacked, name);
    await writeFile(path, `#!/bin/sh\necho ${name}\n`);
    await chmod(path, 0o755);
  }
  const archive = join(root, "fixture.tar.gz");
  await execFile("/usr/bin/tar", ["-czf", archive, "-C", unpacked, "."]);
  return new Uint8Array(await readFile(archive));
}

function releaseFetch(archive: Uint8Array) {
  const name = "margins-0.4.16-x86_64-unknown-linux-gnu.tar.gz";
  const url = `https://github.com/byenzyme/margins/releases/download/v0.4.16/${name}`;
  return vi.fn(async (input: string | URL | Request) => String(input) === url
    ? new Response(Uint8Array.from(archive).buffer, { status: 200 })
    : new Response(JSON.stringify({ assets: [{
      name,
      browser_download_url: url,
      digest: `sha256:${runtimeManagerInternals.sha256(archive)}`,
      size: archive.byteLength,
    }] }), { status: 200 }));
}

describe("Margins runtime manager", () => {
  it("accepts only the exact release asset with a GitHub sha256 digest", () => {
    expect(
      runtimeManagerInternals.selectAsset(
        {
          assets: [
            {
              name: "margins-0.4.16-aarch64-apple-darwin.tar.gz",
              browser_download_url:
                "https://github.com/byenzyme/margins/releases/download/v0.4.16/margins-0.4.16-aarch64-apple-darwin.tar.gz",
              digest: `sha256:${"a".repeat(64)}`,
              size: 42,
            },
          ],
        },
        "margins-0.4.16-aarch64-apple-darwin.tar.gz",
      ),
    ).toEqual({
      url: "https://github.com/byenzyme/margins/releases/download/v0.4.16/margins-0.4.16-aarch64-apple-darwin.tar.gz",
      digest: "a".repeat(64),
      size: 42,
    });

    expect(
      runtimeManagerInternals.selectAsset(
        {
          assets: [
            {
              name: "margins-0.4.16-aarch64-apple-darwin.tar.gz",
              browser_download_url:
                "https://github.com/byenzyme/margins/releases/download/v0.4.16/margins-0.4.16-aarch64-apple-darwin.tar.gz",
              digest: "sha256:not-a-digest",
              size: 42,
            },
          ],
        },
        "margins-0.4.16-aarch64-apple-darwin.tar.gz",
      ),
    ).toBeNull();

    expect(
      runtimeManagerInternals.selectAsset(
        {
          assets: [
            {
              name: "margins-0.4.16-aarch64-apple-darwin.tar.gz",
              browser_download_url: "https://downloads.example.test/margins.tar.gz",
              digest: `sha256:${"a".repeat(64)}`,
              size: 42,
            },
          ],
        },
        "margins-0.4.16-aarch64-apple-darwin.tar.gz",
      ),
    ).toBeNull();
  });

  it("selects the release target for supported project machines", () => {
    expect(runtimeManagerInternals.targetName("darwin", "arm64")).toBe(
      "aarch64-apple-darwin",
    );
    expect(runtimeManagerInternals.targetName("linux", "x64")).toBe("x86_64-unknown-linux-gnu");
    expect(runtimeManagerInternals.targetName("darwin", "x64")).toBeNull();
  });

  it("hashes downloaded bytes before extraction", () => {
    expect(runtimeManagerInternals.sha256(new TextEncoder().encode("margins"))).toBe(
      "950aded1ca0bbe3fd906b07899d1c0e67972082e5aab3633e945a87fb6e3c3ab",
    );
  });

  it("installs both verified executables in a fresh scoped home and reuses them", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-bb-install-"));
    try {
      const archive = await archiveFixture(root, ["margins", "margins-server"]);
      const fetchImpl = releaseFetch(archive) as unknown as typeof fetch;
      const dataDir = join(root, "plugin-data");
      const cliBinDir = join(root, "bin");
      const manager = createRuntimeManager({
        env: { MARGINS_CLI_BIN_DIR: cliBinDir }, fetchImpl, homeDir: root, platform: "linux", arch: "x64",
      });
      const server = await manager.ensureProjectServer({ dataDir });
      expect(server).toBe(join(dataDir, "runtime", "v0.4.16", "margins-server"));
      expect((await execFile(server)).stdout.trim()).toBe("margins-server");
      expect((await execFile(join(cliBinDir, "margins"))).stdout.trim()).toBe("margins");
      expect(await readFile(`${join(cliBinDir, "margins")}.bb-margins-managed`, "utf8")).toContain("version=0.4.16");
      await manager.ensureProjectServer({ dataDir });
      expect(vi.mocked(fetchImpl)).toHaveBeenCalledTimes(2);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("installs the bundled enzyme engine beside the runtime and under the CLI prefix", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-bb-engine-"));
    try {
      const archive = await archiveFixture(root, ["margins", "margins-server", "enzyme"]);
      const dataDir = join(root, "plugin-data");
      const cliBinDir = join(root, "prefix", "bin");
      const manager = createRuntimeManager({
        env: { MARGINS_CLI_BIN_DIR: cliBinDir },
        fetchImpl: releaseFetch(archive) as unknown as typeof fetch,
        homeDir: root, platform: "linux", arch: "x64",
      });
      await manager.ensureProjectServer({ dataDir });
      const runtimeEngine = join(dataDir, "runtime", "v0.4.16", "enzyme");
      expect((await execFile(runtimeEngine)).stdout.trim()).toBe("enzyme");
      const cliEngine = join(root, "prefix", "libexec", "margins", "enzyme");
      expect((await execFile(cliEngine)).stdout.trim()).toBe("enzyme");
      // Never on PATH next to margins, where a user's own enzyme may live.
      await expect(readFile(join(cliBinDir, "enzyme"))).rejects.toMatchObject({ code: "ENOENT" });
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("leaves the engine alone when the CLI path belongs to someone else", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-bb-engine-"));
    try {
      const archive = await archiveFixture(root, ["margins", "margins-server", "enzyme"]);
      const cliBinDir = join(root, "prefix", "bin");
      await mkdir(cliBinDir, { recursive: true });
      await writeFile(join(cliBinDir, "margins"), "#!/bin/sh\necho user\n", { mode: 0o755 });
      const manager = createRuntimeManager({
        env: { MARGINS_CLI_BIN_DIR: cliBinDir },
        fetchImpl: releaseFetch(archive) as unknown as typeof fetch,
        homeDir: root, platform: "linux", arch: "x64",
      });
      await manager.ensureProjectServer({ dataDir: join(root, "plugin-data") });
      await expect(readFile(join(root, "prefix", "libexec", "margins", "enzyme"))).rejects.toMatchObject({ code: "ENOENT" });
      expect((await execFile(join(cliBinDir, "margins"))).stdout.trim()).toBe("user");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("rejects a release missing the server before placing a CLI in the home", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-bb-install-"));
    try {
      const archive = await archiveFixture(root, ["margins"]);
      const manager = createRuntimeManager({
        env: { MARGINS_CLI_BIN_DIR: join(root, "bin") },
        fetchImpl: releaseFetch(archive) as unknown as typeof fetch,
        homeDir: root, platform: "linux", arch: "x64",
      });
      await expect(manager.ensureProjectServer({ dataDir: join(root, "plugin-data") })).rejects.toThrow(
        "did not contain a regular margins-server executable",
      );
      await expect(readFile(join(root, "bin", "margins"))).rejects.toMatchObject({ code: "ENOENT" });
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("explains when the pinned runtime release has not been published", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-bb-unpublished-"));
    try {
      const manager = createRuntimeManager({
        env: {},
        fetchImpl: vi.fn(async () => new Response(null, { status: 404 })) as unknown as typeof fetch,
        homeDir: root,
        platform: "linux",
        arch: "x64",
      });
      await expect(manager.ensureProjectServer({ dataDir: join(root, "plugin-data") })).rejects.toThrow(
        "Margins runtime 0.4.16 is not published yet",
      );
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});
