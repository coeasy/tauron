#!/usr/bin/env node
// tauron crate 发布编排（Node 22，零新依赖）
//
// 用法：
//   node scripts/publish-crates.mjs            # 默认 = --check：从 crate tarball 构建校验，不发布
//   node scripts/publish-crates.mjs --check    # 同上（显式）
//   node scripts/publish-crates.mjs --publish  # 真发布（需要 CARGO_REGISTRY_TOKEN，缺失即拒绝）
//
// 为什么必须按拓扑顺序发（债务 #5 的事实）：
//   `cargo package/publish` 会把 manifest 里的 `path` 依赖**剥离**，只留 `version`；
//   于是 `cargo publish` 时 cargo 会去 crates.io 解析内部依赖 —— 被依赖的 crate
//   必须先出现在 registry 上，否则解析失败。顺序由 `cargo metadata` 实算，不写死。
//
// 本机限制（如实标注）：
//   沙箱无网络、无 crates.io 凭据，**无法真的发布**，也无法与真实 registry 交互。
//   因此 `--check` 的验收口径包含真实 tarball 编译：产物可离线产出并构建，产物内的
//   Cargo.toml 已把 path 剥成 version。为了让 cargo 在**无网络**下也能完成打包，
//   本脚本在 --check 时用 `--config patch.crates-io.<dep>.path=...` 把内部依赖
//   指回本地路径，仅为满足依赖解析；**这不改变产出的 manifest**（脚本会解包实测
//   断言：产物里只剩 version、没有 path）。
//
// 副作用提醒：`cargo package` / `cargo metadata`（不带 --locked 时）会按需刷新
// 仓库根的 `Cargo.lock`。本脚本在 metadata 阶段优先用 `--locked` 做校验，锁文件
// 落后时会**明确告警**再降级重算，不会静默漂移。
import { execFileSync, execSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readTarball } from './lib/tar.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const args = new Set(process.argv.slice(2));
const doPublish = args.has('--publish');
const isCheck = !doPublish;

const log = (msg = '') => console.log(msg);
const fail = (msg) => console.error(`\x1b[31m✗ ${msg}\x1b[0m`);
const ok = (msg) => console.log(`\x1b[32m✓ ${msg}\x1b[0m`);
const warn = (msg) => console.log(`\x1b[33m! ${msg}\x1b[0m`);

function run(cmd, cmdArgs, cwd) {
  const opts = { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] };
  if (process.platform === 'win32') {
    const q = (a) => (/[\s"]/.test(a) ? `"${a}"` : a);
    return execSync([cmd, ...cmdArgs.map(q)].join(' '), opts);
  }
  return execFileSync(cmd, cmdArgs, opts);
}

function reportMetadataFailure(error) {
  const stderr = (error.stderr?.toString() || error.message).trim();
  const uncached = /failed to download `([^`]+)`/.exec(stderr)?.[1];
  if (/attempting to make an HTTP request, but --offline was specified/i.test(stderr)) {
    fail(
      uncached ? `cargo metadata 缺少离线缓存依赖：${uncached}` : 'cargo metadata 缺少离线缓存依赖',
    );
    log('  请先在可联网环境运行 `cargo fetch --locked`，再重新执行 crate 打包检查。');
  } else {
    fail('cargo metadata 失败');
  }
  console.error(stderr);
}

// ── 1. 用 cargo metadata 实算拓扑序 ──────────────────────────────
log('── 1/3 读取 cargo metadata，实算依赖拓扑序 ──');
let meta;
const metaArgs = ['metadata', '--format-version', '1', '--offline'];
try {
  // 首选 --locked：锁文件与 Cargo.toml 不一致时应报错，而不是静默改锁。
  meta = JSON.parse(run('cargo', [...metaArgs.slice(0, 3), '--locked', '--offline'], ROOT));
} catch (e) {
  const stderr = (e.stderr?.toString() || e.message).trim();
  if (/because --locked was passed/i.test(stderr)) {
    // 仓库正在并发改动 crate 依赖时锁文件会短暂落后；此时降级到 --offline 重试，
    // 但**明确告警**，避免把「锁文件漂移」当成正常状态。
    warn('Cargo.lock 与 Cargo.toml 不一致（--locked 校验未过），降级为 --offline 重算。');
    warn(`  cargo 原文：${stderr.split('\n')[0]}`);
    warn('  提示：提交前应把 Cargo.lock 更新后再跑一次 --check。');
    try {
      meta = JSON.parse(run('cargo', metaArgs, ROOT));
    } catch (e2) {
      reportMetadataFailure(e2);
      process.exit(1);
    }
  } else {
    reportMetadataFailure(e);
    process.exit(1);
  }
}

const targetDir = meta.target_directory;
// 只取本仓库 crates/ 下的 workspace 成员（examples/minimal-app 被 workspace exclude，天然不在内）
const crates = meta.packages
  .filter((p) => meta.workspace_members.includes(p.id))
  .filter((p) => relative(ROOT, p.manifest_path).replace(/\\/g, '/').startsWith('crates/'))
  .map((p) => ({
    name: p.name,
    version: p.version,
    manifest: p.manifest_path,
    dir: dirname(p.manifest_path),
    internal: p.dependencies
      .filter((d) => d.path)
      .map((d) => d.name)
      .sort(),
  }))
  .sort((a, b) => a.name.localeCompare(b.name));

const byName = new Map(crates.map((c) => [c.name, c]));
if (byName.size !== crates.length) fail('存在重名 crate，异常');

const order = [];
const seen = new Set();
const visiting = new Set();
function visit(name) {
  if (seen.has(name)) return;
  if (visiting.has(name)) throw new Error(`检测到循环依赖：${name}`);
  const c = byName.get(name);
  if (!c) return; // 外部依赖
  visiting.add(name);
  for (const dep of c.internal) visit(dep);
  visiting.delete(name);
  seen.add(name);
  order.push(c);
}
for (const c of crates) visit(c.name);

log(`\n── 发布顺序（共 ${order.length} 个 crate，被依赖者先发）──`);
order.forEach((c, i) => {
  const deps = c.internal.length ? `  ← 依赖: ${c.internal.join(', ')}` : '  ← 无内部依赖（叶子）';
  log(`  ${String(i + 1).padStart(2)}. ${c.name}@${c.version}${deps}`);
});

// patch 参数：把每个 crate 都 patch 回本地，供无网络解析
const patchArgs = crates.flatMap((c) => [
  '--config',
  `patch.crates-io.${c.name}.path='${relative(ROOT, c.dir).replace(/\\/g, '/')}'`,
]);

// ── 2. 逐 crate 打包 + 校验产物 manifest ─────────────────────────
log(
  '\n── 2/3 逐 crate 打包、从 tarball 构建并校验产物 manifest（cargo package --allow-dirty --offline）──',
);
warn(
  '打包阶段将内部 crate 解析到本地 workspace；cargo package 产物会移除 path 依赖，' +
    '并逐 crate 检查发布 manifest 只保留版本约束。',
);

function internalDepVersionsInManifest(manifestText) {
  // 解析生成的 Cargo.toml，收集 [dependencies.*] / [dev-dependencies.*] / [build-dependencies.*]
  const found = new Map(); // depName -> { version?, hasPath }
  let current = null;
  for (const line of manifestText.split(/\r?\n/)) {
    const sec = line.match(/^\[(dev-|build-)?dependencies\.([^\]]+)\]/);
    if (sec) {
      current = sec[2];
      // 生成的 manifest 里依赖名可能带引号
      current = current.replace(/^["']|["']$/g, '');
      found.set(current, { version: null, hasPath: false });
      continue;
    }
    if (/^\[/.test(line)) {
      current = null;
      continue;
    }
    if (!current) continue;
    const version = line.match(/^version\s*=\s*"([^"]+)"/);
    if (version) found.get(current).version = version[1];
    if (/^path\s*=/.test(line)) found.get(current).hasPath = true;
  }
  return found;
}

let failures = 0;
for (const c of order) {
  const pkgArgs = ['package', '-p', c.name, '--allow-dirty', '--offline'];
  // Pre-package the complete release set before publishing any crate. This must
  // work in both check and publish modes because later crates may not yet exist
  // on crates.io; the patch is only used for resolution and is not shipped.
  const withPatch = [...pkgArgs, ...patchArgs];
  let out = '';
  try {
    out = run('cargo', withPatch, ROOT);
  } catch (e) {
    failures++;
    fail(`${c.name}：cargo package 失败，按拓扑序中止`);
    console.error((e.stderr?.toString() || e.message).trim());
    // 中止并打印后续顺序，便于人工接手
    if (order.indexOf(c) + 1 < order.length) {
      log('  尚未打包的后续 crate（需在修好前一个后继续）：');
      for (const rest of order.slice(order.indexOf(c) + 1)) log(`    - ${rest.name}`);
    }
    process.exit(1);
  }

  const crateFile = join(targetDir, 'package', `${c.name}-${c.version}.crate`);
  const problems = [];
  if (!existsSync(crateFile)) {
    problems.push(`未产出 ${relative(ROOT, crateFile)}`);
  } else {
    const entries = readTarball(crateFile);
    const manifest = entries.find(
      (e) => e.path.endsWith('/Cargo.toml') && e.path.split('/').length === 2,
    );
    if (!manifest) {
      problems.push('产物内找不到 Cargo.toml');
    } else {
      const deps = internalDepVersionsInManifest(manifest.content.toString('utf8'));
      for (const dep of c.internal) {
        const info = deps.get(dep);
        if (!info) problems.push(`产物 manifest 缺少内部依赖 [dependencies.${dep}]`);
        else {
          if (info.hasPath) problems.push(`内部依赖 ${dep} 仍有 path =（发布时会解析失败）`);
          if (!info.version)
            problems.push(`内部依赖 ${dep} 缺少 version =（cargo publish 会拒绝）`);
        }
      }
      const srcCount = entries.filter((e) => e.path.includes('/src/')).length;
      if (srcCount === 0) problems.push('产物内不含 src/ 源码');
    }
  }

  if (problems.length === 0) {
    ok(
      `${c.name}@${c.version}  ${relative(ROOT, crateFile).replace(/\\/g, '/')}  ${out.trim().split('\n').pop()}`,
    );
  } else {
    failures++;
    fail(`${c.name}：产物校验未通过`);
    for (const pb of problems) log(`    - ${pb}`);
  }
}

// ── 3. 结论 / 发布 ──────────────────────────────────────────────
log('\n── 3/3 结论 ──');
log(`crate 共 ${order.length} 个；产物校验失败 ${failures} 个。`);

if (isCheck) {
  if (failures) {
    fail('--check 未通过：存在打包内容问题（见上）。');
    process.exit(1);
  }
  ok(
    `--check 通过：${order.length} 个 crate 均可离线产出并构建 .crate，` +
      '且产物 manifest 已把内部 path 依赖剥成 version。',
  );
  warn('注意：本次**没有真的发布**。真实发布需先按上面的顺序逐个 `cargo publish`，');
  warn('      且必须由**已配置 CARGO_REGISTRY_TOKEN 且有网络**的环境执行，本机沙箱不具备该条件。');
  process.exit(0);
}

// 真发布分支
const token = process.env.CARGO_REGISTRY_TOKEN;
if (!token) {
  fail('--publish 需要环境变量 CARGO_REGISTRY_TOKEN，当前缺失，拒绝执行。');
  log('  用法：CARGO_REGISTRY_TOKEN=<crates.io token> node scripts/publish-crates.mjs --publish');
  process.exit(1);
}
if (failures) {
  fail('存在产物校验失败的 crate，拒绝进入发布阶段。');
  process.exit(1);
}

log('\n── 开始真实发布（严格按拓扑顺序；cargo 会自动等待依赖在 index 上生效）──');
async function crateVersionPublished(crate) {
  const response = await fetch(`https://crates.io/api/v1/crates/${crate.name}`, {
    headers: { 'user-agent': 'Tauron SDK release publisher' },
  });
  if (response.status === 404) return false;
  if (!response.ok) throw new Error(`crates.io returned HTTP ${response.status}`);
  const details = await response.json();
  return details.versions?.some((version) => version.num === crate.version) ?? false;
}

for (const c of order) {
  let alreadyPublished = false;
  try {
    alreadyPublished = await crateVersionPublished(c);
  } catch (e) {
    fail(`无法确认 ${c.name}@${c.version} 的 registry 状态：${e.message}`);
    process.exit(1);
  }
  if (alreadyPublished) {
    console.log(`→ ${c.name}@${c.version} 已存在于 crates.io，跳过（支持安全续发）`);
    continue;
  }
  let rateLimitRetries = 0;
  while (true) {
    process.stdout.write(`→ cargo publish -p ${c.name} ... `);
    try {
      run('cargo', ['publish', '-p', c.name, '--allow-dirty'], ROOT);
      console.log('\x1b[32m成功\x1b[0m');
      break;
    } catch (e) {
      console.log('\x1b[31m失败\x1b[0m');
      const output = (e.stdout?.toString() ?? '') + (e.stderr?.toString() ?? '');
      const retryAfter = /try again after ([^\r\n]+?) and see https?:\/\//i.exec(output)?.[1];
      const retryAt = retryAfter ? Date.parse(retryAfter) : Number.NaN;
      if (
        /429 Too Many Requests/i.test(output) &&
        Number.isFinite(retryAt) &&
        rateLimitRetries < 3
      ) {
        rateLimitRetries++;
        const waitMs = Math.max(0, retryAt - Date.now()) + 1500;
        console.log(
          `crates.io 限流，按服务端时间 ${new Date(retryAt).toISOString()} UTC 等待后自动续发（${rateLimitRetries}/3）。`,
        );
        await new Promise((resolve) => setTimeout(resolve, waitMs));
        try {
          if (await crateVersionPublished(c)) {
            console.log(`→ ${c.name}@${c.version} 已在等待期间上架，跳过重复上传`);
            break;
          }
        } catch (statusError) {
          fail(`等待后无法确认 ${c.name}@${c.version} 状态：${statusError.message}`);
          process.exit(1);
        }
        continue;
      }
      if (/verified email address is required/i.test(output)) {
        fail(
          'crates.io 发布账号尚未验证邮箱。请在 https://crates.io/settings/profile 完成验证后重新运行；已发布版本会自动跳过。',
        );
      }
      console.error(output);
      process.exit(1);
    }
  }
}
ok('全部发布完成。');
