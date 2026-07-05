#!/usr/bin/env node
import { spawnSync } from 'child_process';
import { createRequire } from 'module';

const require = createRequire(import.meta.url);

const PLATFORMS = {
  'darwin-arm64': 'bulltui-darwin-arm64',
  'darwin-x64': 'bulltui-darwin-x64',
  'linux-x64': 'bulltui-linux-x64',
  'linux-arm64': 'bulltui-linux-arm64',
};

const key = `${process.platform}-${process.arch}`;
const pkgName = PLATFORMS[key];

if (!pkgName) {
  console.error(`bulltui: unsupported platform ${key}`);
  console.error(`Supported: ${Object.keys(PLATFORMS).join(', ')}`);
  process.exit(1);
}

const binName = process.platform === 'win32' ? 'bulltui.exe' : 'bulltui';
let binPath;

try {
  binPath = require.resolve(`${pkgName}/${binName}`);
} catch {
  console.error(`bulltui: platform package "${pkgName}" is not installed.`);
  console.error(`Try: npm install ${pkgName}`);
  process.exit(1);
}

const result = spawnSync(binPath, process.argv.slice(2), { stdio: 'inherit' });

if (result.signal) {
  process.kill(process.pid, result.signal);
} else {
  process.exit(result.status ?? 0);
}
