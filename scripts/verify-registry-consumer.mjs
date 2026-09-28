#!/usr/bin/env node
// Clean-directory install/build smoke test for a published Tauron 1.0.1 SDK.
// Run only after the npm and crates.io packages have been published.
import { execFileSync, execSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const ROOT = mkdtempSync(join(tmpdir(), 'tauron-registry-consumer-'));
const isWindows = process.platform === 'win32';

function run(command, args, cwd) {
  const options = { cwd, stdio: 'inherit', env: process.env };
  if (!isWindows) return execFileSync(command, args, options);
  const quote = (value) => `"${String(value).replaceAll('"', '\\"')}"`;
  return execSync([command, ...args.map(quote)].join(' '), options);
}

try {
  const appDir = join(ROOT, 'starter');
  const cargoDir = join(ROOT, 'cargo-consumer');

  console.log('1/5 Create a starter with the public npm command');
  run('npm', ['create', '--yes', 'tauron-app@1.0.1', '--', appDir, '--framework', 'vanilla'], ROOT);

  console.log('2/5 Resolve the public JavaScript packages with pnpm');
  run('pnpm', ['install', '--no-frozen-lockfile'], appDir);
  run('pnpm', ['add', '@tauron/host@1.0.1', '@tauron/ui@1.0.1'], appDir);
  run('pnpm', ['run', 'build'], appDir);

  console.log('3/5 Install the Rust host adapter using cargo add');
  run('cargo', ['new', '--lib', cargoDir], ROOT);
  run(
    'cargo',
    [
      'add',
      'tauron-adapter@=1.0.1',
      '--features',
      'tauri',
      '--manifest-path',
      join(cargoDir, 'Cargo.toml'),
    ],
    ROOT,
  );

  console.log('4/5 Compile the Rust adapter consumer');
  run('cargo', ['check', '--manifest-path', join(cargoDir, 'Cargo.toml')], ROOT);

  console.log('5/5 Compile the generated Tauri 2 example');
  run('cargo', ['check', '--manifest-path', join(appDir, 'src-tauri', 'Cargo.toml')], ROOT);
  console.log('Registry consumer smoke test passed.');
} finally {
  if (process.env.TAURON_KEEP_CONSUMER === '1') {
    console.log(`Preserved clean consumer projects at ${ROOT}`);
  } else {
    rmSync(ROOT, { recursive: true, force: true });
  }
}
