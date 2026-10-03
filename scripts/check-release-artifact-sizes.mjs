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
// 期望腿集合与 target-matrix 台账同源：`release.yml` 的产物名是 `bundle-${{ matrix.label }}`，
// 而 `check-target-matrix.mjs` 已经保证 workflow 里的 label 集合等于台账里 `releaseArtifact`
// 的行集合。写死一个数字（此前是 `files.length < 4`）只能证明「文件总数够多」，证明不了
// 「每条腿都交了」：一条腿的产物名不在白名单扩展名里就被静默丢掉，另一条腿多出两个文件，
// 总数照样过 4，Release 就带着缺一条腿的产物出去了。
// 隐藏约束：`release.yml` 里下载产物必须保持 `merge-multiple: false`——腿身份就是这一层
// `bundle-<label>/` 目录，摊平之后本门会把每个文件名当成一条未声明的腿直接判红（实测），
// 也就是说这条约束破了也是 fail-closed，不会静默放行。
const expectedLegs = JSON.parse(readFileSync(join(ROOT, 'contracts/target-matrix.json'), 'utf8'))
  .rows.filter((row) => row.releaseArtifact)
  .map((row) => `bundle-${row.label}`);
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
  .filter((path) => allowed.has(extname(path)))
  .map((path) => ({
    path: path.slice(root.length + 1).replaceAll('\\', '/'),
    bytes: statSync(path).size,
  }))
  .sort((a, b) => a.path.localeCompare(b.path));

const legOf = (file) => file.path.split('/')[0];
const legs = new Map();
for (const file of files) legs.set(legOf(file), (legs.get(legOf(file)) ?? 0) + 1);

const failures = [];
for (const leg of expectedLegs) {
  if (!legs.has(leg))
    failures.push(
      `release leg ${leg} 没有交出任何白名单扩展名的产物（实际到场的腿：${
        [...legs.keys()].join(', ') || '一条都没有'
      }）`,
    );
}
for (const leg of legs.keys()) {
  if (!expectedLegs.includes(leg))
    failures.push(`出现台账未声明的产物腿 ${leg}（contracts/target-matrix.json 里没有它）`);
}

const aggregateBytes = files.reduce((sum, file) => sum + file.bytes, 0);
failures.push(
  ...files
    .filter((file) => file.bytes > budgets.releaseArtifacts.maxPerFileBytes)
    .map(
      (file) =>
        `${file.path} ${file.bytes}B > per-file budget ${budgets.releaseArtifacts.maxPerFileBytes}B`,
    ),
);
if (aggregateBytes > budgets.releaseArtifacts.maxAggregateBytes) {
  failures.push(`aggregate ${aggregateBytes}B > ${budgets.releaseArtifacts.maxAggregateBytes}B`);
}

const report = {
  schemaVersion: 1,
  sourceSha: process.env.GITHUB_SHA ?? 'local-unset',
  status: failures.length === 0 ? 'passed' : 'failed',
  verified: failures.length === 0,
  expectedLegs,
  legs: Object.fromEntries(legs),
  files,
  aggregateBytes,
  budgets: budgets.releaseArtifacts,
  failures,
};
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify(report, null, 2) + '\n');
console.log(`RELEASE_ARTIFACT_SIZE_METRICS files=${files.length} aggregateBytes=${aggregateBytes}`);
if (failures.length) throw new Error(`release artifact size gate failed: ${failures.join('; ')}`);
