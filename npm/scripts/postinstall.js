#!/usr/bin/env node

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
const repoSlug = 'sderosiaux/aibrowsr';

const supportedTargets = Object.freeze({
  'darwin-arm64': 'aibrowsr-darwin-arm64',
  'darwin-x64': 'aibrowsr-darwin-x64',
  'linux-arm64': 'aibrowsr-linux-arm64',
  'linux-x64': 'aibrowsr-linux-x64',
  'win32-x64': 'aibrowsr-windows-x64.exe',
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
        headers: { Accept: 'application/octet-stream', 'User-Agent': `aibrowsr/${version}` },
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
    console.log(`aibrowsr: native binary already present (${binaryName})`);
    return;
  }

  const url = `https://github.com/${repoSlug}/releases/download/v${version}/${binaryName}`;
  console.log(`aibrowsr: downloading native binary for ${platform()}-${arch()}...`);

  await downloadFile(url, binaryPath);
  if (platform() !== 'win32') chmodSync(binaryPath, 0o755);
  console.log(`aibrowsr: installed ${binaryName}`);
}

main().catch((error) => {
  console.error(`aibrowsr postinstall failed: ${error.message}`);
  process.exitCode = 1;
});
