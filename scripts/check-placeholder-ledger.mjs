#!/usr/bin/env node
// IND-3 · 占位登记台账（"占位必须有归宿"从口号变机器规则）
//
// 独立审计根因 1 与统一计划纪律 2 都写着同一句话：**每个诚实占位要么排进"转正"批次，
// 要么排进"删除/降级 private/移出发布面"批次，不允许无限期挂着半成品**。但既有两座针门
// （`check-simulated-never-commits.mjs` / `check-success-requires-effect.mjs`）只钉死
// **6 个具名文件里的具名函数**的行为形状——它们保证"已知的桩不说谎"，却**不保证"新长出来的
// 桩被登记"**：谁在别的源文件里加一个 `simulated: true`，那两座门一个字不会红。
//
// 本门补的正是这个洞，作用域**扩到全仓源文件**（IND-3.4 要的"作用域扩展"，但以纯反推、
// 不新增手写并行态的方式落地）：
//
//   · Inv-Scan   全仓 `simulated: true`（注释/字符串经 stripped 投影剔除，测试文件排除）
//                的**逐文件计数**必须与台账**双向相等**：源里多了没登记的文件=红（unregistered）、
//                台账里的文件源码里已没有该形状=红（stale，"转正/删除后账没销"）、数量对不上=红
//                （count，"偷加/偷删一个桩没走账")。手写即 bug，--write 复算。
//   · Inv-Disposition  每条登记必须带 `disposition ∈ {wire, downgrade-private, delete, defer}`
//                ——即纪律 2 的"归宿"，--write 只会把新文件留空，逼维护者亲自填。
//   · Inv-Batch        每条登记必须带非空 `batch`（批次归属，指向统一计划 T-x / 轮次）。
//
// 每条判据都配 --self-test 变异夹具，破坏不变量→对应规则必须转红，红不出就是回归。
//
// 用法：
//   node scripts/check-placeholder-ledger.mjs                # 默认 --check
//   node scripts/check-placeholder-ledger.mjs --check
//   node scripts/check-placeholder-ledger.mjs --write        # 复算 + 落盘 ledger（保留已有归宿，新桩留空待填）
//   node scripts/check-placeholder-ledger.mjs --self-test    # 5 条变异夹具

import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('..', import.meta.url));
const LEDGER_REL = 'contracts/placeholder-ledger.json';

const SCAN_ROOTS = ['crates', 'packages'];
const SCAN_EXTS = ['.rs', '.ts'];
const EXCLUDE_DIRS = ['/target/', '/node_modules/', '/dist/', '/build/', '/gen/'];
const EXCLUDE_NAME = /\.test\.ts$|\.spec\.ts$|\.d\.ts$/;

// 诚实占位的词汇：本仓用 `simulated: true` 标记"这一步没真做效果"。这是唯一无歧义的
// 占位标记（不像 `success: false` 会混进真实的运行时失败路径），故台账只认它。
const MARKER = /simulated:\s*true/u;

const DISPOSITIONS = {
  wire: '排入转正批次（把桩接成真链路）',
  'downgrade-private': '降级为 private / 移出发布面（保留代码，等真引擎再回公开面）',
  delete: '排入删除批次',
  defer: '明确延期（带批次归属，非无限期挂账）',
};

// comment/string 投影：与两座诚实针门共用同一套 stripped 语义，避免 README/注释/doc 里的
// `simulated: true` 被误计。仅用于定位代码级形状，不用于展示。
function stripped(lines) {
  return lines.map((line) => {
    let out = '';
    let inString = false;
    let inChar = false;
    let inTemplate = false;
    for (let i = 0; i < line.length; i += 1) {
      const ch = line[i];
      const next = line[i + 1];
      if (!inString && !inChar && !inTemplate && ch === '/' && next === '/') break;
      if (ch === '`' && !inString && !inChar) {
        inTemplate = !inTemplate;
        out += ' ';
        continue;
      }
      if (inTemplate) {
        out += ' ';
        continue;
      }
      if (ch === '"' && !inChar) {
        if (inString && line[i - 1] !== '\\') inString = false;
        else if (!inString) inString = true;
        out += ' ';
        continue;
      }
      if (ch === "'" && !inString) {
        if (inChar && line[i - 1] !== '\\') inChar = false;
        else if (!inChar) inChar = true;
        out += ' ';
        continue;
      }
      out += inString || inChar ? ' ' : ch;
    }
    return out;
  });
}

function walk(absDir, relDir, out) {
  for (const name of readdirSync(absDir)) {
    const abs = join(absDir, name);
    const rel = `${relDir}/${name}`;
    if (EXCLUDE_DIRS.some((e) => rel.includes(e))) continue;
    let stat;
    try {
      stat = statSync(abs);
    } catch {
      continue;
    }
    if (stat.isDirectory()) walk(abs, rel, out);
    else if (SCAN_EXTS.some((ext) => rel.endsWith(ext)) && !EXCLUDE_NAME.test(rel)) out.push(rel);
  }
}

function scannedFiles() {
  const rel = [];
  for (const root of SCAN_ROOTS) {
    const abs = join(ROOT, root);
    if (existsSync(abs)) walk(abs, root, rel);
  }
  return rel.sort();
}

/// 逐文件复算 `simulated: true` 的代码级出现数（stripped 投影）。
function countMarkers(text) {
  let n = 0;
  for (const line of stripped(text.split(/\r?\n/))) if (MARKER.test(line)) n += 1;
  return n;
}

function loadSources(overrides = {}) {
  const files = scannedFiles();
  const texts = {};
  for (const rel of files) texts[rel] = overrides[rel] ?? readFileSync(join(ROOT, rel), 'utf8');
  const observed = {};
  for (const rel of files) {
    const c = countMarkers(texts[rel]);
    if (c > 0) observed[rel] = c;
  }
  const ledgerRaw =
    overrides[LEDGER_REL] ??
    (existsSync(join(ROOT, LEDGER_REL)) ? readFileSync(join(ROOT, LEDGER_REL), 'utf8') : null);
  const ledger = ledgerRaw ? JSON.parse(ledgerRaw) : null;
  return { observed, ledger, fileCount: files.length };
}

function runChecks(src) {
  const failures = [];
  if (!src.ledger) {
    failures.push({ rule: 'ledger-missing', message: `${LEDGER_REL} 不存在——先 --write 生成` });
    return { failures };
  }
  const entries = Array.isArray(src.ledger.entries) ? src.ledger.entries : [];
  const byFile = new Map(entries.map((e) => [e.file, e]));

  // Inv-Scan · 双向相等：源里出现但台账没登记 = unregistered。
  for (const [file, count] of Object.entries(src.observed)) {
    const entry = byFile.get(file);
    if (!entry) {
      failures.push({
        rule: 'unregistered',
        message: `${file} 有 ${count} 处 simulated:true 占位但未登记台账——每个诚实占位必须有归宿（纪律 2）`,
      });
      continue;
    }
    if (entry.count !== count) {
      failures.push({
        rule: 'count',
        message: `${file} 复算 ${count} 处 / 台账登记 ${entry.count} 处——占位数量漂移（偷加或偷删未走账）`,
      });
    }
  }
  // Inv-Scan · 台账里有但源里已无该形状 = stale（转正/删除后没销账）。
  for (const entry of entries) {
    if (!(entry.file in src.observed)) {
      failures.push({
        rule: 'stale',
        message: `台账登记 ${entry.file}（${entry.count} 处）但源码已无 simulated:true——转正/删除后请销账`,
      });
    }
  }

  // Inv-Disposition / Inv-Batch · 归宿必须是枚举内的一种，且带批次归属。
  for (const entry of entries) {
    if (!(entry.disposition in DISPOSITIONS)) {
      failures.push({
        rule: 'disposition',
        message: `${entry.file} 的 disposition "${entry.disposition ?? ''}" 非法——须 ∈ {${Object.keys(
          DISPOSITIONS,
        ).join(', ')}}（--write 不替你决定归宿）`,
      });
    }
    if (typeof entry.batch !== 'string' || entry.batch.trim() === '') {
      failures.push({
        rule: 'batch',
        message: `${entry.file} 缺 batch——每个占位要写清排进哪个转正/降级/删除批次`,
      });
    }
  }
  return { failures };
}

function write() {
  const src = loadSources();
  const prior = new Map(
    (src.ledger && Array.isArray(src.ledger.entries) ? src.ledger.entries : []).map((e) => [
      e.file,
      e,
    ]),
  );
  const entries = Object.entries(src.observed)
    .map(([file, count]) => {
      const old = prior.get(file);
      return {
        file,
        count,
        // --write 只刷新 file+count；归宿是人的决定，绝不自动填。
        disposition: old && old.disposition in DISPOSITIONS ? old.disposition : '',
        batch: old && typeof old.batch === 'string' ? old.batch : '',
        note: old && typeof old.note === 'string' ? old.note : '',
      };
    })
    .sort((a, b) => a.file.localeCompare(b.file));
  const out = {
    schemaVersion: 1,
    purpose:
      'IND-3 占位登记台账：全仓 `simulated: true` 诚实占位的逐文件计数 + 归宿（disposition/batch），由 scripts/check-placeholder-ledger.mjs --write 复算落盘。纪律 2「占位必须有归宿、不许无限期挂半成品」的机器化——手写 count 即 bug（--check 与源码复算双向核对），新桩 --write 只留空逼维护者亲自填归宿。',
    generatedBy: 'scripts/check-placeholder-ledger.mjs --write',
    marker: 'simulated: true',
    scope: {
      roots: SCAN_ROOTS,
      languages: SCAN_EXTS,
      excludes: [...EXCLUDE_DIRS, ...['*.test.ts', '*.spec.ts', '*.d.ts']],
      note: '注释 / 字符串 / doc 里的 simulated:true 经 stripped 投影剔除，只计代码级出现。',
    },
    dispositions: DISPOSITIONS,
    rules: {
      unregistered: '源码有占位但台账没登记',
      stale: '台账登记了但源码已无该形状（转正/删除后没销账）',
      count: '同一文件的占位数量与台账不符',
      disposition: 'disposition 必须 ∈ {wire, downgrade-private, delete, defer}',
      batch: 'batch 必须是非空批次归属字符串',
    },
    entries,
  };
  writeFileSync(join(ROOT, LEDGER_REL), JSON.stringify(out, null, 2) + '\n');
  const total = entries.reduce((s, e) => s + e.count, 0);
  console.log(
    `Placeholder ledger written: ${LEDGER_REL} (${entries.length} 个占位文件 / 合计 ${total} 处 simulated:true)`,
  );
}

const CLI_FILE = 'packages/tauron-cli/src/plugin-lifecycle.ts';
const CLEAN_FILE = 'crates/tauron-host/src/error.rs'; // 当前无 simulated:true，用于 unregistered 注入
const MARKET_FILE = 'crates/tauron-market/src/api.rs'; // 2 处，用于 stale 清空

const MUTATIONS = [
  {
    name: 'M1 · 未登记源文件里长出 simulated:true',
    expect: 'unregistered',
    apply: () => {
      const text = readFileSync(join(ROOT, CLEAN_FILE), 'utf8');
      return { [CLEAN_FILE]: `${text}\nconst _probe = { simulated: true };\n` };
    },
  },
  {
    name: 'M2 · 已登记文件偷加一处占位（计数漂移）',
    expect: 'count',
    apply: () => {
      const text = readFileSync(join(ROOT, CLI_FILE), 'utf8');
      return { [CLI_FILE]: `${text}\nconst _probe = { simulated: true };\n` };
    },
  },
  {
    name: 'M3 · 清掉某文件全部占位但台账没销账（stale）',
    expect: 'stale',
    apply: () => {
      const text = readFileSync(join(ROOT, MARKET_FILE), 'utf8');
      return { [MARKET_FILE]: text.split(/simulated:\s*true/u).join('simulated: nope') };
    },
  },
  {
    name: 'M4 · 台账某条 disposition 被改成非法值',
    expect: 'disposition',
    apply: (base) => {
      const ledger = JSON.parse(JSON.stringify(base.ledger));
      if (ledger.entries.length > 0) ledger.entries[0].disposition = 'maybe-later';
      return { [LEDGER_REL]: JSON.stringify(ledger) };
    },
  },
  {
    name: 'M5 · 台账某条 batch 被清空',
    expect: 'batch',
    apply: (base) => {
      const ledger = JSON.parse(JSON.stringify(base.ledger));
      if (ledger.entries.length > 0) ledger.entries[0].batch = '';
      return { [LEDGER_REL]: JSON.stringify(ledger) };
    },
  },
];

function selfTest() {
  const base = loadSources();
  if (!base.ledger) {
    console.error('Placeholder self-test cannot start: ledger missing');
    process.exit(1);
  }
  const baseRun = runChecks(base);
  if (baseRun.failures.length > 0) {
    console.error('Placeholder self-test baseline is not green:');
    for (const f of baseRun.failures) console.error(`  · [${f.rule}] ${f.message}`);
    process.exit(1);
  }
  const results = [];
  let pass = 0;
  for (const m of MUTATIONS) {
    const mutated = loadSources(m.apply(base));
    const run = runChecks(mutated);
    const fired = run.failures.some((f) => f.rule === m.expect);
    results.push({ name: m.name, expect: m.expect, fired, seen: run.failures.map((f) => f.rule) });
    if (fired) pass += 1;
  }
  if (pass !== MUTATIONS.length) {
    console.error(
      `Placeholder self-test FAILED: ${pass}/${MUTATIONS.length} mutations fired expected rule`,
    );
    for (const r of results)
      console.error(
        `  · ${r.name} → expect=${r.expect} fired=${r.fired ? 'yes' : 'NO'} seen=[${r.seen.join(', ')}]`,
      );
    process.exit(1);
  }
  console.log(
    `Placeholder self-test OK: ${pass}/${MUTATIONS.length} mutations fired expected rule`,
  );
}

function check() {
  const src = loadSources();
  const { failures } = runChecks(src);
  if (failures.length > 0) {
    console.error('Placeholder-Ledger gate FAILED:');
    for (const f of failures) console.error(`  · [${f.rule}] ${f.message}`);
    process.exit(1);
  }
  const entries = src.ledger.entries;
  const total = entries.reduce((s, e) => s + e.count, 0);
  console.log(
    `Placeholder-Ledger gate OK：扫描 ${src.fileCount} 个源文件，登记 ${entries.length} 个占位文件 / 合计 ${total} 处 simulated:true——每条都有归宿（disposition+batch），台账计数与源码逐文件双向相等。`,
  );
}

const argv = process.argv.slice(2);
if (argv.includes('--write')) write();
else if (argv.includes('--self-test')) selfTest();
else check();
