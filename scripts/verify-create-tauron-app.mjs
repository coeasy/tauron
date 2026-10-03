#!/usr/bin/env node
// Exercise the built create-tauron-app executable before publishing it.
import { execSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const TEMP = mkdtempSync(join(tmpdir(), 'tauron-create-app-preflight-'));

try {
  const cliPackage = JSON.parse(
    readFileSync(join(ROOT, 'packages/tauron-app-cli/package.json'), 'utf8'),
  );
  const starterPackage = JSON.parse(
    readFileSync(join(ROOT, 'packages/create-tauron-app/package.json'), 'utf8'),
  );
  if (Object.hasOwn(cliPackage.bin, 'create-tauron-app')) {
    throw new Error('@tauron/app-cli must not shadow create-tauron-app with a duplicate bin.');
  }
  if (starterPackage.bin?.['create-tauron-app'] === undefined) {
    throw new Error('create-tauron-app package must own the create-tauron-app executable.');
  }

  const target = join(TEMP, 'starter');
  const executable = join(ROOT, 'packages/create-tauron-app/dist/create-tauron-app.js');
  const run = spawnSync(process.execPath, [executable, target, '--framework', 'vanilla'], {
    cwd: TEMP,
    encoding: 'utf8',
  });
  if (run.error) throw run.error;
  if (run.status !== 0) {
    throw new Error(`create-tauron-app exited with ${run.status}:\n${run.stdout}\n${run.stderr}`);
  }

  for (const file of [
    'package.json',
    'pnpm-workspace.yaml',
    'src-tauri/Cargo.toml',
    'src-tauri/src/main.rs',
    'src-tauri/tauri.conf.json',
  ]) {
    if (!existsSync(join(target, file))) throw new Error(`Generated starter is missing ${file}.`);
  }

  const pnpmWorkspace = readFileSync(join(target, 'pnpm-workspace.yaml'), 'utf8');
  if (!/^\s*esbuild:\s*true\s*$/m.test(pnpmWorkspace)) {
    throw new Error('Generated pnpm-workspace.yaml must allow the esbuild install script.');
  }

  // 断言的期望值取自 CLI 自己的 package.json，不写字面量：写字面量的后果是发版后
  // 本脚本仍在校验**上一个**版本并「通过」（与 verify-registry-consumer.mjs 同一条理由）。
  // `framework-version.test.ts` 已把生成器的 pin 钉在这个版本上，两侧不会各说各话。
  const expectedVersion = cliPackage.version;
  const generatedPackage = JSON.parse(readFileSync(join(target, 'package.json'), 'utf8'));
  const cargoManifest = readFileSync(join(target, 'src-tauri/Cargo.toml'), 'utf8');
  if (generatedPackage.dependencies?.['@tauron/host'] !== expectedVersion) {
    throw new Error(
      `Generated starter does not pin @tauron/host to ${expectedVersion} ` +
        `(got ${JSON.stringify(generatedPackage.dependencies?.['@tauron/host'])}).`,
    );
  }
  if (!cargoManifest.includes(`tauron-adapter = { version = "=${expectedVersion}"`)) {
    throw new Error(`Generated starter does not pin tauron-adapter to =${expectedVersion}.`);
  }

  if (process.env.TAURON_INSTALL_PREFLIGHT === '1') {
    execSync('pnpm install --no-frozen-lockfile', { cwd: target, stdio: 'inherit' });
    execSync(`pnpm add @tauron/ui@${expectedVersion}`, { cwd: target, stdio: 'inherit' });
    execSync('pnpm run build', { cwd: target, stdio: 'inherit' });
  }

  console.log(
    'create-tauron-app local preflight passed: launcher and generated Tauri project are valid.',
  );
} finally {
  rmSync(TEMP, { recursive: true, force: true });
}
