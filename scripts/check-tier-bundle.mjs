#!/usr/bin/env node
// 轮 45（A65 Profile V2）：Tier ↔ Bundle ↔ 命令子集，三张表与真实命令面的一致性门禁。
//
// 三张表在 `contracts/tier-bundles.json`：① tiers（S/E/P 深度档，各自 requires 必备
// bundle）；② bundles（V4 §87.2 的 10 个能力/形态捆绑，各带命令子集与落地状态）；
// ③ surfaceBundles（形态类捆绑清单）。表的**读者**就是本门禁：拿真实命令面
// （`docs/api/command-surface.md`，由代码单向生成的机器真相）逐条核对——
// 「声明了但没人读」的表过不了这一关（本轮从零建模，这份门禁就是消费点）。
//
// 六条不变量，任一被破坏即非零退出：
//   I1 结构：tiers 恰为 S/E/P；bundles 恰为 §87.2 的 10 个；surfaceBundles ⊆ bundles；
//   I2 命令面同源：文档解析出的命令总数必须等于分节「**N 条**」标记之和
//      （当前 61+22+2=85）；解析失败会当场可见，不会静默空转；
//   I3 完全划分：85 条命令每条 ∈ 恰好一个 bundle（无遗漏、无重复、无未知名）——
//      分类是完整断言，不是示意；
//   I4 状态诚实：已落 ⇔ 命令子集非空（roadmap 不得白挂命令）；requires 引用的
//      bundle 必须已落；
//   I5 深度单调：requires(S) ⊂ requires(E) ⊂ requires(P) 严格子集，且只引用
//      capability 类捆绑；
//   I6 正交性（§87 的机器证明）：任何 tier 的 requires 与 surfaceBundles 交集为空
//      ——「Tier S != Window / Tier E != Desktop / Tier P != Marketplace UI」
//      从此是可判定断言，而不是文档修辞。
//
// 用法：
//   node scripts/check-tier-bundle.mjs               # 校验
//   node scripts/check-tier-bundle.mjs --self-test   # 变异自证（每条不变量都有牙）

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const TIERS_FILE = 'contracts/tier-bundles.json';
const SURFACE_FILE = 'docs/api/command-surface.md';

const CANONICAL_TIERS = ['S', 'E', 'P'];
const CANONICAL_BUNDLES = [
  'core',
  'desktop-ui',
  'mobile-ui',
  'headless',
  'service',
  'remote-client',
  'plugin-runtime',
  'marketplace',
  'observability',
  'enterprise-policy',
];
const CANONICAL_SURFACE_BUNDLES = [
  'desktop-ui',
  'mobile-ui',
  'headless',
  'service',
  'remote-client',
];

const sameSet = (a, b) =>
  a.length === b.length && [...a].sort().join('\n') === [...b].sort().join('\n');

const STATUS_SETTLED = '已落';
const STATUS_ROADMAP = 'roadmap';

// ── I2：解析生成命令面。每个 `## ` 节内的行 `| `host_xxx` …` 是命令行；
// `**N 条**` 是同一节的分节计数标记，两者必须相等。 ──
function parseCommandSurface(text) {
  const sections = [];
  let current = { section: '', marker: null, names: [] };
  for (const line of text.split('\n')) {
    const header = /^## (.+)$/.exec(line);
    if (header) {
      if (current.names.length || current.marker !== null) sections.push(current);
      current = { section: header[1], marker: null, names: [] };
      continue;
    }
    const marker = /^\*\*(\d+) 条\*\*$/.exec(line);
    if (marker && current.names.length === 0) current.marker = Number(marker[1]);
    const row = /^\|\s*`(host_[a-z0-9_]+)`/.exec(line);
    if (row) current.names.push(row[1]);
  }
  if (current.names.length || current.marker !== null) sections.push(current);
  return sections.filter((s) => s.names.length > 0 || s.marker !== null);
}

/** 载入门禁读到的两份输入；self-test 用它做「原始 → 变异」的内存快照。 */
function loadSources() {
  return {
    tiers: readFileSync(join(ROOT, TIERS_FILE), 'utf8'),
    surface: readFileSync(join(ROOT, SURFACE_FILE), 'utf8'),
  };
}

/** 对给定输入快照跑六条不变量（纯函数，供校验与 self-test 共用）。 */
function runChecks(src) {
  const failures = [];
  const fail = (msg) => failures.push(msg);

  let doc;
  try {
    doc = JSON.parse(src.tiers);
  } catch (error) {
    fail(`${TIERS_FILE}: JSON 解析失败（${error.message}）`);
    return { failures, surfaceSize: 0 };
  }

  // ── I1：表结构。 ──
  if (!sameSet(Object.keys(doc.tiers ?? {}), CANONICAL_TIERS)) {
    fail(`${TIERS_FILE}: tiers 键必须恰为 ${CANONICAL_TIERS.join('/')}`);
  }
  if (!sameSet(Object.keys(doc.bundles ?? {}), CANONICAL_BUNDLES)) {
    fail(`${TIERS_FILE}: bundles 键必须恰为 §87.2 的 10 个（少一个或别名都会被挡）`);
  }
  if (!sameSet(doc.surfaceBundles ?? [], CANONICAL_SURFACE_BUNDLES)) {
    fail(`${TIERS_FILE}: surfaceBundles 必须恰为 ${CANONICAL_SURFACE_BUNDLES.join('/')}`);
  }
  for (const key of doc.surfaceBundles ?? []) {
    if (!(key in (doc.bundles ?? {})))
      fail(`${TIERS_FILE}: surfaceBundles 的 ${key} 不在 bundles 里`);
  }

  // ── I2：命令面同源。 ──
  const sections = parseCommandSurface(src.surface);
  const surfaceNames = new Set();
  for (const s of sections) {
    if (s.marker === null) {
      fail(`${SURFACE_FILE}: 「${s.section}」节缺「**N 条**」计数标记（解析口径会漂）`);
    } else if (s.marker !== s.names.length) {
      fail(
        `${SURFACE_FILE}: 「${s.section}」标记 ${s.marker} 条 ≠ 解析到 ${s.names.length} 条（命令面解析与生成物脱节）`,
      );
    }
    for (const name of s.names) surfaceNames.add(name);
  }
  const markerSum = sections.reduce((sum, s) => sum + (s.marker ?? 0), 0);
  if (markerSum !== surfaceNames.size) {
    fail(`${SURFACE_FILE}: 分节标记合计 ${markerSum} ≠ 去重后命令数 ${surfaceNames.size}`);
  }

  // ── I3：完全划分。 ──
  const owner = new Map();
  for (const [bundle, spec] of Object.entries(doc.bundles ?? {})) {
    for (const cmd of spec.commands ?? []) {
      if (!surfaceNames.has(cmd)) {
        fail(`${TIERS_FILE}: ${bundle} 引用了命令面里不存在的 ${cmd}`);
        continue;
      }
      if (owner.has(cmd)) {
        fail(`${TIERS_FILE}: ${cmd} 同时属于 ${owner.get(cmd)} 与 ${bundle}（划分必须互斥）`);
        continue;
      }
      owner.set(cmd, bundle);
    }
  }
  for (const name of surfaceNames) {
    if (!owner.has(name)) fail(`${TIERS_FILE}: ${name} 不属于任何 bundle（分类不完整）`);
  }

  // ── I4：状态诚实。 ──
  for (const [bundle, spec] of Object.entries(doc.bundles ?? {})) {
    const count = (spec.commands ?? []).length;
    if (spec.status === STATUS_SETTLED && count === 0) {
      fail(`${TIERS_FILE}: ${bundle} 标「已落」却零命令（空转表）`);
    }
    if (spec.status === STATUS_ROADMAP && count > 0) {
      fail(`${TIERS_FILE}: ${bundle} 标 roadmap 却挂了 ${count} 条命令（假宣称）`);
    }
    if (spec.status !== STATUS_SETTLED && spec.status !== STATUS_ROADMAP) {
      fail(`${TIERS_FILE}: ${bundle} 的 status「${spec.status}」不在口径内`);
    }
    if (spec.kind !== 'capability' && spec.kind !== 'surface') {
      fail(`${TIERS_FILE}: ${bundle} 的 kind「${spec.kind}」不在口径内`);
    }
  }

  // ── I5 + I6：深度单调 + 形态正交。 ──
  const requiresOf = (tier) => doc.tiers?.[tier]?.requires ?? [];
  for (const tier of CANONICAL_TIERS) {
    const spec = doc.tiers?.[tier];
    if (!spec) continue;
    for (const bundle of spec.requires ?? []) {
      const target = doc.bundles?.[bundle];
      if (!target) {
        fail(`${TIERS_FILE}: tier ${tier} 引用了不存在的 bundle ${bundle}`);
        continue;
      }
      if (target.kind !== 'capability') {
        fail(`${TIERS_FILE}: tier ${tier} 把形态捆绑 ${bundle} 列为必备（形态必须可选）`);
      }
      if (target.status !== STATUS_SETTLED) {
        fail(`${TIERS_FILE}: tier ${tier} 引用了未落地的 bundle ${bundle}`);
      }
    }
    const overlap = (spec.requires ?? []).filter((b) => (doc.surfaceBundles ?? []).includes(b));
    if (overlap.length) {
      fail(
        `${TIERS_FILE}: tier ${tier} 的 requires 与 surfaceBundles 相交（§87 正交性被破坏）：${overlap.join(', ')}`,
      );
    }
  }
  const strictSubset = (small, big) =>
    small.every((x) => big.includes(x)) && big.some((x) => !small.includes(x));
  const reqS = requiresOf('S');
  const reqE = requiresOf('E');
  const reqP = requiresOf('P');
  if (!strictSubset(reqS, reqE) || !strictSubset(reqE, reqP)) {
    fail(`${TIERS_FILE}: requires 必须严格单调 S ⊂ E ⊂ P（当前 S=${reqS} E=${reqE} P=${reqP}）`);
  }

  return { failures, surfaceSize: surfaceNames.size };
}

// ── self-test：deep-clone 表数据 / 变异生成物文本，证明六条不变量各按规则打红 ──
function selfTest() {
  const base = loadSources();
  const baseRun = runChecks(base);
  if (baseRun.failures.length > 0) {
    console.error('self-test 前置失败：真实输入本应全绿，但门禁已红——');
    for (const f of baseRun.failures) console.error(`- ${f}`);
    process.exit(1);
  }
  const baseDoc = JSON.parse(base.tiers);
  // 动态取夹具：一个「已落且非空」的 capability bundle、一个已被拥有的命令。
  const settledBundle = Object.entries(baseDoc.bundles).find(
    ([, s]) => s.status === STATUS_SETTLED && (s.commands ?? []).length > 0,
  );
  const surfaceBundle = Object.entries(baseDoc.bundles).find(
    ([, s]) => s.kind === 'surface' && (s.commands ?? []).length > 0,
  );
  const clone = () => JSON.parse(JSON.stringify(baseDoc));

  const bad = [];
  let judged = 0;
  const check = (name, tiersText, expect, surfaceText) => {
    const run = runChecks({ tiers: tiersText, surface: surfaceText ?? base.surface });
    const bitten = run.failures.some((f) => f.includes(expect));
    if (!bitten) {
      bad.push(
        `${name}: 期望含 \`${expect}\` 变红，实际 ${run.failures.length ? run.failures.map((f) => f.slice(0, 40)).join(' | ') : 'GREEN(假绿!)'}`,
      );
      return;
    }
    judged += 1;
  };

  // I1：bundles 少一个键。
  {
    const d = clone();
    delete d.bundles[Object.keys(d.bundles)[0]];
    check('I1 bundles 键不全', JSON.stringify(d), 'bundles 键必须恰为');
  }
  // I1：surfaceBundles 多出非法成员。
  {
    const d = clone();
    d.surfaceBundles.push('core');
    check('I1 surfaceBundles 非 canon', JSON.stringify(d), 'surfaceBundles 必须恰为');
  }
  // I2：生成物分节计数标记被改（解析口径脱节）。
  {
    const m = base.surface.match(/\*\*(\d+) 条\*\*/);
    const orig = m ? m[0] : '**61 条**';
    const n = Number(/\*\*(\d+) 条\*\*/.exec(orig)[1]);
    const tampered = `**${n + 1} 条**`;
    check('I2 计数标记≠解析数', base.tiers, '脱节', base.surface.replace(orig, tampered));
  }
  // I3：命令被从唯一归属 bundle 里删掉（分类不完整）。
  {
    const d = clone();
    const [b] = settledBundle;
    d.bundles[b].commands = d.bundles[b].commands.slice(1);
    check('I3 命令脱离归属', JSON.stringify(d), '分类不完整');
  }
  // I3：命令被两个 bundle 同时拥有（互斥破坏）。
  {
    const d = clone();
    const [sb] = settledBundle;
    const [ub] = surfaceBundle || settledBundle;
    d.bundles[ub].commands = [...d.bundles[ub].commands, d.bundles[sb].commands[0]];
    check('I3 命令重复归属', JSON.stringify(d), '划分必须互斥');
  }
  // I4：已落 bundle 命令清空（空转表）。
  {
    const d = clone();
    const [b] = settledBundle;
    d.bundles[b].commands = [];
    check('I4 已落却零命令', JSON.stringify(d), '零命令');
  }
  // I5：破坏 requires 严格单调。
  {
    const d = clone();
    d.tiers.E.requires = [];
    check('I5 非严格单调', JSON.stringify(d), '严格单调');
  }
  // I6：tier requires 与 surfaceBundles 相交（正交破坏）。
  {
    const d = clone();
    d.tiers.P.requires = [...(d.tiers.P.requires ?? []), CANONICAL_SURFACE_BUNDLES[0]];
    check('I6 正交破坏', JSON.stringify(d), '正交性被破坏');
  }
  // JSON 解析守卫：坏 JSON 必须当场可见，不静默空转。
  check('parse 坏 JSON', '{ this is not json', 'JSON 解析失败');

  if (bad.length > 0) {
    console.error(`self-test 失败（${bad.length} 条）：`);
    for (const line of bad) console.error(`- ${line}`);
    process.exit(1);
  }
  console.log(`check-tier-bundle self-test OK (${judged}/9 条变异各按预期把对应不变量打红)`);
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  const { failures, surfaceSize } = runChecks(loadSources());
  if (failures.length) {
    console.error('Tier-Bundle gate FAILED:');
    for (const f of failures) console.error(`  - ${f}`);
    process.exit(1);
  }
  console.log(
    `Tier-Bundle gate OK: ${CANONICAL_TIERS.length} tiers / ${CANONICAL_BUNDLES.length} bundles / ` +
      `${surfaceSize} commands 全部单属一个 bundle；S⊂E⊂P 严格单调；requires 与 surfaceBundles 零交集。`,
  );
}
