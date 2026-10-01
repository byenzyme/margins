import { createHash } from "node:crypto";
import { execFile as execFileCallback } from "node:child_process";
import {
  chmod,
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import { homedir, platform } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

const execFile = promisify(execFileCallback);

export const RUNTIME_RELEASE_VERSION = "0.4.11";
const RELEASE_API = `https://api.github.com/repos/byenzyme/margins/releases/tags/v${RUNTIME_RELEASE_VERSION}`;
const MAX_ARCHIVE_BYTES = 128 * 1024 * 1024;

interface ReleaseAsset {
  name?: unknown;
  browser_download_url?: unknown;
  digest?: unknown;
  size?: unknown;
}

interface RuntimeManagerOptions {
  env?: NodeJS.ProcessEnv;
  fetchImpl?: typeof fetch;
  homeDir?: string;
  platform?: NodeJS.Platform;
  arch?: string;
  execFile?: typeof execFile;
}

function targetName(hostPlatform: NodeJS.Platform, arch: string) {
  if (hostPlatform === "darwin" && arch === "arm64") return "aarch64-apple-darwin";
  if (hostPlatform === "linux" && arch === "x64") return "x86_64-unknown-linux-gnu";
  if (hostPlatform === "linux" && arch === "arm64") return "aarch64-unknown-linux-gnu";
  return null;
}

const RELEASE_EXECUTABLES = ["margins", "margins-server"] as const;

async function isRegularExecutable(path: string) {
  try {
    const stat = await lstat(path);
    return stat.isFile() && !stat.isSymbolicLink() && (stat.mode & 0o111) !== 0;
  } catch {
    return false;
  }
}

function sha256(bytes: Uint8Array) {
  return createHash("sha256").update(bytes).digest("hex");
}

function selectAsset(raw: unknown, expectedName: string) {
  if (typeof raw !== "object" || raw === null) return null;
  const assets = (raw as { assets?: unknown }).assets;
  if (!Array.isArray(assets)) return null;
  const asset = assets.find(
    (candidate): candidate is ReleaseAsset =>
      typeof candidate === "object" &&
      candidate !== null &&
      (candidate as ReleaseAsset).name === expectedName,
  );
  if (
    !asset ||
    typeof asset.browser_download_url !== "string" ||
    typeof asset.digest !== "string" ||
    !/^sha256:[a-f0-9]{64}$/.test(asset.digest) ||
    typeof asset.size !== "number" ||
    !Number.isSafeInteger(asset.size) ||
    asset.size <= 0 ||
    asset.size > MAX_ARCHIVE_BYTES
  ) {
    return null;
  }
  let url: URL;
  try {
    url = new URL(asset.browser_download_url);
  } catch {
    return null;
  }
  if (
    url.protocol !== "https:" ||
    url.hostname !== "github.com" ||
    !url.pathname.startsWith(
      `/byenzyme/margins/releases/download/v${RUNTIME_RELEASE_VERSION}/`,
    )
  ) {
    return null;
  }
  return {
    url: url.toString(),
    digest: asset.digest.slice("sha256:".length),
    size: asset.size,
  };
}

async function downloadPinnedArchive(
  fetchImpl: typeof fetch,
  expectedName: string,
  signal?: AbortSignal,
) {
  const release = await fetchImpl(RELEASE_API, {
    signal,
    headers: { accept: "application/vnd.github+json", "user-agent": "bb-plugin-margins" },
  });
  if (release.status === 404) return null;
  if (!release.ok) throw new Error(`release lookup failed (${release.status})`);
  const asset = selectAsset(await release.json(), expectedName);
  if (!asset) throw new Error(`release v${RUNTIME_RELEASE_VERSION} has no verified ${expectedName}`);

  const response = await fetchImpl(asset.url, { signal, redirect: "follow" });
  if (!response.ok) throw new Error(`runtime download failed (${response.status})`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.byteLength !== asset.size) {
    throw new Error(`runtime download was incomplete (${bytes.byteLength} of ${asset.size} bytes)`);
  }
  const observed = sha256(bytes);
  if (observed !== asset.digest) throw new Error("runtime download did not match its release digest");
  return bytes;
}

async function replaceManagedBinary(source: string, destination: string) {
  const marker = `${destination}.bb-margins-managed`;
  const exists = await lstat(destination).catch(() => null);
  if (exists) {
    const managed = await readFile(marker, "utf8")
      .then((value) => value.startsWith("managed-by=bb-plugin-margins\n"))
      .catch(() => false);
    if (!managed) {
      throw new Error(`${destination} already exists and was not installed by the Margins bb plugin`);
    }
  }
  const temp = `${destination}.tmp-${process.pid}-${Date.now()}`;
  await copyFile(source, temp);
  await chmod(temp, 0o755);
  await rename(temp, destination);
  await writeFile(
    marker,
    `managed-by=bb-plugin-margins\nversion=${RUNTIME_RELEASE_VERSION}\n`,
    { mode: 0o600 },
  );
}

async function copyRuntimeBinary(source: string, destination: string) {
  const temp = `${destination}.tmp-${process.pid}-${Date.now()}`;
  await copyFile(source, temp);
  await chmod(temp, 0o755);
  await rename(temp, destination);
}

async function installRuntime(input: {
  archive: Uint8Array;
  dataDir: string;
  runtimeBinDir: string;
  cliBinDir: string;
  execFileImpl: typeof execFile;
  signal?: AbortSignal;
  executables: readonly (typeof RELEASE_EXECUTABLES)[number][];
}) {
  await mkdir(input.dataDir, { recursive: true });
  await mkdir(input.runtimeBinDir, { recursive: true });
  const tempDir = await mkdtemp(join(input.dataDir, "install-"));
  try {
    const archivePath = join(tempDir, "margins.tar.gz");
    const unpacked = join(tempDir, "unpacked");
    await mkdir(unpacked);
    await writeFile(archivePath, input.archive, { mode: 0o600 });
    await input.execFileImpl("/usr/bin/tar", ["-xzf", archivePath, "-C", unpacked], {
      signal: input.signal,
    });
    for (const name of input.executables) {
      const source = join(unpacked, name);
      const stat = await lstat(source).catch(() => null);
      if (!stat?.isFile() || stat.isSymbolicLink()) {
        throw new Error(`the Margins release did not contain a regular ${name} executable`);
      }
      await copyRuntimeBinary(source, join(input.runtimeBinDir, name));
    }

    // Make the normal command available to agents and shells when that path is
    // free or already belongs to this plugin. An existing Margins command is
    // left alone; an unrelated command is never overwritten.
    await mkdir(input.cliBinDir, { recursive: true });
    const cliDestination = join(input.cliBinDir, "margins");
    const cliExists = await lstat(cliDestination).catch(() => null);
    const pluginManaged = await readFile(`${cliDestination}.bb-margins-managed`, "utf8")
      .then((value) => value.startsWith("managed-by=bb-plugin-margins\n"))
      .catch(() => false);
    if (!cliExists || pluginManaged) {
      await replaceManagedBinary(join(unpacked, "margins"), cliDestination);
    }
  } finally {
    await rm(tempDir, { recursive: true, force: true });
  }
}

export function createRuntimeManager(options: RuntimeManagerOptions = {}) {
  const env = options.env ?? process.env;
  const fetchImpl = options.fetchImpl ?? fetch;
  const home = options.homeDir ?? homedir();
  const hostPlatform = options.platform ?? platform();
  const hostArch = options.arch ?? process.arch;
  const execFileImpl = options.execFile ?? execFile;

  return {
    async ensureProjectServer(input: { dataDir: string; signal?: AbortSignal }): Promise<string> {
      const configured = env.MARGINS_PROJECT_SERVER_PATH?.trim();
      if (configured) {
        if (!(await isRegularExecutable(configured))) {
          throw new Error("The configured Margins recorder is not executable");
        }
        return configured;
      }
      const runtimeBinDir = join(input.dataDir, "runtime", `v${RUNTIME_RELEASE_VERSION}`);
      const serverPath = join(runtimeBinDir, "margins-server");
      if (await isRegularExecutable(serverPath)) return serverPath;
      const target = targetName(hostPlatform, hostArch);
      if (!target) throw new Error("Recording is not available on this project machine");
      const expectedName = `margins-${RUNTIME_RELEASE_VERSION}-${target}.tar.gz`;
      const archive = await downloadPinnedArchive(fetchImpl, expectedName, input.signal);
      if (!archive) throw new Error(`Margins ${RUNTIME_RELEASE_VERSION} is not published for this project machine`);
      await installRuntime({
        archive,
        dataDir: input.dataDir,
        runtimeBinDir,
        cliBinDir: env.MARGINS_CLI_BIN_DIR?.trim() || join(home, ".local", "bin"),
        execFileImpl,
        signal: input.signal,
        executables: ["margins", "margins-server"],
      });
      if (!(await isRegularExecutable(serverPath))) {
        throw new Error("The Margins recorder was not installed correctly");
      }
      return serverPath;
    },
  };
}

export const runtimeManagerInternals = { selectAsset, sha256, targetName };
