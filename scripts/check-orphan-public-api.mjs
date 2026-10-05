#!/usr/bin/env node
// V7 轮 22 / 审计轮 2：孤儿公共 API 台账门禁。
//
// 判据不是「有没有人 import」，而是「文档宣称可用 vs 产品接线面真有消费者」是否一致。
// 三件事，任一不成立即红：
//   1. `orphans` 每条：probe 在接线面（除声明文件）必须零命中——命中说明它已经接线，
//      条目留着就是撒谎（文档还在说「未接线」）。
//   2. `wiredWitnesses` 每条：probe 在接线面（除声明文件）必须至少一处命中——这是
//      非空洞性证明，判定逻辑坏掉时它们会先变红。
//   3. 自动发现总数 ≤ `discoveryBaseline`——「不许新增孤儿」的棘轮。

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = dirname(fileURLToPath(import.meta.url)) + '/..';
const LEDGER = join(ROOT, 'contracts/orphan-public-api.json');
const SKIP = new Set(['node_modules', '.git', 'target', 'dist', 'coverage', 'memory', '.qoder']);
const EXT = /\.(rs|ts|tsx|mjs|cjs|vue|svelte)$/;
const TESTY = /\.test\.|\.spec\.|[\\/](tests|__tests__)[\\/]/;

const ledger = JSON.parse(readFileSync(LEDGER, 'utf8'));

const files = [];
(function walk(dir) {
  for (const name of readdirSync(dir)) {
    if (SKIP.has(name)) continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) {
      walk(full);
      continue;
    }
    if (!EXT.test(name)) continue;
    const rel = relative(ROOT, full).split(sep).join('/');
    // 接线面：产品代码 + 示例；工具脚本与台账本身不算消费者。
    if (!/^(crates\/[^/]+\/src|packages\/[^/]+\/src|examples\/|apps\/)/.test(rel)) continue;
    if (TESTY.test(rel)) continue;
    files.push(rel);
  }
})(join(ROOT, '.'));

/**
 * 逐块摘掉 `#[cfg(test)]` 修饰的模块/条目：必须**配对花括号**找块尾。
 * 早前的实现是「从第一处 #[cfg(test)] 截断整份文件」——那会把截断点之后的全部生产代码
 * 也一起丢掉，于是 adapter 里 `draft_grant_set` / `validate_grants` 这类**真消费者**
 * 查不到，门禁反过来指控已接线的 API 是孤儿。假红比漏报更有害：它会驱动人去「收口」
 * 一条本来就接好的链路。
 */
function stripRustTestModules(text) {
  const lines = text.split('\n');
  const out = [];
  for (let i = 0; i < lines.length; i += 1) {
    if (!/^\s*#\[cfg\(test\)\]/.test(lines[i])) {
      out.push(lines[i]);
      continue;
    }
    // 找到该属性的下一个声明起点，并从其 `{` 开始配对到块尾。
    let j = i + 1;
    while (j < lines.length && /^\s*#\[|^\s*#!\[/.test(lines[j])) j += 1;
    if (j < lines.length && /^\s*(pub\s+)?(mod|fn|struct|enum|impl|trait)\b/.test(lines[j])) {
      let depth = 0;
      let opened = false;
      let k = j;
      for (; k < lines.length; k += 1) {
        for (const ch of lines[k]) {
          if (ch === '{') {
            depth += 1;
            opened = true;
          } else if (ch === '}') depth -= 1;
        }
        if (opened && depth <= 0) break;
      }
      i = k;
      continue;
    }
    // 属性找不到配对的声明体：保守起见原样保留这一行，不做摘除。
    out.push(lines[i]);
  }
  return out.join('\n');
}

/** 去掉注释与 Rust 测试模块（声明扫描的底座：保留 `pub fn` 行本身）。 */
function stripRegion(rel) {
  let text = readFileSync(join(ROOT, rel), 'utf8');
  if (rel.endsWith('.rs')) text = stripRustTestModules(text);
  text = text.replace(/\/\*[\s\S]*?\*\//g, '');
  return text
    .split('\n')
    .filter((l) => !/^\s*(\/\/|\/\/\/|\/\/!)/.test(l))
    .join('\n');
}

/** 再去掉纯再导出行与声明行——它们都不构成「真的有人用」。 */
function consumerView(rel, regionText) {
  const rust = rel.endsWith('.rs');
  const kept = [];
  let inReexport = false;
  for (const line of regionText.split('\n')) {
    // `export {` / `export type {` / `pub use` 多行块：整块跳过，否则 index.ts 会把每个符号都算成消费者。
    // `export type {` 这一支是轮 32 补的：台账里第一条**纯类型**条目（`UpdaterState` /
    // `UpdaterConfig`）撞上了它——再导出块的成员行既不含 `from`，也不是声明行，于是
    // 一行 `  UpdaterConfig,` 被当成了"有人在用"。
    if (/^\s*(export(\s+type)?\s*[*{]|pub\s+use\b|use\b)/.test(line))
      inReexport = !/[;}]/.test(line) ? true : inReexport;
    if (inReexport) {
      if (/[;}]/.test(line)) inReexport = false;
      continue;
    }
    if (rust && /^\s*pub (fn|const|struct|enum|trait|type|mod|use)\b/.test(line)) continue;
    if (!rust && /^\s*export (async )?(function|class|const|type|interface|default)\b/.test(line))
      continue;
    if (!rust && /^\s*(import|export)\b.*\bfrom\b/.test(line)) continue;
    kept.push(line);
  }
  return kept.join('\n');
}

const regions = new Map(files.map((f) => [f, stripRegion(f)]));
const views = new Map(files.map((f) => [f, consumerView(f, regions.get(f))]));

function langOk(rel, lang) {
  if (lang === 'rust') return rel.endsWith('.rs');
  if (lang === 'ts') return /\.(ts|tsx|vue|svelte)$/.test(rel);
  return true;
}

function consumers(entry) {
  const re = new RegExp(entry.probe);
  const declared = entry.declaredIn;
  const hits = [];
  for (const f of files) {
    if (f === declared) continue;
    if (!langOk(f, entry.lang)) continue;
    if (re.test(views.get(f))) hits.push(f);
  }
  return hits;
}

const failures = [];

for (const entry of ledger.orphans) {
  const hits = consumers(entry);
  if (hits.length > 0) {
    failures.push(
      `orphans 条目已接线，必须从台账删除并同步文档：${entry.symbol}（消费者：${hits.slice(0, 3).join(', ')}）`,
    );
  }
}

for (const entry of ledger.wiredWitnesses) {
  const hits = consumers(entry);
  if (hits.length === 0) {
    failures.push(
      `witness 失效（判定逻辑或链路坏了）：${entry.symbol} 在接线面查不到任何消费者；本条宣称它已接线`,
    );
  }
}

// ── 自动发现棘轮 ────────────────────────────────────────────────────────────
function declarations(rel, text) {
  const names = new Set();
  for (const m of text.matchAll(/^\s*pub fn ([a-z0-9_]+)/gm)) names.add(m[1]);
  for (const m of text.matchAll(/^\s*export (?:async )?(?:function|class) ([A-Za-z0-9_]+)/gm))
    names.add(m[1]);
  return [...names].map((name) => ({ name, rel }));
}

const orphanCandidates = [];
for (const f of files) {
  for (const { name } of declarations(f, regions.get(f))) {
    const re = new RegExp(`\\b${name}\\b`);
    let found = false;
    for (const g of files) {
      if (g === f) continue;
      if (!langOk(g, f.endsWith('.rs') ? 'rust' : 'ts')) continue;
      if (re.test(views.get(g))) {
        found = true;
        break;
      }
    }
    if (!found) orphanCandidates.push(`${f}#${name}`);
  }
}

const baseline = ledger.discoveryBaseline;
if (orphanCandidates.length > baseline) {
  failures.push(
    `新增孤儿公共 API ${orphanCandidates.length - baseline} 个（总数 ${orphanCandidates.length} > 基线 ${baseline}）。\n` +
      `  要么接上真实消费者，要么降级为内部实现（去掉 pub/export），要么在文档里明确它「宣称可用但无人使用」并登记进台账。\n` +
      `  当前候选（前 20 个，用 --discover 看全量分布）：\n    ${orphanCandidates.slice(0, 20).join('\n    ')}`,
  );
}

if (process.argv.includes('--discover')) {
  const byCrate = new Map();
  for (const c of orphanCandidates) {
    const top = c.split('/src/')[0];
    byCrate.set(top, (byCrate.get(top) ?? 0) + 1);
  }
  console.log(`孤儿候选总数 = ${orphanCandidates.length}（基线 ${baseline}）`);
  for (const [k, v] of [...byCrate].sort((a, b) => b[1] - a[1]))
    console.log(`  ${String(v).padStart(4)}  ${k}`);
}

if (failures.length > 0) {
  console.error('孤儿公共 API 台账门禁失败：\n- ' + failures.join('\n- '));
  process.exit(1);
}

console.log(
  `孤儿公共 API 台账 OK：${ledger.orphans.length} 条未接线宣称复核通过、` +
    `${ledger.wiredWitnesses.length} 条已接线反例复核通过、` +
    `自动发现 ${orphanCandidates.length} ≤ 基线 ${baseline}`,
);
