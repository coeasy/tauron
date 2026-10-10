#!/usr/bin/env node
// V4 A104: conservative no-lock-across-await source gate.
//
// This is intentionally a narrow mechanical gate, not a Rust parser. It catches the dangerous
// shape we want to forbid in Kernel/Adapter code: a named parking_lot/std lock guard acquired in
// an async function and still live when .await is reached. More complex async ownership should
// use actors/serialized owners or be covered by a dedicated lint later.
//
// 轮 69（本次落地）：拆成 loadSources → runChecks，并补 --self-test。四变异两两成对，
// 既证明「守卫跨 .await 会红」，又用绿控证明它不是「见到 await 就红」的空转针：
//   R  guard 未 drop 直跨 .await            → 必须红
//   G  guard 先 drop() 再 .await            → 必须绿（证明 drop 释放分支是真的）
//   G  裸 .await 无任何 lock 守卫           → 必须绿（证明不是对所有 await 误报）
//   G  lock+await 落在**非** async fn 里     → 必须绿（证明作用域判定是真的，只在 async fn 内生效）

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const roots = ['crates/tauron-host/src', 'crates/tauron-adapter/src'];

function walk(path, out) {
  for (const name of readdirSync(path)) {
    const item = join(path, name);
    const stat = statSync(item);
    if (stat.isDirectory()) walk(item, out);
    else if (item.endsWith('.rs')) out.push(item);
  }
}

/** 载入接线面：roots 下全部 .rs 文件的路径→正文映射（self-test 拿它做「原始→变异」快照）。 */
function loadSources() {
  const paths = [];
  for (const root of roots) walk(root, paths);
  const files = {};
  for (const p of paths) files[p] = readFileSync(p, 'utf8');
  return { files, target: Object.keys(files).sort()[0] };
}

/** 纯判据：逐文件扫描 async fn 内「守卫仍活着就碰到 .await」的危险形状。 */
function runChecks(src) {
  const failures = [];
  for (const file of Object.keys(src.files)) {
    const lines = src.files[file].split(/\r?\n/);
    let asyncDepth = 0;
    let braces = 0;
    const guards = new Map();

    for (let i = 0; i < lines.length; i += 1) {
      const line = lines[i];
      const opens = (line.match(/\{/g) ?? []).length;
      const closes = (line.match(/\}/g) ?? []).length;

      if (/\basync\s+fn\b/.test(line)) {
        asyncDepth = braces + opens - closes;
        guards.clear();
      }

      if (asyncDepth > 0) {
        const match = line.match(/\blet\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=.*\.lock\(\)/);
        if (match) guards.set(match[1], i + 1);

        for (const guard of [...guards.keys()]) {
          if (new RegExp(`\\bdrop\\s*\\(\\s*${guard}\\s*\\)`).test(line)) guards.delete(guard);
        }

        if (line.includes('.await') && guards.size > 0) {
          for (const [guard, acquiredAt] of guards) {
            failures.push(
              `${file}:${i + 1}: guard '${guard}' acquired at line ${acquiredAt} may cross .await`,
            );
          }
        }
      }

      braces += opens - closes;
      if (asyncDepth > 0 && braces < asyncDepth) {
        asyncDepth = 0;
        guards.clear();
      }
    }
  }
  return failures;
}

// ── self-test：往真实文件末尾追加合成代码，证明判据有牙且不空转 ────────────────
const MUTATIONS = [
  {
    name: 'R：async fn 里 guard 未 drop 直跨 .await',
    want: 'red',
    expect: 'may cross .await',
    snippet:
      '\nasync fn __selftest_lock_holder(x: &parking_lot::Mutex<i32>) {\n' +
      '    let guard = x.lock();\n' +
      '    __selftest_poll().await;\n' +
      '    let _ = guard;\n' +
      '}\n',
  },
  {
    name: 'G：guard 先 drop() 再 .await（释放分支必须成立）',
    want: 'green',
    expect: 'may cross .await',
    snippet:
      '\nasync fn __selftest_lock_released(x: &parking_lot::Mutex<i32>) {\n' +
      '    let guard = x.lock();\n' +
      '    drop(guard);\n' +
      '    __selftest_poll().await;\n' +
      '}\n',
  },
  {
    name: 'G：裸 .await 无任何 lock 守卫（不得误报每一个 await）',
    want: 'green',
    expect: 'may cross .await',
    snippet: '\nasync fn __selftest_no_guard() {\n    __selftest_poll().await;\n}\n',
  },
  {
    name: 'G：lock+await 落在非 async fn 里（作用域判定必须只在 async 内生效）',
    want: 'green',
    expect: 'may cross .await',
    snippet:
      '\nfn __selftest_sync_holder(x: &parking_lot::Mutex<i32>) {\n' +
      '    let guard = x.lock();\n' +
      '    let _ = guard;\n' +
      '}\n',
  },
];

function selfTest() {
  const base = loadSources();
  const baseFailures = runChecks(base);
  if (baseFailures.length > 0) {
    console.error('self-test 前置失败：真实源码本应全绿，但门禁已红——');
    for (const failure of baseFailures) console.error(`- ${failure}`);
    process.exit(1);
  }
  let judged = 0;
  const bad = [];
  for (const mutation of MUTATIONS) {
    const mutated = {
      ...base,
      files: { ...base.files, [base.target]: base.files[base.target] + mutation.snippet },
    };
    const failures = runChecks(mutated);
    const bitten = failures.some((f) => f.includes(mutation.expect));
    if (mutation.want === 'red' && !bitten) {
      bad.push(
        `${mutation.name}: 期望「${mutation.expect}」变红，实际 ${bitten ? '' : 'GREEN(假绿!)'}`,
      );
      continue;
    }
    if (mutation.want === 'green' && bitten) {
      bad.push(`${mutation.name}: 绿控却误报「${mutation.expect}」——判据退化，见 await 就红`);
      continue;
    }
    judged += 1;
  }
  if (bad.length > 0) {
    console.error(`self-test 失败（${bad.length} 条）：`);
    for (const line of bad) console.error(`- ${line}`);
    process.exit(1);
  }
  console.log(
    `check-no-lock-across-await self-test OK (${judged}/${MUTATIONS.length} 条变异：红判据有牙 + 三条绿控不退化)`,
  );
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  const src = loadSources();
  const failures = runChecks(src);
  if (failures.length > 0) {
    console.error('No-Lock-Across-Await gate failed:');
    for (const failure of failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log(
    `No-Lock-Across-Await gate OK across ${Object.keys(src.files).length} Rust source files.`,
  );
}
