#!/usr/bin/env node
// Public-registry gate for creating an official GitHub SDK release.
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const version =
  process.argv[2] ?? JSON.parse(readFileSync(join(ROOT, 'package.json'), 'utf8')).version;
const packageNames = readdirSync(join(ROOT, 'packages'))
  .map((directory) => join(ROOT, 'packages', directory, 'package.json'))
  .map((file) => JSON.parse(readFileSync(file, 'utf8')))
  .filter((pkg) => pkg.private !== true)
  .map((pkg) => pkg.name);
const crateNames = readdirSync(join(ROOT, 'crates'), { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => {
    const manifest = readFileSync(join(ROOT, 'crates', entry.name, 'Cargo.toml'), 'utf8');
    const name = /^name\s*=\s*"([^"]+)"/m.exec(manifest)?.[1];
    if (!name) throw new Error(`Could not read crate name from crates/${entry.name}/Cargo.toml`);
    return name;
  });

const checks = [
  ...packageNames.map((name) => async () => {
    const url = `https://registry.npmjs.org/${encodeURIComponent(name)}/${version}`;
    const response = await fetch(url, { signal: AbortSignal.timeout(20_000) });
    if (!response.ok)
      throw new Error(`npm package ${name}@${version} is unavailable (HTTP ${response.status}).`);
    const metadata = await response.json();
    if (metadata.version !== version)
      throw new Error(`npm package ${name} resolved unexpected version ${metadata.version}.`);
    console.log(`npm ${name}@${version}: published`);
  }),
  ...crateNames.map((name) => async () => {
    const response = await fetch(`https://crates.io/api/v1/crates/${name}/${version}`, {
      headers: { 'user-agent': 'Tauron SDK release check' },
      signal: AbortSignal.timeout(20_000),
    });
    if (!response.ok)
      throw new Error(
        `crates.io package ${name}@${version} is unavailable (HTTP ${response.status}).`,
      );
    const metadata = await response.json();
    if (metadata.version?.num !== version || metadata.version?.yanked) {
      throw new Error(`crates.io package ${name}@${version} is absent, mismatched, or yanked.`);
    }
    console.log(`crates.io ${name}@${version}: published`);
  }),
];

const failures = [];
for (let i = 0; i < checks.length; i += 5) {
  const results = await Promise.allSettled(checks.slice(i, i + 5).map((check) => check()));
  failures.push(...results.filter((result) => result.status === 'rejected'));
}
for (const failure of failures)
  console.error(`✗ ${failure.reason instanceof Error ? failure.reason.message : failure.reason}`);
if (failures.length > 0) process.exitCode = 1;
else
  console.log(
    `All ${packageNames.length} npm packages and ${crateNames.length} Rust crates are public at ${version}.`,
  );
