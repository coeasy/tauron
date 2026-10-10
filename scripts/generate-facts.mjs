#!/usr/bin/env node
// 事实单一真源的**生产者**：从代码/构建输入单向生成 contracts/facts.json。
//
// 为什么需要这个文件（独立审计 §P1-M1 / 易用方案 B-8 / DX-1.3）：README 徽章称 "1759 TS · 1426 Rust"、
// README 正文称 "104 测试文件 / 声明 1498"、V10 称 "1284+"、竞品文档称 "1221"——四个数**没有一个对，
// 且互相矛盾**，却被当作产品可信度卖点。根因是"文档数字"与"代码事实"之间没有单一真源，
// 六份方案文档各自手写、`docs:check` 又不覆盖方案文档，于是数字静默腐烂。
//
// 本生成器把「对外承诺的数字」变成**可复算的构建产出**：全部从代码单向计算，
// `--check` 不一致即红（纳入 `pnpm gates:check`）。任何一侧漂移都会在这里被抓到。
//
// 口径说明（诚实边界，一个字不美化）：
// - `rustTestDeclarations` = 源码里 `#[test]` 的**声明数**（grep 计数），不是某次 `cargo test` 的
//   "通过数"。两者在有 `#[ignore]`/feature 门/编译期裁剪时会不同——本文件只承诺**声明数**这个
//   可复算的静态事实；"实际跑过多少"由 CI 的 `cargo test` 汇总产出，属另一口径，不混为一谈。
// - `tsTestFiles` = `packages/**` 下 `*.test.ts` / `*.test.tsx` 的**文件数**（排除 dist/node_modules），
//   与"用例（it/test）数"是两个不同的东西，这里如实标"文件数"。
// - `commandSurface` 解析 `tauron-adapter/src/lib.rs` 的三个编译期命令集合，与
//   `generate-command-surface.mjs` 同源同形（底座 / 运行时 / 安装；合计去重）。
//
// 用法：node scripts/generate-facts.mjs          # 写文件
//      node scripts/generate-facts.mjs --check   # 只判定（CI）
//      node scripts/generate-facts.mjs --self-test  # 变异夹具自检（证明门禁真有牙）

import { readFileSync, readdirSync, statSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const OUT_REL = 'contracts/facts.json';
const read = (rel) => readFileSync(join(ROOT, rel), 'utf8');

/** 递归收集满足 `pred(relPath, name)` 的文件相对路径（跳过 node_modules/dist/.git/target）。 */
function walk(dir, out = []) {
  const abs = join(ROOT, dir);
  if (!existsSync(abs)) return out;
  for (const name of readdirSync(abs)) {
    if (name === 'node_modules' || name === 'dist' || name === 'target' || name.startsWith('.'))
      continue;
    const rel = dir ? `${dir}/${name}` : name;
    const st = statSync(join(ROOT, rel));
    if (st.isDirectory()) walk(rel, out);
    else out.push(rel);
  }
  return out;
}

/** 数一个源文件里 `#[test]` 属性出现次数（含 `#[test]` 独占行；不含注释掉的行）。 */
function countRustTestAttrs(rel) {
  const lines = read(rel).split(/\r?\n/);
  let n = 0;
  for (const l of lines) {
    const t = l.trim();
    if (t.startsWith('//') || t.startsWith('///') || t.startsWith('//!')) continue;
    if (/#\[test(\b|\]\s)/.test(t) || /^#\[test\]$/.test(t)) n++;
  }
  return n;
}

/** 从 lib.rs 提取一个命令集合（`pub const NAME: &[&str] = &[...]`）里的字符串字面量。 */
function commandArray(src, name) {
  const m = src.match(new RegExp(`pub const ${name}:[^=]*=\\s*&?\\[([\\s\\S]*?)\\];`));
  if (!m) throw new Error(`找不到命令数组 ${name}`);
  return [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
}

/** 计算全部事实（无时间戳——保证 `--check` 稳定复算）。 */
function computeFacts() {
  // 1. Rust #[test] 声明数。
  const rsFiles = walk('crates').filter((p) => p.endsWith('.rs'));
  let rustTestDeclarations = 0;
  for (const f of rsFiles) rustTestDeclarations += countRustTestAttrs(f);

  // 2. TS 测试文件数（.test.ts / .test.tsx，排除 dist/node_modules）。
  const tsFiles = walk('packages').filter(
    (p) => /(^|\/)packages\/.*\.test\.tsx?$/.test(p) || /\.test\.tsx?$/.test(p),
  );
  const tsTestFiles = new Set(tsFiles.filter((p) => p.startsWith('packages/'))).size;

  // 3. crates / packages 数。
  const crateDirs = walk('crates')
    .filter((p) => p.endsWith('Cargo.toml'))
    .map((p) => p.split('/').slice(0, 2).join('/'));
  const crateCount = new Set(crateDirs).size;
  const pkgDirs = walk('packages')
    .filter((p) => p.endsWith('package.json'))
    .map((p) => p.split('/').slice(0, 2).join('/'));
  const packageCount = new Set(pkgDirs).size;

  // 4. 命令面（与 generate-command-surface.mjs 同源）。
  const adapter = read('crates/tauron-adapter/src/lib.rs');
  const substrate = commandArray(adapter, 'SUBSTRATE_COMMANDS');
  const runtime = commandArray(adapter, 'PLUGIN_RUNTIME_COMMANDS');
  const install = commandArray(adapter, 'PLUGIN_INSTALL_COMMANDS');
  const surfaceTotal = new Set([...substrate, ...runtime, ...install]).size;

  // 5. 版本（repo/workspace 口径；registry 实时版本由 version:check 联网比对，不在静态事实里钉死）。
  const cargoVersion = (read('Cargo.toml').match(/^version\s*=\s*"([^"]+)"/m) || [])[1];
  const npmVersion = JSON.parse(read('package.json')).version;

  return {
    version: 1,
    description:
      '对外承诺数字的单一真源。由 `scripts/generate-facts.mjs` 从代码复算；README/文档读取本文件。手写数字即 bug。',
    rust: {
      testDeclarations: rustTestDeclarations,
      note: '源码 #[test] 声明数（静态可复算），非某次 cargo test 的通过数。',
      crateCount,
    },
    typescript: {
      testFileCount: tsTestFiles,
      note: 'packages/** 下 *.test.ts(x) 文件数（排除 dist/node_modules），非用例数。',
      packageCount,
    },
    commandSurface: {
      total: surfaceTotal,
      substrate: substrate.length,
      pluginRuntime: runtime.length,
      pluginInstall: install.length,
      note: '解析 tauron-adapter/src/lib.rs 三个编译期命令集合，与 command-surface:check 同源。',
    },
    versioning: {
      repoCargoWorkspaceVersion: cargoVersion,
      rootNpmVersion: npmVersion,
      note: 'registry 实际发布版本由 `pnpm version:check` 联网比对 dist-tag，不在本静态文件钉死（避免离线不可复算）。',
    },
  };
}

function serialize(facts) {
  return JSON.stringify(facts, null, 2) + '\n';
}

const README_REL = 'README.md';

/**
 * 把 README 里「可复算的静态事实」强制对齐 facts（单一真源**写向文档**）。
 * 判据锚点用空格宽容的正则；任一锚点找不到即记入 `missing`（锚点被改/删＝承诺破裂，必须红，
 * 绝不能因为匹配不到就静默放行——那正是本仓反复踩过的「针门无牙」陷阱）。
 * 返回 { text, missing }；`text` 是同步后的完整 README。
 */
function syncReadme(text, f) {
  const missing = [];
  let out = text;
  const apply = (re, make) => {
    if (!re.test(out)) {
      missing.push(re.source);
      return;
    }
    out = out.replace(re, make);
  };
  apply(
    /tests-\d+%20TS%20files%20%C2%B7%20\d+%20Rust%20%23%5Btest%5D/,
    () =>
      `tests-${f.typescript.testFileCount}%20TS%20files%20%C2%B7%20${f.rust.testDeclarations}%20Rust%20%23%5Btest%5D`,
  );
  apply(
    /\|\s*Rust\s*`#\[test\]`\s*声明数\s*\|\s*\*\*\d+\*\*/,
    () => `| Rust \`#[test]\` 声明数 | **${f.rust.testDeclarations}**`,
  );
  apply(
    /\|\s*TypeScript\s*测试文件数\s*\|\s*\*\*\d+\*\*/,
    () => `| TypeScript 测试文件数 | **${f.typescript.testFileCount}**`,
  );
  apply(
    /\|\s*crate\s*\/\s*package 数\s*\|\s*\*\*\d+\s*\/\s*\d+\*\*/,
    () => `| crate / package 数 | **${f.rust.crateCount} / ${f.typescript.packageCount}**`,
  );
  apply(
    /\|\s*命令面（总\s*\/\s*底座\s*\/\s*运行时\s*\/\s*安装）\s*\|\s*\*\*\d+\s*\/\s*\d+\s*\/\s*\d+\s*\/\s*\d+\*\*/,
    () =>
      `| 命令面（总 / 底座 / 运行时 / 安装） | **${f.commandSurface.total} / ${f.commandSurface.substrate} / ${f.commandSurface.pluginRuntime} / ${f.commandSurface.pluginInstall}**`,
  );
  return { text: out, missing };
}

// ── 变异夹具自检：证明"代码变了、facts 没跟着变"时 `--check` 会真的红 ─────────────
// 独立审计 §P1-M4 指出多数针门没有 `--self-test`，改名/绕过即静默绿。这里给 facts 门补上：
// 用一份"少算一个 crate"的伪事实跑比对逻辑，必须判定为不一致。
function selfTest() {
  const real = computeFacts();
  const tampered = structuredClone(real);
  tampered.rust.testDeclarations += 1; // 注入一个漂移
  const okConsistent = serialize(real) === serialize(computeFacts());
  const caughtDrift = serialize(tampered) !== serialize(real);
  // README 同步也必须有牙：当前 README 视为与真值同频，喂入被篡改的 facts 必须把它改写
  // （数字变动）或暴露缺失锚点——否则「README 由 facts 生成」这句承诺就是空话。
  const baseReadme = read(README_REL);
  const synced = syncReadme(baseReadme, real);
  const readmeHasAnchors = synced.missing.length === 0;
  const tamperedSync = syncReadme(baseReadme, tampered);
  const caughtReadmeDrift = tamperedSync.missing.length > 0 || tamperedSync.text !== baseReadme;
  if (okConsistent && caughtDrift && readmeHasAnchors && caughtReadmeDrift) {
    console.log('facts self-test OK：复算稳定，facts.json 与 README 的注入漂移都会被 --check 抓到');
    return;
  }
  console.error(
    `facts self-test FAILED：门禁无牙（复算不稳定 / facts 漂移未抓 / README 锚点缺失=${synced.missing.length} / README 漂移未抓）`,
  );
  process.exit(1);
}

const facts = computeFacts();
const out = serialize(facts);
const target = resolve(ROOT, OUT_REL);

if (process.argv.includes('--self-test')) {
  selfTest();
} else if (process.argv.includes('--check')) {
  let current = '';
  try {
    current = readFileSync(target, 'utf8');
  } catch {
    current = '';
  }
  const failures = [];
  if (current !== out) {
    failures.push(
      `✗ ${OUT_REL} 与代码不同步（测试数/命令面/包数/版本变了）。跑 \`node scripts/generate-facts.mjs\` 重新生成后一并提交。`,
    );
  }
  const readmeCur = read(README_REL);
  const syncedReadme = syncReadme(readmeCur, facts);
  if (syncedReadme.missing.length > 0) {
    failures.push(
      `✗ README 缺少预期锚点（「README 数字由 facts 生成」的承诺已被破坏，锚点被改/删）：\n    ${syncedReadme.missing.join('\n    ')}`,
    );
  } else if (syncedReadme.text !== readmeCur) {
    failures.push(
      '✗ README 里的测试数/命令面/包数与 contracts/facts.json 不同步。跑 `node scripts/generate-facts.mjs` 让它重写 README 后一并提交。',
    );
  }
  if (failures.length > 0) {
    console.error(failures.join('\n'));
    process.exit(1);
  }
  console.log(
    `Facts OK: Rust #[test] 声明 ${facts.rust.testDeclarations}（${facts.rust.crateCount} crate）· ` +
      `TS 测试文件 ${facts.typescript.testFileCount}（${facts.typescript.packageCount} 包）· ` +
      `命令面 ${facts.commandSurface.total}（底座 ${facts.commandSurface.substrate} / ` +
      `运行时 ${facts.commandSurface.pluginRuntime} / 安装 ${facts.commandSurface.pluginInstall}）· ` +
      `版本 ${facts.versioning.repoCargoWorkspaceVersion}（README 数字与 facts 同步）`,
  );
} else {
  writeFileSync(target, out);
  const readmeCur = read(README_REL);
  const syncedReadme = syncReadme(readmeCur, facts);
  if (syncedReadme.missing.length > 0) {
    console.error(
      `✗ README 缺少预期锚点，无法完成生成（锚点被改/删）：\n    ${syncedReadme.missing.join('\n    ')}`,
    );
    process.exit(1);
  }
  if (syncedReadme.text !== readmeCur) writeFileSync(join(ROOT, README_REL), syncedReadme.text);
  console.log(
    `wrote ${OUT_REL}: Rust ${facts.rust.testDeclarations} · TS ${facts.typescript.testFileCount} · 命令 ${facts.commandSurface.total}（README 数字已同步）`,
  );
}
