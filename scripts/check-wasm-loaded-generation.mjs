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

import { collectFiles, linesOf, findBlock, finish } from './lib/gate-scan.mjs';

const roots = ['crates/tauron-wasm/src'];
const files = collectFiles(roots, ['.rs']);
const failures = [];
const executeLines = linesOf('crates/tauron-wasm/src/execute.rs');
const libLines = linesOf('crates/tauron-wasm/src/lib.rs');

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
  'crates/tauron-wasm/src/execute.rs',
);
if (execute) {
  const body = flagOrder(execute, 'crates/tauron-wasm/src/execute.rs', 'execute', [
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
      `crates/tauron-wasm/src/execute.rs:${execute.headerIndex + 1}: a \`success: true\` result ` +
        'is only allowed after the provider was actually invoked',
    );
  }
  if (!/InstanceBinding\s*\{[^}]*generation/.test(body)) {
    failures.push(
      `crates/tauron-wasm/src/execute.rs:${execute.headerIndex + 1}: new instances must be ` +
        'bound with `InstanceBinding { generation, .. }` from the active generation',
    );
  }
  if (!/find_idle_instance\s*\(\s*&?[^,)]+,[^)]*generation/.test(body)) {
    failures.push(
      `crates/tauron-wasm/src/execute.rs:${execute.headerIndex + 1}: instance reuse must pass ` +
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
      `crates/tauron-wasm/src/lib.rs: ${label} must take \`generation: u64\` (ungrouped reuse ` +
        'is the V7-P0-02 defect)',
    );
  }
}

// 4. Loading replaces the generation and discards stale instances.
const load = bodyOf(
  executeLines,
  /^\s*pub fn load_module\s*\(\s*$/,
  '`WasmEngine::load_module`',
  'crates/tauron-wasm/src/execute.rs',
);
if (load) {
  flagOrder(load, 'crates/tauron-wasm/src/execute.rs', 'load_module', [
    ['validate the ABI hash before caching', 'validate_abi_hash'],
    ['cache the prepared module', 'cache.insert('],
    ['drop instances bound to the previous generation', 'discard_stale_instances('],
  ]);
}

finish('Wasm-Loaded-Generation', failures, files.length);
