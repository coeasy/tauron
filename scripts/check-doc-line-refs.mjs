#!/usr/bin/env node
// 文档行号引用核对门禁。
//
// 仓库文档习惯写 `file.rs:1234` 或 `symbol:1234`。代码一改，这些数字就会漂——漂掉的引用
// 比不写引用更糟（读者按它跳到一个无关行）。本脚本把它们当**可执行断言**跑：
//
//   1. `path.rs:NNN[-MMM]`：路径必须能在仓库里按后缀解析到，行号必须在文件行数内，
//      且被指的区间不得整段是空行；
//   2. `symbol:NNN[-MMM]`：symbol 必须真的出现在被指的那几行。文件优先取同一条目里
//      最近一次出现的路径；没有就把全仓当候选（任一候选满足即通过——文档常常只在段落
//      开头点一次文件名）。
//
// 用法：`pnpm docs:check`，或直接
//   node scripts/check-doc-line-refs.mjs                       # 默认文件集
//   node scripts/check-doc-line-refs.mjs docs/a.md docs/b.md   # 指定文件
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('..', import.meta.url));
const EXT = 'rs|ts|tsx|mjs|cjs|json|toml|yaml|yml|md';
const SKIP_DIRS = new Set([
  'node_modules',
  '.git',
  'target',
  'dist',
  'coverage',
  'memory',
  '.qoder',
]);

const DEFAULT_TARGETS = [
  'docs/architecture/v4-industrial-gap-closure-plan.md',
  'docs/architecture/app-layer-wire.md',
  'docs/architecture/overview.md',
  'docs/api/plugin-development-guide.md',
  // 生成物也要查：它的行内引用（专题文档路径、代码符号）同样是宣称。
  'docs/api/command-surface.md',
  'docs/integration/incremental-adoption.md',
];
const args = process.argv.slice(2).filter((a) => !a.startsWith('-'));
const targets = args.length > 0 ? args : DEFAULT_TARGETS;

const repoFiles = [];
(function walk(dir) {
  for (const name of readdirSync(dir)) {
    if (SKIP_DIRS.has(name)) continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) {
      walk(full);
      continue;
    }
    if (!new RegExp(`\\.(${EXT})$`).test(name)) continue;
    repoFiles.push(relative(ROOT, full).replaceAll('\\', '/'));
  }
})(ROOT);

const cache = new Map();
function linesOf(rel) {
  if (!cache.has(rel)) cache.set(rel, readFileSync(join(ROOT, rel), 'utf8').split(/\r?\n/));
  return cache.get(rel);
}
/** 按「路径段子序列」解析文档里写的缩写：`lib.rs`、`adapter/lib.rs`、
 *  `tauron-adapter/src/lib.rs`、完整路径都要能落到同一个文件；歧义时返回全部候选，
 *  由调用方按「任一候选满足即通过」判定。 */
function candidatesFor(raw) {
  const segments = raw.replace(/^\.\//, '').toLowerCase().split('/').filter(Boolean);
  const file = segments.at(-1);
  const rest = segments.slice(0, -1);
  return repoFiles.filter((rel) => {
    const parts = rel.toLowerCase().split('/');
    if (parts.at(-1) !== file) return false;
    // 剩余段按顺序匹配（允许中间插入目录，也允许 `adapter` 命中 `tauron-adapter`）
    let i = 0;
    for (const want of rest) {
      let found = -1;
      for (let j = i; j < parts.length - 1; j += 1) {
        if (parts[j].includes(want)) {
          found = j;
          break;
        }
      }
      if (found === -1) return false;
      i = found + 1;
    }
    return true;
  });
}

// `path.ext:12-34` 或 `symbol:12`（symbol ≥ 3 字符，避免 `x:1` 之类的键值噪声）
const REF = new RegExp(
  `([\\w./-]*\\.(?:${EXT})|(?<![\\w./-])[A-Za-z_][A-Za-z0-9_]{2,}):(\\d+)(?:-(\\d+))?(\\+)?`,
  'g',
);

const problems = [];
let checked = 0;
for (const doc of targets) {
  const text = readFileSync(join(ROOT, doc), 'utf8');
  for (const [i, entry] of text.split(/\r?\n/).entries()) {
    let known = null;
    for (const m of entry.matchAll(REF)) {
      const head = m[1];
      const from = Number(m[2]);
      const to = m[3] ? Number(m[3]) : from;
      const isPath = head.includes('.');
      let candidates;
      if (isPath) {
        candidates = candidatesFor(head);
        if (candidates.length === 0) {
          problems.push(`${doc}:${i + 1} 路径在仓库里找不到：\`${head}\``);
          continue;
        }
        known = candidates;
      } else {
        candidates = known ?? repoFiles;
      }
      checked += 1;
      const symbol = isPath ? null : head.split('.').pop();
      const label = `\`${m[0]}\``;
      const probe = (cands) => {
        const hits = [];
        for (const rel of cands) {
          const lines = linesOf(rel);
          if (from > lines.length) continue;
          const span = lines.slice(from - 1, Math.min(to + 4, lines.length));
          const body = span.join('\n');
          if (symbol) {
            if (body.includes(symbol)) hits.push(rel);
            continue;
          }
          if (body.trim() !== '') hits.push(rel);
        }
        return hits;
      };
      // 符号引用允许落在「同条目最近那个文件」之外的文件里：文档常只在段落开头点一次
      // 文件名，后续符号引用省略它。省略不等于写错，所以先按上下文候选判，判不过再全库兜底。
      let hits = probe(candidates);
      if (hits.length === 0 && !isPath && candidates !== repoFiles) hits = probe(repoFiles);
      if (hits.length === 0) {
        if (symbol) {
          problems.push(`${doc}:${i + 1} ${label} 在仓库任何文件里都指不到该符号（行号已漂）`);
        } else {
          problems.push(
            `${doc}:${i + 1} ${label} 越界或整段空行（候选：${candidates.slice(0, 2).join(', ')}）`,
          );
        }
      }
    }
  }
}

console.log(`行号引用核对：${targets.length} 个文档，${checked} 条引用`);
for (const p of problems) console.log(`✗ ${p}`);
if (problems.length > 0) {
  console.log(
    `\n共 ${problems.length} 条引用漂移。修法：改成**符号锚定**（写 \`fn name\`／测试名，不带行号），或按当前代码更正数字。`,
  );
  process.exit(1);
}
console.log('OK：所有行号引用都落在有效范围且指得到被引用的符号。');
