#!/usr/bin/env node
// V4 performance/size hard gate for the source CI run.

import { mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
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

const required = (name) => {
  const value = args.get(name);
  if (!value) throw new Error(`missing --${name}=...`);
  return resolve(ROOT, value);
};

const probe = JSON.parse(readFileSync(required('probe'), 'utf8'));
const budgets = JSON.parse(readFileSync(join(ROOT, 'contracts/performance-budgets.json'), 'utf8'));
const ffiBytes = statSync(required('ffi')).size;
const localHostBytes = statSync(required('local-host')).size;
const out = required('out');

const failures = [];
if (probe.wireRoundTripIterations !== budgets.wireRoundTrip.iterations) {
  failures.push(
    `wire iterations drift: ${probe.wireRoundTripIterations} != ${budgets.wireRoundTrip.iterations}`,
  );
}
if (probe.wireRoundTripElapsedMs > budgets.wireRoundTrip.maxElapsedMs) {
  failures.push(
    `wire elapsed ${probe.wireRoundTripElapsedMs}ms > ${budgets.wireRoundTrip.maxElapsedMs}ms`,
  );
}
if (typeof probe.peakRssKb !== 'number') {
  failures.push('peak RSS was not observed on the Linux performance runner');
} else if (probe.peakRssKb > budgets.process.maxPeakRssKb) {
  failures.push(`peak RSS ${probe.peakRssKb}KB > ${budgets.process.maxPeakRssKb}KB`);
}
if (ffiBytes > budgets.binaries.maxFfiCdylibBytes) {
  failures.push(`FFI cdylib ${ffiBytes}B > ${budgets.binaries.maxFfiCdylibBytes}B`);
}
if (localHostBytes > budgets.binaries.maxLocalHostReferenceBytes) {
  failures.push(
    `Local Host reference ${localHostBytes}B > ${budgets.binaries.maxLocalHostReferenceBytes}B`,
  );
}

const report = {
  schemaVersion: 1,
  sourceSha: process.env.GITHUB_SHA ?? 'local-unset',
  status: failures.length === 0 ? 'passed' : 'failed',
  verified: failures.length === 0,
  measurements: {
    wireRoundTripIterations: probe.wireRoundTripIterations,
    wireRoundTripElapsedMs: probe.wireRoundTripElapsedMs,
    peakRssKb: probe.peakRssKb,
    ffiCdylibBytes: ffiBytes,
    localHostReferenceBytes: localHostBytes,
  },
  budgets,
  failures,
};

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify(report, null, 2) + '\n');
console.log(`PERF_SIZE_METRICS ${JSON.stringify(report.measurements)}`);
if (failures.length) {
  throw new Error(`performance/size gate failed: ${failures.join('; ')}`);
}
