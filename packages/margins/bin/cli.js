#!/usr/bin/env node

import { spawn, execSync } from 'child_process';
import { existsSync, readFileSync, mkdirSync, chmodSync, writeFileSync, renameSync, rmSync } from 'fs';
import { dirname, join, resolve } from 'path';
import { fileURLToPath } from 'url';
import { homedir, platform, arch } from 'os';
import https from 'https';
import http from 'http';

const packageJson = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf-8'));
const VERSION = packageJson.version;
const packageDir = dirname(dirname(fileURLToPath(import.meta.url)));
const checkoutRoot = resolve(packageDir, '../..');

const port = process.env.MARGINS_PORT || '8787';
const host = process.env.MARGINS_HOST || '127.0.0.1';
const dataDir = process.env.MARGINS_DATA_DIR || join(homedir(), '.margins-app');
const healthTimeoutMs = parseInt(process.env.MARGINS_HEALTH_TIMEOUT_MS || '30000', 10);

// Ensure data directory exists
mkdirSync(dataDir, { recursive: true });

function getPlatformId() {
  const plat = platform();
  const arch_ = arch();

  let platformStr = '';
  if (plat === 'darwin') {
    platformStr = 'darwin';
  } else if (plat === 'linux') {
    platformStr = 'linux';
  } else {
    throw new Error(`Unsupported platform: ${plat}`);
  }

  let archStr = '';
  if (arch_ === 'arm64') {
    archStr = 'arm64';
  } else if (arch_ === 'x64') {
    archStr = 'x64';
  } else {
    throw new Error(`Unsupported architecture: ${arch_}`);
  }

  return { platform: platformStr, arch: archStr };
}

function localBinaryCandidates() {
  const candidates = [];
  if (process.env.CARGO_TARGET_DIR) {
    candidates.push(join(process.env.CARGO_TARGET_DIR, 'debug', 'margins-server'));
    candidates.push(join(process.env.CARGO_TARGET_DIR, 'release', 'margins-server'));
  }
  candidates.push(join(checkoutRoot, 'target', 'debug', 'margins-server'));
  candidates.push(join(checkoutRoot, 'target', 'release', 'margins-server'));
  candidates.push(join(checkoutRoot, 'desktop', 'src-tauri', 'target', 'debug', 'margins-server'));
  candidates.push(join(checkoutRoot, 'desktop', 'src-tauri', 'target', 'release', 'margins-server'));
  return candidates;
}

function getDownloadUrl(owner, repo, version, platform, arch) {
  const base = `https://github.com/${owner}/${repo}/releases/download/v${version}`;
  const filename = `margins-server-${platform}-${arch}`;
  const gz = `${base}/${filename}.tar.gz`;
  const plain = `${base}/${filename}`;

  // Try .tar.gz first if on Unix-like systems; fallback to plain binary
  if (platform === 'darwin' || platform === 'linux') {
    return { url: gz, isTarGz: true, fallbackUrl: plain, fallbackIsTarGz: false };
  }
  return { url: plain, isTarGz: false };
}

async function downloadFile(url) {
  return new Promise((resolve, reject) => {
    https.get(url, { redirect: 'follow' }, (res) => {
      if (res.statusCode >= 400) {
        reject(new Error(`Download failed: HTTP ${res.statusCode} from ${url}`));
        return;
      }
      const chunks = [];
      res.on('data', (chunk) => chunks.push(chunk));
      res.on('end', () => resolve(Buffer.concat(chunks)));
      res.on('error', reject);
    }).on('error', reject);
  });
}

async function resolveBinary() {
  // If MARGINS_SERVER_BIN is set, use it directly
  if (process.env.MARGINS_SERVER_BIN) {
    const bin = process.env.MARGINS_SERVER_BIN;
    if (!existsSync(bin)) {
      throw new Error(`MARGINS_SERVER_BIN points to non-existent file: ${bin}`);
    }
    return bin;
  }

  for (const candidate of localBinaryCandidates()) {
    if (existsSync(candidate)) {
      console.error(`Using local margins-server binary: ${candidate}`);
      return candidate;
    }
  }

  // Check cache
  const binDir = join(dataDir, 'bin');
  const cachedBin = join(binDir, `margins-server-v${VERSION}`);

  if (existsSync(cachedBin)) {
    return cachedBin;
  }

  // Download from GitHub Releases
  const { platform: plat, arch: arch_ } = getPlatformId();
  const owner = 'jshph';
  const repo = 'aside-desktop';

  console.error(`Downloading margins-server v${VERSION} for ${plat}-${arch_}...`);

  const downloadInfo = getDownloadUrl(owner, repo, VERSION, plat, arch_);

  let buffer;
  try {
    buffer = await downloadFile(downloadInfo.url);
  } catch (err) {
    if (downloadInfo.fallbackUrl) {
      console.error(`Primary URL failed, trying fallback...`);
      try {
        buffer = await downloadFile(downloadInfo.fallbackUrl);
        downloadInfo.isTarGz = downloadInfo.fallbackIsTarGz;
      } catch (fallbackErr) {
        throw new Error(
          `Failed to download margins-server. Release v${VERSION} may not exist yet.\n` +
          `Set MARGINS_SERVER_BIN=/path/to/binary to use a local build.\n` +
          `Error: ${fallbackErr.message}`
        );
      }
    } else {
      throw new Error(
        `Failed to download margins-server from ${downloadInfo.url}.\n` +
        `Release v${VERSION} may not exist yet.\n` +
        `Set MARGINS_SERVER_BIN=/path/to/binary to use a local build.\n` +
        `Error: ${err.message}`
      );
    }
  }

  mkdirSync(binDir, { recursive: true });

  if (downloadInfo.isTarGz) {
    // Extract tarball
    const tarPath = join(binDir, 'temp.tar.gz');
    try {
      writeFileSync(tarPath, buffer);

      // Use tar to extract
      execSync(`tar -xzf "${tarPath}" -C "${binDir}"`, { stdio: 'inherit' });

      // Clean up tar file
      try { rmSync(tarPath); } catch {}

      // Find the extracted binary (should be margins-server-<platform>-<arch>)
      const expectedName = `margins-server-${plat}-${arch_}`;
      const extractedPath = join(binDir, expectedName);

      if (!existsSync(extractedPath)) {
        throw new Error(`Extraction complete but expected binary not found: ${extractedPath}`);
      }

      chmodSync(extractedPath, 0o755);
      renameSync(extractedPath, cachedBin);
      chmodSync(cachedBin, 0o755);
      return cachedBin;
    } catch (err) {
      throw new Error(`Failed to extract tarball: ${err.message}`);
    }
  } else {
    // Write binary directly
    writeFileSync(cachedBin, buffer);
    chmodSync(cachedBin, 0o755);
    return cachedBin;
  }
}

async function waitForHealth(retries = 0) {
  return new Promise((resolve, reject) => {
    if (retries * 250 > healthTimeoutMs) {
      reject(new Error(`Health check timeout after ${healthTimeoutMs}ms`));
      return;
    }

    const url = `http://127.0.0.1:${port}/health`;
    http.get(url, (res) => {
      if (res.statusCode === 200) {
        resolve();
      } else {
        setTimeout(() => waitForHealth(retries + 1).then(resolve, reject), 250);
      }
    }).on('error', () => {
      setTimeout(() => waitForHealth(retries + 1).then(resolve, reject), 250);
    });
  });
}

async function main() {
  try {
    const binaryPath = await resolveBinary();
    console.error(`Starting margins-server from ${binaryPath}`);

    const child = spawn(binaryPath, [], {
      stdio: ['ignore', 'pipe', 'inherit'],
      env: {
        ...process.env,
        MARGINS_PORT: port,
        MARGINS_HOST: host,
        MARGINS_DATA_DIR: dataDir,
      },
    });

    // Forward stdout
    child.stdout.on('data', (data) => {
      process.stdout.write(data);
    });

    // Handle signals
    const cleanup = (signal) => {
      child.kill(signal || 'SIGTERM');
    };

    process.on('SIGINT', () => cleanup('SIGINT'));
    process.on('SIGTERM', () => cleanup('SIGTERM'));

    child.on('exit', (code, signal) => {
      if (signal) {
        // Exit with code 128 + signal number for common signals
        const signalMap = {
          'SIGINT': 2,
          'SIGTERM': 15,
          'SIGHUP': 1,
          'SIGQUIT': 3,
          'SIGKILL': 9,
        };
        process.exit(128 + (signalMap[signal] || 1));
      } else {
        process.exit(code || 0);
      }
    });

    // Wait for health check
    try {
      await waitForHealth();
      console.log(`\nMargins running at http://${host}:${port}\n`);

      const tokenPath = join(dataDir, 'token');
      if (existsSync(tokenPath)) {
        const token = readFileSync(tokenPath, 'utf-8').trim();
        console.log(`Auth token: ${token}\n`);
      }
    } catch (err) {
      console.error(`\nWarning: ${err.message}`);
      console.error(`Server may still be starting. Check logs above.`);
    }
  } catch (err) {
    console.error(`Error: ${err.message}`);
    process.exit(1);
  }
}

main().catch((err) => {
  console.error(`Fatal error: ${err.message}`);
  process.exit(1);
});
