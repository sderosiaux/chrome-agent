#!/usr/bin/env node

import { spawn } from 'child_process';
import { accessSync, chmodSync, constants, existsSync, readFileSync } from 'fs';
import { arch, platform } from 'os';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const packageJson = JSON.parse(readFileSync(join(__dirname, '..', 'package.json'), 'utf8'));
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

function main() {
  const targetKey = getTargetKey();
  const binaryName = targetKey ? supportedTargets[targetKey] : null;

  if (!binaryName) {
    console.error(`Error: Unsupported platform: ${platform()}-${arch()}`);
    console.error(`Supported: ${Object.keys(supportedTargets).join(', ')}`);
    process.exit(1);
  }

  const binaryPath = join(__dirname, binaryName);

  if (!existsSync(binaryPath)) {
    const url = `https://github.com/${repoSlug}/releases/download/v${version}/${binaryName}`;
    console.error(`Error: Native binary not found at ${binaryPath}`);
    console.error('The postinstall step downloads it from GitHub releases.');
    console.error(`Reinstall the package, or download manually from: ${url}`);
    process.exit(1);
  }

  if (platform() !== 'win32') {
    try { accessSync(binaryPath, constants.X_OK); } catch { chmodSync(binaryPath, 0o755); }
  }

  const child = spawn(binaryPath, process.argv.slice(2), {
    stdio: 'inherit',
    windowsHide: false,
  });

  child.on('error', (error) => {
    console.error(`Error: ${error.message}`);
    process.exit(1);
  });

  child.on('exit', (code, signal) => {
    if (signal) { process.kill(process.pid, signal); return; }
    process.exit(code ?? 1);
  });
}

main();
