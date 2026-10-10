#!/usr/bin/env node
// V7-P0-01 / V7 §9 `wasm_loaded_generation`: WASM execute must bind the ACTIVE
// module generation.
//
// The shipped-before-state could execute a plugin whose module was never loaded
// (it fabricated a `success: true`), and the instance pool keyed reuse by
// `plugin_id` alone — so an upgraded generation could keep running the previous
// module bytes. The landed shape is a fail-closed gate chain inside
// `WasmEngine::execute`: active module lookup → export check → import whitelist
// → generation-keyed instance reuse/creation → real `provider.invoke`.
//
// Rules (ordering is the point; a reordered chain is a different bug):
//   1. Inside `execute`: `cache.active_module` → `ModuleNotLoaded` →
//      `exports_function` → `find_idle_instance(.., generation)` →
//      `provider.invoke`, in that order.
//   2. `execute` must not report success without invoking the provider.
//   3. Instance reuse is generation-keyed in the pool and the binding carries
//      `generation` (no single-argument reuse call may come back).
//   4. `load_module` must replace the generation and drop stale instances.
//
// 用法：
//   node scripts/check-wasm-loaded-generation.mjs               # 校验
//   node scripts/check-wasm-loaded-generation.mjs --self-test   # 变异自证（证明每条针有牙）

import { readFileSync } from 'node:fs';

import { collectFiles, findBlock, finish } from './lib/gate-scan.mjs';

const EXECUTE = 'crates/tauron-wasm/src/execute.rs';
const LIB = 'crates/tauron-wasm/src/lib.rs';
const roots = ['crates/tauron-wasm/src'];

/** 载入本门禁读到的源文件；self-test 用它做「原始 → 变异」的内存快照。 */
function loadSources() {
  return {
    execute: readFileSync(EXECUTE, 'utf8'),
    lib: readFileSync(LIB, 'utf8'),
    fileCount: collectFiles(roots, ['.rs']).length,
  };
}

/** 对给定源码快照跑全部不变量，返回 failures（纯函数，供校验与 self-test 共用）。 */
function runChecks(src) {
  const failures = [];
  const executeLines = src.execute.split(/\r?\n/);
  const libLines = src.lib.split(/\r?\n/);

  function bodyOf(lines, headerPattern, label, file) {
    const block = findBlock(lines, headerPattern);
    if (!block) {
      failures.push(`${file}: ${label} not found — the gate cannot verify its invariants`);
      return null;
    }
    return block;
  }

  function flagOrder(block, file, label, sequence) {
    const body = block.body.join('\n');
    let previous = -1;
    let previousName = '';
    for (const [name, needle] of sequence) {
      const at = body.indexOf(needle);
      if (at === -1) {
        failures.push(
          `${file}:${block.headerIndex + 1}: ${label} must ${name} (missing \`${needle}\`)`,
        );
        // Keep the previous anchor: reporting a bogus "wrong order" on top of a
        // missing step would send the reader after the wrong cause.
        continue;
      }
      if (at < previous) {
        failures.push(
          `${file}:${block.headerIndex + 1}: ${label} must ${name} **before** ${previousName} ` +
            `(found \`${needle}\` out of order)`,
        );
      }
      previous = at;
      previousName = name;
    }
    return body;
  }

  // 1 + 2. The gate chain inside execute.
  const execute = bodyOf(
    executeLines,
    /^\s*pub fn execute\s*\(\s*$/,
    '`WasmEngine::execute`',
    EXECUTE,
  );
  if (execute) {
    const body = flagOrder(execute, EXECUTE, 'execute', [
      ['resolve the active loaded generation', 'cache.active_module('],
      ['fail closed with `ModuleNotLoaded` when nothing is loaded', 'WasmError::ModuleNotLoaded'],
      ['check the export against the loaded module facts', 'exports_function('],
      ['reuse only an instance bound to that generation', 'find_idle_instance('],
      ['invoke the real provider', 'provider.invoke('],
    ]);
    const successAt = body.indexOf('success: true');
    const invokeAt = body.indexOf('provider.invoke(');
    if (successAt !== -1 && (invokeAt === -1 || successAt < invokeAt)) {
      failures.push(
        `${EXECUTE}:${execute.headerIndex + 1}: a \`success: true\` result ` +
          'is only allowed after the provider was actually invoked',
      );
    }
    if (!/InstanceBinding\s*\{[^}]*generation/.test(body)) {
      failures.push(
        `${EXECUTE}:${execute.headerIndex + 1}: new instances must be ` +
          'bound with `InstanceBinding { generation, .. }` from the active generation',
      );
    }
    if (!/find_idle_instance\s*\(\s*&?[^,)]+,[^)]*generation/.test(body)) {
      failures.push(
        `${EXECUTE}:${execute.headerIndex + 1}: instance reuse must pass ` +
          'the generation (`find_idle_instance(plugin_id, generation)`) — plugin_id alone lets an ' +
          'upgraded generation keep running stale module bytes',
      );
    }
  }

  // 3. The pool API itself must stay generation-keyed.
  for (const [label, pattern] of [
    [
      '`InstancePool::find_idle_instance`',
      /pub fn find_idle_instance\(&self, plugin_id: &str, generation: u64\)/,
    ],
    [
      '`InstancePool::discard_stale_instances`',
      /pub fn discard_stale_instances\(&mut self, plugin_id: &str, generation: u64\)/,
    ],
  ]) {
    if (!libLines.some((line) => pattern.test(line))) {
      failures.push(
        `${LIB}: ${label} must take \`generation: u64\` (ungrouped reuse ` +
          'is the V7-P0-02 defect)',
      );
    }
  }

  // 4. Loading replaces the generation and discards stale instances.
  const load = bodyOf(
    executeLines,
    /^\s*pub fn load_module\s*\(\s*$/,
    '`WasmEngine::load_module`',
    EXECUTE,
  );
  if (load) {
    flagOrder(load, EXECUTE, 'load_module', [
      ['validate the ABI hash before caching', 'validate_abi_hash'],
      ['cache the prepared module', 'cache.insert('],
      ['drop instances bound to the previous generation', 'discard_stale_instances('],
    ]);
  }

  return failures;
}

// ── self-test：对真实源码做内存变异，证明每条针都会红（非空转）────────────────
// execute 链、execute 内的两处正则子判定、success-无-invoke、lib 两个池签名、load_module
// 各自的 check 都至少一条变异覆盖：重构若丢掉任一 check，对应变异不会变红 → self-test 判红。
const MUTATIONS = [
  {
    name: 'execute 不再解析 active 加载代次',
    file: 'execute',
    from: 'cache.active_module(',
    to: 'cache.stale_module(',
    expect: 'execute',
  },
  {
    name: 'execute 不再核对导出符号',
    file: 'execute',
    from: 'exports_function(',
    to: 'exports_bypass(',
    expect: 'execute',
  },
  {
    name: 'execute 不调真 provider（success 失去凭据）',
    file: 'execute',
    from: 'provider.invoke(',
    to: 'provider.deref_only(',
    expect: 'only allowed after',
  },
  {
    name: 'execute 复用实例不再带 generation 实参',
    file: 'execute',
    from: '.find_idle_instance(&plugin_config.plugin_id, generation)',
    to: '.find_idle_instance(&plugin_config.plugin_id)',
    expect: 'must pass',
  },
  {
    name: '新实例绑定丢掉 generation 字段',
    file: 'execute',
    from: 'InstanceBinding { generation',
    to: 'InstanceBinding { stale_tag',
    expect: 'bound with',
  },
  {
    name: 'load_module 不再丢弃旧代次实例',
    file: 'execute',
    from: 'discard_stale_instances(',
    to: 'discard_fresh_instances(',
    expect: 'load_module',
  },
  {
    name: '池 find_idle_instance 去掉 generation 形参',
    file: 'lib',
    from: 'pub fn find_idle_instance(&self, plugin_id: &str, generation: u64)',
    to: 'pub fn find_idle_instance(&self, plugin_id: &str)',
    expect: 'find_idle_instance',
  },
  {
    name: '池 discard_stale_instances 去掉 generation 形参',
    file: 'lib',
    from: 'pub fn discard_stale_instances(&mut self, plugin_id: &str, generation: u64)',
    to: 'pub fn discard_stale_instances(&mut self, plugin_id: &str)',
    expect: 'discard_stale_instances',
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
    const original = base[mutation.file];
    if (typeof original !== 'string') {
      bad.push(`${mutation.name}: 变异目标文件键不存在（脚本病，非门禁）`);
      continue;
    }
    const occurrences = original.split(mutation.from).length - 1;
    if (occurrences !== 1) {
      bad.push(
        `${mutation.name}: 针已漂——from 串在 ${mutation.file} 出现 ${occurrences} 次（期望 1）`,
      );
      continue;
    }
    const mutated = { ...base, [mutation.file]: original.replace(mutation.from, mutation.to) };
    const failures = runChecks(mutated);
    const bitten = failures.some((f) => f.includes(mutation.expect));
    if (!bitten) {
      bad.push(
        `${mutation.name}: 期望含 \`${mutation.expect}\` 变红，实际 ${failures.length ? failures.map((f) => f.slice(0, 40)).join(' | ') : 'GREEN(假绿!)'}`,
      );
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
    `check-wasm-loaded-generation self-test OK (${judged}/${MUTATIONS.length} 条变异各按预期把对应检查打红)`,
  );
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  const src = loadSources();
  finish('Wasm-Loaded-Generation', runChecks(src), src.fileCount);
}
