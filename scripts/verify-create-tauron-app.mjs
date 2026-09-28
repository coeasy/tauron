#!/usr/bin/env node
// Exercise the built create-tauron-app executable before publishing it.
import { spawnSync } from 'node:child_process';
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
    'src-tauri/Cargo.toml',
    'src-tauri/src/main.rs',
    'src-tauri/tauri.conf.json',
  ]) {
    if (!existsSync(join(target, file))) throw new Error(`Generated starter is missing ${file}.`);
  }

  const generatedPackage = JSON.parse(readFileSync(join(target, 'package.json'), 'utf8'));
  const cargoManifest = readFileSync(join(target, 'src-tauri/Cargo.toml'), 'utf8');
  if (generatedPackage.dependencies?.['@tauron/host'] !== '1.0.2') {
    throw new Error('Generated starter does not pin @tauron/host to 1.0.2.');
  }
  if (!cargoManifest.includes('tauron-adapter = { version = "=1.0.2"')) {
    throw new Error('Generated starter does not pin tauron-adapter to 1.0.2.');
  }

  console.log(
    'create-tauron-app local preflight passed: launcher and generated Tauri project are valid.',
  );
} finally {
  rmSync(TEMP, { recursive: true, force: true });
}
