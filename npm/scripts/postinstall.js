#!/usr/bin/env node

import { createHash } from 'crypto';
import { chmodSync, createWriteStream, existsSync, mkdirSync, readFileSync, renameSync, rmSync } from 'fs';
import { get } from 'https';
import { arch, platform } from 'os';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const projectRoot = join(__dirname, '..');
const binDir = join(projectRoot, 'bin');
const packageJson = JSON.parse(readFileSync(join(projectRoot, 'package.json'), 'utf8'));
const version = packageJson.version;
const repoSlug = 'sderosiaux/chrome-agent';

const supportedTargets = Object.freeze({
  'darwin-arm64': 'chrome-agent-darwin-arm64',
  'darwin-x64': 'chrome-agent-darwin-x64',
  'linux-arm64': 'chrome-agent-linux-arm64',
  'linux-x64': 'chrome-agent-linux-x64',
  'win32-x64': 'chrome-agent-windows-x64.exe',
});

function getTargetKey() {
  const p = platform();
  const a = arch();
  if (p === 'darwin') return a === 'arm64' ? 'darwin-arm64' : a === 'x64' ? 'darwin-x64' : null;
  if (p === 'linux') return a === 'x64' ? 'linux-x64' : a === 'arm64' ? 'linux-arm64' : null;
  if (p === 'win32') return a === 'x64' ? 'win32-x64' : null;
  return null;
}

async function downloadFile(url, destination) {
  const tempPath = `${destination}.download`;
  rmSync(tempPath, { force: true });

  return new Promise((resolve, reject) => {
    const request = (currentUrl, redirects = 10) => {
      get(currentUrl, {
        headers: { Accept: 'application/octet-stream', 'User-Agent': `chrome-agent/${version}` },
      }, (response) => {
        if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
          response.resume();
          if (redirects === 0) { reject(new Error('Too many redirects')); return; }
          request(new URL(response.headers.location, currentUrl), redirects - 1);
          return;
        }
        if (response.statusCode !== 200) {
          response.resume();
          reject(new Error(`HTTP ${response.statusCode} from ${currentUrl}`));
          return;
        }
        const file = createWriteStream(tempPath);
        response.pipe(file);
        file.on('finish', () => file.close(() => {
          try { renameSync(tempPath, destination); resolve(); }
          catch (e) { reject(e); }
        }));
        file.on('error', reject);
        response.on('error', reject);
      }).on('error', reject).setTimeout(30_000, function() { this.destroy(new Error('Timeout')); });
    };
    request(url);
  }).catch((error) => { rmSync(tempPath, { force: true }); throw error; });
}

/** Fetch a small text asset (the checksum file) into memory. */
async function fetchText(url) {
  return new Promise((resolve, reject) => {
    const request = (currentUrl, redirects = 10) => {
      get(currentUrl, {
        headers: { Accept: 'text/plain', 'User-Agent': `chrome-agent/${version}` },
      }, (response) => {
        if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
          response.resume();
          if (redirects === 0) { reject(new Error('Too many redirects')); return; }
          request(new URL(response.headers.location, currentUrl), redirects - 1);
          return;
        }
        if (response.statusCode !== 200) {
          response.resume();
          reject(new Error(`HTTP ${response.statusCode} from ${currentUrl}`));
          return;
        }
        let body = '';
        response.setEncoding('utf8');
        response.on('data', (chunk) => { body += chunk; });
        response.on('end', () => resolve(body));
        response.on('error', reject);
      }).on('error', reject).setTimeout(30_000, function() { this.destroy(new Error('Timeout')); });
    };
    request(url);
  });
}

export function sha256File(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

/**
 * Compare a downloaded file against the checksum published beside it.
 *
 * The binary is fetched over the network and chmod +x'd; without this there was nothing
 * anywhere in the chain saying the bytes are the ones that were built. A mismatch deletes
 * the file rather than leaving a half-trusted binary on disk: a corrupted-but-running
 * binary surfaces as confusing behaviour later, which is worse than a failed install.
 */
export function verifyChecksum(actualHex, expectedRaw, binaryName) {
  const expected = String(expectedRaw).trim().split(/\s+/)[0].toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(expected)) {
    throw new Error(`Malformed checksum for ${binaryName}: "${String(expectedRaw).trim().slice(0, 80)}"`);
  }
  if (actualHex.toLowerCase() !== expected) {
    throw new Error(
      `Checksum mismatch for ${binaryName}: expected ${expected}, got ${actualHex}. ` +
      'The download was corrupted or the release asset does not match what was published.'
    );
  }
}

async function main() {
  const targetKey = getTargetKey();
  const binaryName = targetKey ? supportedTargets[targetKey] : null;

  if (!binaryName) {
    // Not a fatal error during local dev
    if (existsSync(join(projectRoot, '.git'))) {
      console.warn(`Warning: No prebuilt binary for ${platform()}-${arch()}. Build from source with: cargo build --release`);
      return;
    }
    throw new Error(`Unsupported platform: ${platform()}-${arch()}. Supported: ${Object.keys(supportedTargets).join(', ')}`);
  }

  mkdirSync(binDir, { recursive: true });
  const binaryPath = join(binDir, binaryName);

  if (existsSync(binaryPath)) {
    if (platform() !== 'win32') chmodSync(binaryPath, 0o755);
    console.log(`chrome-agent: native binary already present (${binaryName})`);
    return;
  }

  const url = `https://github.com/${repoSlug}/releases/download/v${version}/${binaryName}`;
  console.log(`chrome-agent: downloading native binary for ${platform()}-${arch()}...`);

  await downloadFile(url, binaryPath);
  try {
    verifyChecksum(sha256File(binaryPath), await fetchText(`${url}.sha256`), binaryName);
  } catch (error) {
    rmSync(binaryPath, { force: true });
    throw error;
  }
  if (platform() !== 'win32') chmodSync(binaryPath, 0o755);
  console.log(`chrome-agent: installed ${binaryName} (sha256 verified)`);
}

// Importable for the unit tests without running the install.
if (process.env.CHROME_AGENT_POSTINSTALL_NOOP !== '1') {
  main().catch((error) => {
    console.error(`chrome-agent postinstall failed: ${error.message}`);
    process.exitCode = 1;
  });
}
