#!/usr/bin/env node
// tauron npm 包发布编排（Node 22，零新依赖）
//
// 用法：
//   node scripts/publish-npm.mjs            # 默认 = --check：只构建 + 打包 + 校验，不发布
//   node scripts/publish-npm.mjs --check    # 同上（显式）
//   node scripts/publish-npm.mjs --publish  # 真发布（需要 NPM_TOKEN，缺失即拒绝）
//
// --check 做了什么（**本机可验证**）：
//   1. `pnpm -r build`（除非带 --no-build）
//   2. 按内部依赖拓扑顺序，对每个「可发布」包执行 `pnpm pack` 到临时目录
//   3. 读回 tarball，逐项校验：
//      - tarball 内确实包含 dist/ 产物
//      - 无残留 `workspace:` 协议（这是关键：npm pack 会原样保留，pnpm pack 会改写）
//      - main / module / types / exports / bin 指向的文件都在 tarball 里
//      - version 与根 package.json 同源
//   4. 打印发布顺序与中文结论；任一包失败 → 退出码 1
//
// --publish 做了什么（**本机无法验证**：沙箱无网络、无凭据）：
//   真跑 `pnpm publish`，必须显式提供 NPM_TOKEN，否则拒绝执行。
import { execFileSync, execSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readTarball } from './lib/tar.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const PACKAGES_DIR = join(ROOT, 'packages');

const args = new Set(process.argv.slice(2));
const doPublish = args.has('--publish');
const skipBuild = args.has('--no-build');
const isCheck = !doPublish;
// Windows 上 pnpm 是 .cmd 垫片，execFileSync 不带 shell 会 ENOENT
const PNPM = process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm';

const log = (msg = '') => console.log(msg);
const fail = (msg) => console.error(`\x1b[31m✗ ${msg}\x1b[0m`);
const ok = (msg) => console.log(`\x1b[32m✓ ${msg}\x1b[0m`);
const warn = (msg) => console.log(`\x1b[33m! ${msg}\x1b[0m`);

// Windows 上 pnpm 是 .cmd 垫片：Node 20+ 不允许在不开 shell 的情况下直接执行 .cmd（EINVAL），
// 因此 win32 走 execSync（自己给含空格的参数加引号），POSIX 走 execFileSync。
function run(cmd, cmdArgs, cwd, extraEnv) {
  const opts = { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] };
  if (extraEnv) opts.env = extraEnv;
  if (process.platform === 'win32') {
    const q = (a) => (/[\s"]/.test(a) ? `"${a}"` : a);
    return execSync([cmd, ...cmdArgs.map(q)].join(' '), opts);
  }
  return execFileSync(cmd, cmdArgs, opts);
}

// ── 1. 读取全部包 ────────────────────────────────────────────────
const rootVersion = JSON.parse(readFileSync(join(ROOT, 'package.json'), 'utf8')).version;
const all = readdirSync(PACKAGES_DIR)
  .map((dir) => ({ dir, file: join(PACKAGES_DIR, dir, 'package.json') }))
  .filter((p) => existsSync(p.file))
  .map((p) => ({ ...p, pkg: JSON.parse(readFileSync(p.file, 'utf8')) }))
  .sort((a, b) => a.dir.localeCompare(b.dir));

const publishable = all.filter((p) => p.pkg.private !== true);
const skipped = all.filter((p) => p.pkg.private === true);

// ── 2. 拓扑排序（被依赖者先发）───────────────────────────────────
const byName = new Map(publishable.map((p) => [p.pkg.name, p]));
const internalDeps = (p) => {
  const fields = ['dependencies', 'peerDependencies', 'optionalDependencies'];
  const out = new Set();
  for (const f of fields) {
    for (const [name, spec] of Object.entries(p.pkg[f] ?? {})) {
      if (String(spec).startsWith('workspace:') || byName.has(name)) out.add(name);
    }
  }
  return [...out];
};

const order = [];
const seen = new Set();
const visiting = new Set();
function visit(name) {
  if (seen.has(name)) return;
  const p = byName.get(name);
  if (!p) throw new Error(`包 ${name} 不在可发布集合中（可能仍是 private，或依赖写错）`);
  if (visiting.has(name)) throw new Error(`检测到循环依赖：${name}`);
  visiting.add(name);
  for (const dep of internalDeps(p).sort()) visit(dep);
  visiting.delete(name);
  seen.add(name);
  order.push(p);
}
for (const p of publishable) visit(p.pkg.name);

log('── 发布顺序（内部依赖拓扑序，被依赖者先发）──');
order.forEach((p, i) => log(`  ${String(i + 1).padStart(2)}. ${p.pkg.name}  [packages/${p.dir}]`));
if (skipped.length) {
  log('');
  log('── 跳过（保持 private=true，不参与发布）──');
  skipped.forEach((p) => log(`  - ${p.pkg.name}  [packages/${p.dir}]`));
}

// ── 3. 构建 ─────────────────────────────────────────────────────
if (!skipBuild) {
  log('\n── 1/3 按依赖顺序构建全部 TS 包：pnpm -r build ──');
  try {
    run(PNPM, ['-r', 'build'], ROOT);
    ok('pnpm -r build 完成');
  } catch (e) {
    fail('pnpm -r build 失败，中止');
    console.error(e.stdout?.toString() ?? '');
    console.error(e.stderr?.toString() ?? '');
    process.exit(1);
  }
} else {
  warn('已跳过构建（--no-build）');
}

// ── 4. 逐包 pack + 校验 ──────────────────────────────────────────
const tmpRoot = mkdtempSync(join(tmpdir(), 'tauron-npm-pack-'));
let failures = 0;

log('\n── 2/3 逐包 pnpm pack 并解包校验 tarball 内容 ──');

function collectEntryPaths(pkg) {
  const wanted = new Map(); // 目标路径 → 字段说明
  const rel = (v) => String(v).replace(/^\.\//, '');
  if (pkg.main) wanted.set(rel(pkg.main), 'main');
  if (pkg.module) wanted.set(rel(pkg.module), 'module');
  if (pkg.types) wanted.set(rel(pkg.types), 'types');
  if (pkg.bin) {
    const bins = typeof pkg.bin === 'string' ? { [pkg.name]: pkg.bin } : pkg.bin;
    for (const v of Object.values(bins)) wanted.set(rel(v), 'bin');
  }
  for (const [sub, val] of Object.entries(pkg.exports ?? {})) {
    if (typeof val === 'string') wanted.set(rel(val), `exports["${sub}"]`);
    else
      for (const [cond, v] of Object.entries(val ?? {}))
        wanted.set(rel(v), `exports["${sub}"].${cond}`);
  }
  return wanted;
}

const packed = [];
for (const p of order) {
  const dest = join(tmpRoot, p.dir);
  const problems = [];
  try {
    run(PNPM, ['pack', '--pack-destination', dest], join(PACKAGES_DIR, p.dir));
  } catch (e) {
    failures++;
    fail(`${p.pkg.name}：pnpm pack 失败`);
    console.error((e.stderr?.toString() || e.message).trim());
    continue;
  }
  const tgzName = readdirSync(dest).find((f) => f.endsWith('.tgz'));
  if (!tgzName) {
    failures++;
    fail(`${p.pkg.name}：未产出 .tgz`);
    continue;
  }
  const tgz = join(dest, tgzName);
  const entries = readTarball(tgz);
  const paths = new Set(entries.map((e) => e.path));

  // (a) 版本号同源
  const manifestEntry = entries.find((e) => e.path === 'package/package.json');
  if (!manifestEntry) problems.push('tarball 内缺少 package/package.json');
  const packedPkg = manifestEntry ? JSON.parse(manifestEntry.content.toString('utf8')) : {};
  if (packedPkg.version !== rootVersion) {
    problems.push(`version=${packedPkg.version}，与根 package.json 的 ${rootVersion} 不一致`);
  }
  if (packedPkg.private === true) problems.push('tarball 内仍是 private=true，npm 会拒绝发布');

  // (b) dist 产物确实在 tarball 里
  const distFiles = [...paths].filter((x) => x.startsWith('package/dist/'));
  if (distFiles.length === 0)
    problems.push('tarball 内不含 package/dist/（未构建或 files 白名单漏了 dist）');

  // (c) workspace: 协议是否残留 —— 关键事实：pnpm pack 会改写成具体版本
  const leftover = [];
  for (const f of ['dependencies', 'devDependencies', 'peerDependencies', 'optionalDependencies']) {
    for (const [name, spec] of Object.entries(packedPkg[f] ?? {})) {
      if (String(spec).includes('workspace:')) leftover.push(`${f}.${name}=${spec}`);
    }
  }
  if (leftover.length) problems.push(`残留 workspace: 协议 → ${leftover.join(', ')}`);

  // (d) 入口字段指向的文件必须存在
  for (const [rel, field] of collectEntryPaths(p.pkg)) {
    if (!paths.has(`package/${rel}`)) problems.push(`${field} 指向不存在的文件：${rel}`);
  }

  if (problems.length === 0) {
    ok(`${p.pkg.name}  ${tgzName}  dist 文件 ${distFiles.length} 个，入口完整，无 workspace: 残留`);
    packed.push({ name: p.pkg.name, tgz, dest: p.dir });
  } else {
    failures++;
    fail(`${p.pkg.name}`);
    for (const pb of problems) log(`    - ${pb}`);
  }
}

// ── 5. 结论 / 发布 ──────────────────────────────────────────────
log('\n── 3/3 结论 ──');
log(`可发布包 ${publishable.length} 个；通过校验 ${packed.length} 个；失败 ${failures} 个。`);

if (isCheck) {
  if (failures) {
    fail('--check 未通过：存在打包内容问题，见上面逐条列出。');
    rmSync(tmpRoot, { recursive: true, force: true });
    process.exit(1);
  }
  ok(
    '--check 通过：所有可发布包的 tarball 内容正确（含 dist、入口完整、版本同源、无 workspace: 残留）。',
  );
  warn('注意：本次**没有真的发布**。真实发布请显式执行 `node scripts/publish-npm.mjs --publish`。');
  rmSync(tmpRoot, { recursive: true, force: true });
  process.exit(0);
}

// 真发布分支
const token = process.env.NPM_TOKEN;
if (!token) {
  fail('--publish 需要环境变量 NPM_TOKEN，当前缺失，拒绝执行。');
  log('  用法：NPM_TOKEN=<npm 的 publish token> node scripts/publish-npm.mjs --publish');
  rmSync(tmpRoot, { recursive: true, force: true });
  process.exit(1);
}
if (failures) {
  fail('存在校验失败的包，拒绝进入发布阶段。');
  rmSync(tmpRoot, { recursive: true, force: true });
  process.exit(1);
}

log('\n── 开始真实发布（按拓扑顺序）──');
const npmrc = join(tmpRoot, '.npmrc');
writeFileSync(npmrc, `//registry.npmjs.org/:_authToken=${token}\n`, 'utf8');
const publishEnv = { ...process.env, NPM_CONFIG_USERCONFIG: npmrc };

function isPublishedVersion(pkg) {
  try {
    const found = run(
      'npm',
      ['view', `${pkg.name}@${pkg.version}`, 'version', '--json', '--fetch-retries=0'],
      ROOT,
      publishEnv,
    ).trim();
    return JSON.parse(found) === pkg.version;
  } catch (e) {
    const output = `${e.stdout?.toString() ?? ''}\n${e.stderr?.toString() ?? ''}`;
    if (/E404|404 Not Found|No match found for version/i.test(output)) return false;
    throw new Error(`无法确认 ${pkg.name}@${pkg.version} 是否已发布：${output.trim()}`);
  }
}

for (const p of order) {
  try {
    if (isPublishedVersion(p.pkg)) {
      console.log(`→ ${p.pkg.name}@${p.pkg.version} 已存在于 npm，跳过（支持安全续发）`);
      continue;
    }
  } catch (e) {
    console.error(`\x1b[31m✗ ${e.message}\x1b[0m`);
    rmSync(tmpRoot, { recursive: true, force: true });
    process.exit(1);
  }
  process.stdout.write(`→ pnpm publish ${p.pkg.name} ... `);
  try {
    run(
      PNPM,
      ['publish', '--access', 'public', '--no-git-checks'],
      join(PACKAGES_DIR, p.dir),
      publishEnv,
    );
    console.log('\x1b[32m成功\x1b[0m');
  } catch (e) {
    console.log('\x1b[31m失败\x1b[0m');
    const output = (e.stdout?.toString() ?? '') + (e.stderr?.toString() ?? '');
    if (/Scope not found/i.test(output)) {
      console.error(
        `npm scope for ${p.pkg.name} does not exist. Create the @tauron organization on npmjs.com using the free public-packages plan, ensure the publishing account is an organization owner/member, then rerun the package workflow. No later package was attempted.`,
      );
    }
    console.error(output);
    rmSync(tmpRoot, { recursive: true, force: true });
    process.exit(1);
  }
}
rmSync(tmpRoot, { recursive: true, force: true });
ok('全部发布完成。');
