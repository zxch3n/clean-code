#!/usr/bin/env node
const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');

function getBinaryPath() {
  const vendor = path.join(__dirname, '..', 'vendor');
  const exe = process.platform === 'win32' ? 'clean-my-code.exe' : 'clean-my-code';
  return path.join(vendor, exe);
}

function run() {
  const bin = getBinaryPath();
  if (!fs.existsSync(bin)) {
    console.error('[clean-my-code] binary not found.');
    console.error('Tried:', bin);
    console.error('If install failed, try again:');
    console.error('  npm i clean-my-code -g  # or use npx again');
    console.error('Or build from source (Rust required):');
    console.error(
      '  cargo install --git https://github.com/zxch3n/clean-code --bin clean-my-code',
    );
    process.exit(1);
  }

  const child = spawn(bin, process.argv.slice(2), {
    stdio: 'inherit',
  });
  child.on('close', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
    } else {
      process.exit(code);
    }
  });
}

run();
