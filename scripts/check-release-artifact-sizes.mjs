#!/usr/bin/env node
// V4 final installer/package size hard gate. Runs only after all four packaging legs succeed.

import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const args = new Map(
  process.argv
    .slice(2)
    .filter((x) => x.startsWith('--') && x.includes('='))
    .map((x) => {
      const i = x.indexOf('=');
      return [x.slice(2, i), x.slice(i + 1)];
    }),
);
const root = resolve(ROOT, args.get('root') ?? 'release-bundles');
const out = resolve(ROOT, args.get('out') ?? 'release-artifact-size-report.json');
const budgets = JSON.parse(readFileSync(join(ROOT, 'contracts/performance-budgets.json'), 'utf8'));
const allowed = new Set(['.exe', '.msi', '.dmg', '.deb', '.rpm', '.AppImage']);

function walk(path) {
  const output = [];
  for (const name of readdirSync(path)) {
    const full = join(path, name);
    const stat = statSync(full);
    if (stat.isDirectory()) output.push(...walk(full));
    else output.push(full);
  }
  return output;
}

const files = walk(root)
  .filter((path) => allowed.has(extname(path)) || path.endsWith('.AppImage'))
  .map((path) => ({ path: path.slice(root.length + 1), bytes: statSync(path).size }))
  .sort((a, b) => a.path.localeCompare(b.path));

if (files.length < 4) {
  throw new Error(`expected packaged assets from four release legs, found only ${files.length}`);
}
const aggregateBytes = files.reduce((sum, file) => sum + file.bytes, 0);
const failures = files
  .filter((file) => file.bytes > budgets.releaseArtifacts.maxPerFileBytes)
  .map(
    (file) =>
      `${file.path} ${file.bytes}B > per-file budget ${budgets.releaseArtifacts.maxPerFileBytes}B`,
  );
if (aggregateBytes > budgets.releaseArtifacts.maxAggregateBytes) {
  failures.push(`aggregate ${aggregateBytes}B > ${budgets.releaseArtifacts.maxAggregateBytes}B`);
}

const report = {
  schemaVersion: 1,
  sourceSha: process.env.GITHUB_SHA ?? 'local-unset',
  status: failures.length === 0 ? 'passed' : 'failed',
  verified: failures.length === 0,
  files,
  aggregateBytes,
  budgets: budgets.releaseArtifacts,
  failures,
};
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify(report, null, 2) + '\n');
console.log(`RELEASE_ARTIFACT_SIZE_METRICS files=${files.length} aggregateBytes=${aggregateBytes}`);
if (failures.length) throw new Error(`release artifact size gate failed: ${failures.join('; ')}`);
