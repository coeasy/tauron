#!/usr/bin/env node
// Clean-directory install/build smoke test for the published Tauron SDK.
// Run only after the npm and crates.io packages have been published.
import { execFileSync, execSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const ROOT = mkdtempSync(join(tmpdir(), 'tauron-registry-consumer-'));
const isWindows = process.platform === 'win32';

// 版本从仓库里读，不写死：写死的后果是发版后脚本仍在装**上一个**版本并「通过」——
// 那比失败更危险。两处真相必须一致，不一致直接退出（而不是静默取一个）。
function workspaceVersion() {
  const cargo = readFileSync('Cargo.toml', 'utf8');
  const pkg = cargo.slice(cargo.indexOf('[workspace.package]'));
  const cargoVersion = /^version = "([^"]+)"/m.exec(pkg)?.[1];
  const npmVersion = JSON.parse(readFileSync('package.json', 'utf8')).version;
  if (!cargoVersion) throw new Error('Cargo.toml 的 [workspace.package] 里没解析出版本');
  if (cargoVersion !== npmVersion) {
    throw new Error(`版本漂移：Cargo.toml=${cargoVersion} 而 package.json=${npmVersion}`);
  }
  return cargoVersion;
}

const VERSION = process.env.TAURON_VERIFY_VERSION ?? workspaceVersion();

function run(command, args, cwd) {
  const options = { cwd, stdio: 'inherit', env: process.env };
  if (!isWindows) return execFileSync(command, args, options);
  const quote = (value) => `"${String(value).replaceAll('"', '\\"')}"`;
  return execSync([command, ...args.map(quote)].join(' '), options);
}

try {
  const appDir = join(ROOT, 'starter');
  const cargoDir = join(ROOT, 'cargo-consumer');

  console.log(`1/5 Create a starter with the public npm command (tauron-app@${VERSION})`);
  run('npm', ['create', '--yes', `tauron-app@${VERSION}`, '--', appDir, '--framework', 'vanilla'], ROOT);

  console.log('2/5 Resolve the public JavaScript packages with pnpm');
  run('pnpm', ['install', '--no-frozen-lockfile'], appDir);
  run('pnpm', ['add', `@tauron/host@${VERSION}`, `@tauron/ui@${VERSION}`], appDir);
  run('pnpm', ['run', 'build'], appDir);

  console.log('3/5 Install the Rust host adapter using cargo add');
  run('cargo', ['new', '--lib', cargoDir], ROOT);
  run(
    'cargo',
    [
      'add',
      `tauron-adapter@=${VERSION}`,
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
