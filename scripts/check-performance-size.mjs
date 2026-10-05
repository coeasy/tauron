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
// 轮 44（A94）：内存读数来自**真实安装流**（install_stream_probe 走 preview→reviewed 提交链），
// 不再是 perf_probe 的进程读数——那条读数只测了 Wire 压测，与内存最重的路径无关。
const installProbe = JSON.parse(readFileSync(required('install-probe'), 'utf8'));
const budgets = JSON.parse(readFileSync(join(ROOT, 'contracts/performance-budgets.json'), 'utf8'));
const ffiBytes = statSync(required('ffi')).size;
const localHostBytes = statSync(required('local-host')).size;
const out = required('out');

const failures = [];
// ① Wire 热路径：轮数必须与预算一致、耗时要在上限内。
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
// ② 真实安装流内存（轮 44 / A94）。三道断言各自独立：
//    载荷下限——探针包装得比下限大，小包跑出的"低内存"什么都证明不了；
//    堆峰值——计数分配器的读数，平台无关（本地可变异证红）；
//    RSS 峰值——Linux VmHWM，只在性能 runner 上有，缺读数是失败而非跳过。
if (installProbe.status !== 'installed') {
  failures.push(
    `install stream probe did not complete: status=${installProbe.status} (${installProbe.error ?? 'no error detail'})`,
  );
}
if (
  typeof installProbe.payloadBytes !== 'number' ||
  installProbe.payloadBytes < budgets.installStream.minPayloadBytes
) {
  failures.push(
    `install payload ${installProbe.payloadBytes}B < floor ${budgets.installStream.minPayloadBytes}B`,
  );
}
if (typeof installProbe.peakHeapKib !== 'number') {
  failures.push('install peak heap was not reported by the counting allocator');
} else if (installProbe.peakHeapKib > budgets.installStream.maxPeakHeapKib) {
  failures.push(
    `install peak heap ${installProbe.peakHeapKib}KiB > ${budgets.installStream.maxPeakHeapKib}KiB`,
  );
}
if (typeof installProbe.peakRssKb !== 'number') {
  failures.push('install peak RSS was not observed on the Linux performance runner');
} else if (installProbe.peakRssKb > budgets.installStream.maxPeakRssKb) {
  failures.push(
    `install peak RSS ${installProbe.peakRssKb}KB > ${budgets.installStream.maxPeakRssKb}KB`,
  );
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
    installStreamPayloadBytes: installProbe.payloadBytes,
    installStreamPeakHeapKib: installProbe.peakHeapKib,
    installStreamPeakRssKb: installProbe.peakRssKb,
    installStreamElapsedMs: installProbe.installElapsedMs,
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
