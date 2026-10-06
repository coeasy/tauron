#!/usr/bin/env node
// V7 轮 22 / 审计轮 2：孤儿公共 API 台账门禁。轮 61 增加「TS 公开声明面」第二口径。
//
// 判据不是「有没有人 import」，而是「文档宣称可用 vs 产品接线面真有消费者」是否一致。
// 四件事，任一不成立即红：
//   1. `orphans` 每条：probe 在接线面（除声明文件）必须零命中——命中说明它已经接线，
//      条目留着就是撒谎（文档还在说「未接线」）。
//   2. `wiredWitnesses` 每条：probe 在接线面（除声明文件）必须至少一处命中——这是
//      非空洞性证明，判定逻辑坏掉时它们会先变红。
//   3. A 口径（`pub fn` / `export function` / `export class`）自动发现总数 ≤ `discoveryBaseline`。
//   4. B 口径（轮 61 新增：TS `export const` / `export type` / `export interface`）总数 ≤
//      `discoveryBaselineDecl`。两条棘轮各自独立，一条的余量不能拿来给另一条遮丑。

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

/** 去掉注释与 Rust 测试模块（声明扫描与两种视图的底座）。 */
function stripRegion(rel) {
  let text = readFileSync(join(ROOT, rel), 'utf8');
  if (rel.endsWith('.rs')) text = stripRustTestModules(text);
  text = text.replace(/\/\*[\s\S]*?\*\//g, '');
  return text
    .split('\n')
    .filter((l) => !/^\s*(\/\/|\/\/\/|\/\/!)/.test(l))
    .join('\n');
}

/**
 * 跨行「再导出/导入块」的开启判据与终结符。
 *
 * 轮 61 修掉两处**宽松方向**的失真（宽松＝放过真孤儿，比假红更需要防）：
 *   - 多行 `import {` 的成员行曾被当成「有人在用」：只 import 不使用也算消费者。
 *   - `useEffect(() => {` 这类以 `use` 起头、行内没有 `;`/`}` 的 TS 代码行，曾被旧模式
 *     的 `use\b` 分支误判成 Rust `use` 语句而开启跨行吞块，把 hook 体里的真消费者整段抹掉。
 * 现在只有 TS 的 `import`/`export` 与 Rust 的 `use`/`pub use` 能开启块，且必须真的没在本行闭合。
 */
function blockTerminator(line) {
  if (/^\s*(?:import|export)(?:\s+type)?\s*[*{]/.test(line) && !/[}]/.test(line)) return '}';
  if (/^\s*(?:pub\s+)?use\b/.test(line) && !/[;]/.test(line)) return ';';
  return null;
}

/**
 * 行视图。
 * @param keepDeclarations 保留声明行（B 口径要把它当作引用位）；false＝旧 A 口径与台账 probe 用的视图。
 */
function lineView(rel, regionText, keepDeclarations) {
  const rust = rel.endsWith('.rs');
  const kept = [];
  let inBlock = null;
  for (const line of regionText.split('\n')) {
    if (inBlock) {
      if (line.includes(inBlock)) inBlock = null;
      continue;
    }
    const terminator = blockTerminator(line);
    if (terminator) {
      inBlock = terminator;
      continue;
    }
    // 整行的 import / 纯再导出：都不构成「真的有人用」。
    if (
      !rust &&
      /^\s*(?:import|export)\b/.test(line) &&
      (/\bfrom\b/.test(line) || /^\s*(?:import|export)(?:\s+type)?\s*[*{]/.test(line))
    )
      continue;
    if (rust && /^\s*(?:pub\s+)?use\b/.test(line)) continue;
    if (keepDeclarations) {
      kept.push(line);
      continue;
    }
    if (rust && /^\s*pub (fn|const|struct|enum|trait|type|mod)\b/.test(line)) continue;
    if (!rust && /^\s*export (async )?(function|class|const|type|interface|default)\b/.test(line))
      continue;
    kept.push(line);
  }
  return kept.join('\n');
}

const regions = new Map(files.map((f) => [f, stripRegion(f)]));
/** 旧 A 口径与台账 probe 用：声明行与再导出都不算消费者。 */
const views = new Map(files.map((f) => [f, lineView(f, regions.get(f), false)]));
/** B 口径用：声明行算引用位（`PluginForm` 只出现在同文件函数签名里，就是在用）。 */
const declViews = new Map(files.map((f) => [f, lineView(f, regions.get(f), true)]));

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

// ── 自动发现棘轮 A 口径：函数/类（轮 2 起） ──────────────────────────────────
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

// ── 自动发现棘轮 B 口径（轮 61）：TS 公开声明面 ──────────────────────────────
// 与 A 口径的两点差别，都是实测逼出来的：
//   1. 声明类型不同：`export const` / `export type` / `export interface` 在 A 口径里完全不可见，
//      于是 SDK 的**词表与形状面**（89 个导出声明里 46 个零跨文件消费者，轮 60 实测）一直躺在棘轮外。
//   2. 消费判据不同：**声明文件正文也算消费者**（候选自身的声明行除外），并把同文件其它声明行
//      当引用位。否则「只被同文件使用」的常量（`TAURON_COMMANDS`）与「只出现在同文件函数签名」
//      的类型别名（`PluginForm`）会被判成孤儿——实测两种判据相差 4 倍以上，用 A 判据跑 B 声明集
//      是纯粹噪声，会驱动人去「收口」在用面。
const TS_DECL_PATTERNS = [
  ['const', /^\s*export const ([A-Za-z0-9_]+)\b/],
  ['type', /^\s*export type ([A-Za-z0-9_]+)\b/],
  ['interface', /^\s*export interface ([A-Za-z0-9_]+)\b/],
];

function tsDeclarations(rel) {
  if (rel.endsWith('.rs')) return [];
  const out = [];
  for (const line of regions.get(rel).split('\n')) {
    for (const [kind, re] of TS_DECL_PATTERNS) {
      const m = re.exec(line);
      if (m) out.push({ name: m[1], kind });
    }
  }
  return out;
}

/** 声明该名的行（任何形式的同名声明都不算它自己的消费者）。 */
function isSelfDeclaration(name, line) {
  const escaped = name.replace(/[$]/g, '\\$&');
  return new RegExp(
    `^\\s*(?:export\\s+)?(?:async\\s+)?(?:const|let|var|function|class|interface|type|enum)\\s+${escaped}\\b`,
  ).test(line);
}

function hasDeclConsumer(name, file) {
  const re = new RegExp(`\\b${name.replace(/[$]/g, '\\$&')}\\b`);
  const own = declViews
    .get(file)
    .split('\n')
    .filter((line) => !isSelfDeclaration(name, line))
    .join('\n');
  if (re.test(own)) return true;
  for (const g of files) {
    if (g === file) continue;
    if (!langOk(g, 'ts')) continue;
    if (re.test(views.get(g))) return true;
  }
  return false;
}

/** 同一声明集按 A 判据（除声明文件）的候选数——只用于 --discover 里对照两种判据的落差。 */
function countUnderARule(name, file) {
  const re = new RegExp(`\\b${name.replace(/[$]/g, '\\$&')}\\b`);
  for (const g of files) {
    if (g === file) continue;
    if (!langOk(g, 'ts')) continue;
    if (re.test(views.get(g))) return false;
  }
  return true;
}

const declCandidates = [];
let declUnderARule = 0;
for (const f of files) {
  for (const { name, kind } of tsDeclarations(f)) {
    if (countUnderARule(name, f)) declUnderARule += 1;
    if (!hasDeclConsumer(name, f)) declCandidates.push({ key: `${f}#${name}`, kind });
  }
}

const declBaseline = ledger.discoveryBaselineDecl;
if (typeof declBaseline !== 'number') {
  failures.push(
    `台账缺少 discoveryBaselineDecl（B 口径棘轮基线）；当前 B 口径候选 = ${declCandidates.length}。缺键必须红，否则新口径会静默不设上限。`,
  );
} else if (declCandidates.length > declBaseline) {
  failures.push(
    `新增孤儿公开声明 ${declCandidates.length - declBaseline} 个（总数 ${declCandidates.length} > 基线 ${declBaseline}，B 口径）。\n` +
      `  要么接上真实消费者（含同文件正文），要么去掉 export 降级为内部实现，要么登记台账并写清 disposition。\n` +
      `  当前候选（前 20 个，用 --discover 看全量分布）：\n    ${declCandidates
        .slice(0, 20)
        .map((c) => c.key)
        .join('\n    ')}`,
  );
}

if (process.argv.includes('--discover')) {
  const byCrate = new Map();
  for (const c of orphanCandidates) {
    const top = c.split('/src/')[0];
    byCrate.set(top, (byCrate.get(top) ?? 0) + 1);
  }
  console.log(
    `A 口径（pub fn / export function / export class）候选 = ${orphanCandidates.length}（基线 ${baseline}）`,
  );
  if (process.argv.includes('--list-a'))
    for (const c of [...orphanCandidates].sort()) console.log(`  ${c}`);
  for (const [k, v] of [...byCrate].sort((a, b) => b[1] - a[1]))
    console.log(`  ${String(v).padStart(4)}  ${k}`);

  const byKind = new Map();
  const declByTop = new Map();
  for (const { key, kind } of declCandidates) {
    byKind.set(kind, (byKind.get(kind) ?? 0) + 1);
    const top = key.split('/src/')[0];
    declByTop.set(top, (declByTop.get(top) ?? 0) + 1);
  }
  console.log(
    `\nB 口径（TS export const / type / interface）候选 = ${declCandidates.length}（基线 ${declBaseline}）；` +
      `同一声明集若按 A 判据（除声明文件）会报 ${declUnderARule} 个——落差即「只被同文件使用」的在用面。`,
  );
  for (const [k, v] of [...byKind].sort((a, b) => b[1] - a[1]))
    console.log(`  ${String(v).padStart(4)}  ${k}`);
  console.log('  -- 按包 --');
  for (const [k, v] of [...declByTop].sort((a, b) => b[1] - a[1]))
    console.log(`  ${String(v).padStart(4)}  ${k}`);

  if (process.argv.includes('--list-decl')) {
    for (const { key, kind } of [...declCandidates].sort((a, b) => a.key.localeCompare(b.key)))
      console.log(`  ${kind.padEnd(10)} ${key}`);
  }
}

if (failures.length > 0) {
  console.error('孤儿公共 API 台账门禁失败：\n- ' + failures.join('\n- '));
  process.exit(1);
}

console.log(
  `孤儿公共 API 台账 OK：${ledger.orphans.length} 条未接线宣称复核通过、` +
    `${ledger.wiredWitnesses.length} 条已接线反例复核通过、` +
    `A 口径 ${orphanCandidates.length} ≤ 基线 ${baseline}、` +
    `B 口径 ${declCandidates.length} ≤ 基线 ${declBaseline}`,
);
