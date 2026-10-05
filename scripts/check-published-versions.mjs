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
const crates = readdirSync(join(ROOT, 'crates'), { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => {
    const manifest = readFileSync(join(ROOT, 'crates', entry.name, 'Cargo.toml'), 'utf8');
    const name = /^name\s*=\s*"([^"]+)"/m.exec(manifest)?.[1];
    if (!name) throw new Error(`Could not read crate name from crates/${entry.name}/Cargo.toml`);
    // `publish = false` / `registry = false` 的 crate 永远不可能出现在 crates.io。
    // 把它们算进核验清单，这条门禁在任何版本上都恒红，而 release.yml 的 registry-check
    // 正是 GitHub Release 的创建门槛——被剔除的项必须打印出来，不能静默少查。
    const unpublished =
      /^\s*publish\s*=\s*false\s*$/m.test(manifest) ||
      /^\s*registry\s*=\s*false\s*$/m.test(manifest);
    return { name, unpublished };
  });
const skippedCrates = crates.filter((crate) => crate.unpublished).map((crate) => crate.name);
const crateNames = crates.filter((crate) => !crate.unpublished).map((crate) => crate.name);
if (skippedCrates.length > 0) {
  console.log(
    `剔除 ${skippedCrates.length} 个 publish=false 的 crate（不参与 registry 核验）：${skippedCrates.join(', ')}`,
  );
}

// 「registry 上没有这个版本」由检查自己抛出；网络异常/超时抛的是别人的错误，不带包名。
// 所以身份（registry + 包名）由 checks 数组带着走，输出时统一格式化一次，
// 否则汇总只能写出「有一项未能核实」而没人知道是哪一项。
class NotPublished extends Error {}

const checks = [
  ...packageNames.map((name) => ({
    registry: 'npm',
    bucket: 'npm',
    name,
    run: async () => {
      const url = `https://registry.npmjs.org/${encodeURIComponent(name)}/${version}`;
      const response = await fetch(url, { signal: AbortSignal.timeout(20_000) });
      if (!response.ok) throw new NotPublished(`is unavailable (HTTP ${response.status}).`);
      const metadata = await response.json();
      if (metadata.version !== version)
        throw new NotPublished(`resolved unexpected version ${metadata.version}.`);
    },
  })),
  ...crateNames.map((name) => ({
    registry: 'crates.io',
    bucket: 'crates',
    name,
    run: async () => {
      const response = await fetch(`https://crates.io/api/v1/crates/${name}/${version}`, {
        headers: { 'user-agent': 'Tauron SDK release check' },
        signal: AbortSignal.timeout(20_000),
      });
      if (!response.ok) throw new NotPublished(`is unavailable (HTTP ${response.status}).`);
      const metadata = await response.json();
      if (metadata.version?.num !== version || metadata.version?.yanked)
        throw new NotPublished('is absent, mismatched, or yanked.');
    },
  })),
];

const failures = [];
for (let i = 0; i < checks.length; i += 5) {
  const batch = checks.slice(i, i + 5);
  const results = await Promise.allSettled(batch.map((check) => check.run()));
  batch.forEach((check, j) => {
    if (results[j].status === 'fulfilled')
      console.log(`${check.registry} ${check.name}@${version}: published`);
    else failures.push({ check, reason: results[j].reason });
  });
}

const missing = { npm: [], crates: [] };
const unverifiable = [];
for (const { check, reason } of failures) {
  const message = reason instanceof Error ? reason.message : String(reason);
  console.error(`✗ ${check.registry} ${check.name}@${version}: ${message}`);
  if (reason instanceof NotPublished) missing[check.bucket].push(check.name);
  else unverifiable.push(`${check.registry} ${check.name}`);
}

if (failures.length > 0) {
  // 这道门是 `release.yml` 里创建 GitHub Release 的前置作业：红掉就不会有任何公开产物，
  // 所以失败输出必须直接说清「缺什么、哪些没查成、去哪儿补」，而不是让人去读 workflow。
  console.error(
    `\n共 ${failures.length} 项在 ${version} 上未通过核验：` +
      `npm 缺 ${missing.npm.length} 个、crates.io 缺 ${missing.crates.length} 个` +
      (unverifiable.length > 0 ? `、未能核实 ${unverifiable.length} 个` : '') +
      '。',
  );
  if (missing.npm.length > 0) console.error(`  缺 npm：${missing.npm.join(', ')}`);
  if (missing.crates.length > 0) console.error(`  缺 crates.io：${missing.crates.join(', ')}`);
  if (unverifiable.length > 0)
    console.error(
      `  未能核实（网络/超时，判红但不等于未发布，重跑即可复核）：${unverifiable.join(', ')}`,
    );
  console.error(
    '  补齐方式：运行 `Publish SDK packages` 工作流（workflow_dispatch，publish=true，' +
      '需仓库配置 NPM_TOKEN 与 CARGO_REGISTRY_TOKEN），再在本 run 里重跑失败的作业。',
  );
  process.exitCode = 1;
} else
  console.log(
    `All ${packageNames.length} npm packages and ${crateNames.length} Rust crates are public at ${version}.`,
  );
