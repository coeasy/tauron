#!/usr/bin/env node
// 宿主错误码「人读路标」文档的**生产者**：从 contracts/error/error-codes.json + Rust ErrorCode
// 枚举单向生成 docs/api/error-codes.md。
//
// 为什么是生成而不是手写（独立审计 DX-3 / FULL — 「错误即路标」）：错误码是跨 IPC 的协议名，
// 它的**权威集合在 Rust 枚举里**，而 retryClass 的权威在 `retry_class()`。手写一份错误码文档
// 必然与代码漂——就像 README 手写测试数那样。本生成器把「文档覆盖 = 代码枚举」变成可复算断言：
//   1. registry 的码集合必须与 Rust `pub enum ErrorCode` 变体**全等**（跨源单一真源）；
//   2. 每条 code 的 `summary` 与 `recovery` 必须**非空**——占位要么转正、要么删除，不留空壳
//      （这正是审计批评的「诚实纪律只有半边」的对偶约束：不许用空字段冒充「已文档化」）；
//   3. `retryClass` 必须落在合法词表内。
// 三条任一不成立，`--check` 即红；生成的 md 与文件不一致也红。`--self-test` 注入四类漂移，
// 证明每条判据都真的有牙（审计指出多数针门没有 self-test）。
//
// 用法：node scripts/generate-error-codes-doc.mjs          # 写 docs/api/error-codes.md
//      node scripts/generate-error-codes-doc.mjs --check   # 只判定（CI，纳入 gates:check）
//      node scripts/generate-error-codes-doc.mjs --self-test

import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const REGISTRY = 'contracts/error/error-codes.json';
const ERROR_RS = 'crates/tauron-host/src/error.rs';
const OUT = 'docs/api/error-codes.md';
const read = (rel) => readFileSync(join(ROOT, rel), 'utf8');

// RetryClass 的 kebab-case 线值集合（对齐 error.rs 里 `#[serde(rename_all = "kebab-case")]`）。
const VALID_RETRY = new Set(['never', 'manual', 'auto-idempotent', 'after-reconnect']);

const nonEmpty = (s) => typeof s === 'string' && s.trim().length > 0;

/** Rust `pub enum ErrorCode { ... }` 的变体名（声明顺序）——错误码的唯一权威源。 */
function parseRustCodes() {
  const src = read(ERROR_RS);
  const block = /pub enum ErrorCode\s*\{([\s\S]*?)\n\}/.exec(src);
  if (!block) throw new Error(`找不到 pub enum ErrorCode 体：${ERROR_RS}`);
  const codes = [];
  for (const m of block[1].matchAll(/^\s*(E_[A-Z0-9_]+),/gm)) codes.push(m[1]);
  if (codes.length === 0) throw new Error('ErrorCode 枚举里没解析到任何变体');
  return codes;
}

/** 读 registry：返回 { version, codes }。 */
function loadRegistry() {
  const doc = JSON.parse(read(REGISTRY));
  if (!Array.isArray(doc.codes)) throw new Error(`${REGISTRY} 缺 codes 数组`);
  return doc;
}

/**
 * 三条不变量的判定：跨源集合全等 + 无空占位 + retryClass 合法。返回失败原因数组（空＝通过）。
 * 抽成纯函数，`--self-test` 与 `--check` 共用同一判定，杜绝「自检跑的和 CI 跑的不是同一套」。
 */
function validate(rustCodes, entries) {
  const fails = [];
  const rustSet = new Set(rustCodes);
  const seen = new Set();
  for (const e of entries) {
    if (seen.has(e.code)) fails.push(`registry 重复码：${e.code}`);
    seen.add(e.code);
    if (!rustSet.has(e.code))
      fails.push(`registry 有、Rust 枚举没有的码：${e.code}（枚举才是权威源）`);
    if (!VALID_RETRY.has(e.retryClass))
      fails.push(`${e.code} 的 retryClass 非法：${JSON.stringify(e.retryClass)}`);
    if (!nonEmpty(e.summary)) fails.push(`${e.code} 缺 summary——占位必须转正或删除`);
    if (!nonEmpty(e.recovery)) fails.push(`${e.code} 缺 recovery——占位必须转正或删除`);
  }
  for (const c of rustCodes) if (!seen.has(c)) fails.push(`Rust 枚举码未登记进 registry：${c}`);
  return fails;
}

const cell = (s) => s.replace(/\|/g, '\\|').replace(/\s+/g, ' ').trim();

/** 按 Rust 枚举声明顺序渲染（权威顺序），逐码从 registry 取 summary/recovery/retryClass。 */
function renderDoc(rustCodes, entries) {
  const byCode = new Map(entries.map((e) => [e.code, e]));
  const rows = rustCodes.map((code) => {
    const e = byCode.get(code);
    return `| \`${code}\` | \`${e.retryClass}\` | ${cell(e.summary)} | ${cell(e.recovery)} |`;
  });
  const retryTally = {};
  for (const e of entries) retryTally[e.retryClass] = (retryTally[e.retryClass] || 0) + 1;
  const tallyLine = ['never', 'manual', 'auto-idempotent', 'after-reconnect']
    .filter((k) => retryTally[k])
    .map((k) => `${k} **${retryTally[k]}**`)
    .join('、');

  return `# 宿主错误码参考（自动生成，请勿手改）

> **这个文件是生成物**：\`node scripts/generate-error-codes-doc.mjs\` 从
> \`contracts/error/error-codes.json\` + \`${ERROR_RS}\` 的 \`pub enum ErrorCode\` 单向生成，
> CI 用 \`pnpm gates:check\` 里的 \`--check\` 复算。它把三件事变成可执行断言，而不是某人
> 记得去补的表：
>
> 1. **文档码集合 = Rust 枚举码集合**（跨源单一真源，枚举才是权威）；
> 2. **每条码都有非空 \`summary\` 与 \`recovery\`**——空占位即红，杜绝「用空字段冒充已文档化」；
> 3. **\`retryClass\` 落在合法词表**（never / manual / auto-idempotent / after-reconnect）。
>
> \`retryClass\` 与 \`code\` 是**协议面**，由 wire-gate 三方对读（Rust \`retry_class()\` /
> \`@tauron/host\` 词表 / 本 registry）；\`summary\`/\`recovery\` 是给人看的**路标**，
> 不参与协议判定。自动重试永远不 implied：\`retryable\` 仅对 \`auto-idempotent\` 为真，
> 而当前集合里没有任何码走这一档（见下）。

## 计数口径

共 **${rustCodes.length}** 条错误码；按 \`retryClass\` 分布：${tallyLine}。

| 错误码 | retryClass | 含义（一句话） | 调用方恢复动作（下一步该做什么） |
|---|---|---|---|
${rows.join('\n')}

## 读表须知

1. **\`never\` 不是「别管它」**：多数码是确定性故障（清单写错、序列错了、容量闸、宣称与事实
   落差），自动重试只会再失败一次——恢复动作那一列就是给调用方分流用的路标。
2. **\`manual\` / \`after-reconnect\` 要人推一把**：超时（\`E_CALL_TIMEOUT\`）与反压
   （\`E_STREAM_BACKPRESSURE\`）都可能已经产生副作用，重发前必须自己判定幂等性；
   租约失效（\`E_LEASE_EXPIRED\`）要先 \`host_runtime_spawn\` 建新租约。
3. **\`E_PLUGIN_TYPE_NO_RUNTIME\` 是诚实失败码**，不是桩成功：本仓的
   \`RuntimeDriver\` 契约（\`crates/tauron-host/src/runtime_driver.rs\`）正是要把这条码逐步
   变成「对应有执行器」的真运行路径（见统一开发计划 T-6/T-10/T-11/T-12）。
4. 语义边界仍以代码为准：错误模型的产生点写在 \`${ERROR_RS}\` 的文档注释里，
   命令的档位与判定见 \`docs/api/command-surface.md\`。
`;
}

// ── 变异夹具自检：四条判据都必须能被打红 ─────────────────────────────────────
function selfTest() {
  const rust = parseRustCodes();
  const base = loadRegistry();
  if (base.codes.length < 5) throw new Error('registry 太小，self-test 无意义');
  // 真实数据本身必须干净，否则下面「注入漂移被抓」的对比失去基准。
  if (validate(rust, base.codes).length !== 0) {
    console.error('self-test FAILED：真实 registry 就不满足判据（先修数据再谈门禁有牙）');
    process.exit(1);
  }
  const mk = (mut) => {
    const e = structuredClone(base.codes);
    mut(e);
    return validate(rust, e).length > 0;
  };
  const cases = [
    ['空 recovery', (e) => (e[0].recovery = '')],
    ['空 summary', (e) => (e[0].summary = '   ')],
    ['删掉一个码（枚举有、registry 缺）', (e) => e.splice(0, 1)],
    [
      '多一个假码（registry 有、枚举无）',
      (e) => e.push({ code: 'E_NOT_A_CODE', retryClass: 'never', summary: 'x', recovery: 'y' }),
    ],
    ['非法 retryClass', (e) => (e[0].retryClass = 'sometimes')],
    ['重复码', (e) => e.push({ ...e[0] })],
  ];
  const missed = cases.filter(([, mut]) => !mk(mut)).map(([name]) => name);
  if (missed.length === 0) {
    console.log(`error-codes self-test OK：${cases.length} 类注入漂移都被 --check 判据抓到`);
    return;
  }
  console.error(`error-codes self-test FAILED：以下判据无牙 → ${missed.join('、')}`);
  process.exit(1);
}

const rustCodes = parseRustCodes();
const registry = loadRegistry();
const doc = renderDoc(rustCodes, registry.codes);
const target = resolve(ROOT, OUT);

if (process.argv.includes('--self-test')) {
  selfTest();
} else if (process.argv.includes('--check')) {
  const fails = validate(rustCodes, registry.codes);
  if (fails.length > 0) {
    console.error(`✗ ${REGISTRY} 不满足判据：\n- ` + fails.join('\n- '));
    process.exit(1);
  }
  let current = '';
  try {
    current = readFileSync(target, 'utf8');
  } catch {
    current = '';
  }
  if (current !== doc) {
    console.error(
      `✗ ${OUT} 与 registry/枚举不同步（码集合、summary、recovery 或 retryClass 变了）。` +
        ' 跑 `node scripts/generate-error-codes-doc.mjs` 重新生成后一并提交。',
    );
    process.exit(1);
  }
  console.log(
    `Error-codes doc OK: ${rustCodes.length} 码与 Rust 枚举全等、summary/recovery 全非空、retryClass 全合法、文档与生成物一致`,
  );
} else {
  const fails = validate(rustCodes, registry.codes);
  if (fails.length > 0) {
    console.error(`✗ ${REGISTRY} 不满足判据，拒绝生成：\n- ` + fails.join('\n- '));
    process.exit(1);
  }
  writeFileSync(target, doc);
  console.log(`wrote ${OUT}: ${rustCodes.length} 码（与 Rust 枚举全等）`);
}
