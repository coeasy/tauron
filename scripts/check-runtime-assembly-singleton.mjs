#!/usr/bin/env node
// V7-P1-01 / V7 §9 `runtime_assembly_singleton`: one substrate, one plugin runtime.
//
// Before this gate the adapter printed a warning and still returned a *second*
// `PluginRuntimeState` (second `Registry`) whenever a host assembled the same
// substrate twice. Recovery reconciliation kept writing through the first
// registry while the caller held the second one — a log line, not a state
// invariant. The landed shape is: assembly claims an unforgeable token on the
// substrate with `OnceLock::set` and returns `Err(AlreadyAssembled)` when the
// claim fails, so no second runtime object ever exists.
//
// Narrow mechanical rules (each one is a regression that already happened once):
//   1. `with_substrate` / `with_substrate_and_spawner` return
//      `Result<Self, AssemblyError>` — a caller cannot ignore the conflict.
//   2. The token claim happens **before** `Registry::new` (claim-then-build; the
//      reversed order leaks a registry + lease reaper per rejected attempt).
//   3. A conflict returns `AssemblyError::AlreadyAssembled`, and `AssemblyError`
//      implements `std::error::Error` so Tauri `setup` can surface it.
//   4. The old "print and continue" wording must not come back.
//   5. Tauri: the assembly result is `?`-propagated, `manage_states` runs only
//      after it, and both managed states are checked with `try_state` so a
//      duplicate `init()` / `state_init()` fails structurally instead of
//      panicking inside Tauri's state table.
//
// 用法：
//   node scripts/check-runtime-assembly-singleton.mjs               # 校验
//   node scripts/check-runtime-assembly-singleton.mjs --self-test   # 变异自证（每条针有牙）

import { readFileSync } from 'node:fs';

import { collectFiles, stripped, findBlock, finish } from './lib/gate-scan.mjs';

const roots = ['crates/tauron-adapter/src'];
const LIB = 'crates/tauron-adapter/src/lib.rs';
const TAURI = 'crates/tauron-adapter/src/tauri.rs';
const PRINTED_CONFLICTS = ['同一底座不应装配两个插件运行时', '本次插件运行时不会覆盖既有事实'];

const toLines = (text) => text.split(/\r?\n/);

function loadSources() {
  return {
    lib: readFileSync(LIB, 'utf8'),
    tauri: readFileSync(TAURI, 'utf8'),
    fileCount: collectFiles(roots, ['.rs']).length,
  };
}

/** 对给定源码快照跑全部不变量，返回 failures（纯函数，供校验与 self-test 共用）。 */
function runChecks(src) {
  const failures = [];
  const lib = toLines(src.lib);
  const tauri = toLines(src.tauri);

  function signatureOf(lines, headerPattern, label) {
    const projection = stripped(lines);
    const headerIndex = projection.findIndex((line) => headerPattern.test(line));
    if (headerIndex === -1) {
      failures.push(`${roots[0]}: ${label} not found`);
      return null;
    }
    const parts = [];
    for (let i = headerIndex; i < projection.length; i += 1) {
      parts.push(projection[i]);
      if (projection[i].includes('{') || projection[i].includes(';')) break;
    }
    return { index: headerIndex, signature: parts.join(' ') };
  }

  // 1. Both assembly entry points must be fallible.
  for (const [pattern, label] of [
    [/^\s*pub fn with_substrate\s*\(/, 'with_substrate'],
    [/^\s*pub fn with_substrate_and_spawner\s*\(/, 'with_substrate_and_spawner'],
  ]) {
    const found = signatureOf(lib, pattern, label);
    if (found && !/Result<\s*Self\s*,\s*AssemblyError\s*>/.test(found.signature)) {
      failures.push(
        `${LIB}:${found.index + 1}: ${label} must return ` +
          '`Result<Self, AssemblyError>` (a conflict must not hand back a runtime)',
      );
    }
  }

  // 2 + 3. Claim ordering and the typed conflict inside the assembly body.
  const assembly = findBlock(lib, /^\s*pub fn with_substrate_and_spawner\s*\(\s*$/);
  if (!assembly) {
    failures.push(`${LIB}: \`with_substrate_and_spawner\` body not found`);
  } else {
    const body = assembly.body.join('\n');
    const claimedAt = body.indexOf('plugin_runtime_assembly');
    const claimSets = body.match(/plugin_runtime_assembly\s*\.\s*set\s*\(/g) ?? [];
    const registryAt = body.indexOf('Registry::new');
    if (claimSets.length === 0) {
      failures.push(
        `${LIB}:${assembly.headerIndex + 1}: assembly must claim the ` +
          'token with `plugin_runtime_assembly.set(..)` (its return value is the invariant)',
      );
    }
    if (claimedAt !== -1 && registryAt !== -1 && claimedAt > registryAt) {
      failures.push(
        `${LIB}:${assembly.headerIndex + 1}: token claim must precede ` +
          '`Registry::new`, otherwise a rejected attempt leaves an orphan registry + reaper',
      );
    }
    if (!body.includes('AssemblyError::AlreadyAssembled')) {
      failures.push(
        `${LIB}:${assembly.headerIndex + 1}: a repeated assembly must ` +
          'return `AssemblyError::AlreadyAssembled`',
      );
    }
    if (!body.includes('Ok(Self {') && !body.includes('Ok(\n            Self {')) {
      failures.push(
        `${LIB}:${assembly.headerIndex + 1}: assembly must wrap the ` +
          'runtime in `Ok(..)` — a bare `Self {` means the signature regressed',
      );
    }
  }

  const errorImpl = stripped(lib).findIndex((line) =>
    /^impl\s+std::error::Error\s+for\s+AssemblyError\s*\{/.test(line),
  );
  if (errorImpl === -1) {
    failures.push(
      `${LIB}: \`AssemblyError\` must implement \`std::error::Error\` ` +
        'so Tauri setup can return it as a structured error',
    );
  }

  // 4. The print-and-continue wording is the exact regression we are gating.
  for (const [name, lines] of [
    ['lib.rs', lib],
    ['tauri.rs', tauri],
  ]) {
    const text = lines.join('\n');
    for (const phrase of PRINTED_CONFLICTS) {
      if (text.includes(phrase)) {
        failures.push(
          `${roots[0]}/${name}: assembly conflict was downgraded to an eprintln ` +
            `("${phrase}") — it must be an \`AssemblyError\``,
        );
      }
    }
    lines.forEach((line, index) => {
      if (line.includes('eprintln!') && line.includes('同一底座')) {
        failures.push(
          `${roots[0]}/${name}:${index + 1}: duplicate assembly must be a typed ` +
            'error, not an eprintln',
        );
      }
    });
  }

  // 5. Tauri setup path.
  const tauriAssembly = signatureOf(
    tauri,
    /^\s*fn command_state_with_dir_and_config\s*\(/,
    'command_state_with_dir_and_config',
  );
  if (
    tauriAssembly &&
    !/Result<\s*CommandState\s*,\s*crate::AssemblyError\s*>/.test(tauriAssembly.signature)
  ) {
    failures.push(
      `${TAURI}:${tauriAssembly.index + 1}: assembly must return ` +
        '`Result<CommandState, crate::AssemblyError>`',
    );
  }
  for (const needle of ['try_state::<CommandState>', 'try_state::<SubstrateState>']) {
    if (!tauri.some((line) => line.includes(needle))) {
      failures.push(`${TAURI}: setup must guard with \`${needle}\` before manage`);
    }
  }
  tauri.forEach((line, index) => {
    if (line.includes('manage_states(') && line.includes('command_state_with_dir_and_config(')) {
      failures.push(
        `${TAURI}:${index + 1}: manage_states must receive an already ` +
          '`?`-propagated assembly result, never the raw call',
      );
    }
  });
  const setupCalls = tauri
    .map((line, index) => ({ line, index }))
    .filter(
      ({ line }) => line.includes('command_state_with_dir_and_config(') && !line.includes('fn '),
    );
  for (const { index } of setupCalls) {
    const window = tauri.slice(index, index + 6).join('\n');
    if (!window.includes('?')) {
      failures.push(
        `${TAURI}:${index + 1}: assembly call must propagate with ` +
          '`?` so setup fails before any managed state is exposed',
      );
    }
  }

  return failures;
}

// ── self-test：证明每条针有牙（delete 删全部出现、inject 注入被禁形状）────────
const MUTATIONS = [
  {
    name: '装配入口不再返回 Result<Self, AssemblyError>（冲突可被忽略）',
    op: 'delete',
    file: 'lib',
    needle: 'Result<Self, AssemblyError>',
    expect: 'must return',
  },
  {
    name: '装配体不再声明 plugin_runtime_assembly token',
    op: 'delete',
    file: 'lib',
    needle: 'plugin_runtime_assembly',
    expect: 'must claim',
  },
  {
    name: '装配体不再返回 AlreadyAssembled',
    op: 'delete',
    file: 'lib',
    needle: 'AssemblyError::AlreadyAssembled',
    expect: 'a repeated assembly',
  },
  {
    name: 'AssemblyError 不再实现 std::error::Error',
    op: 'delete',
    file: 'lib',
    needle: 'impl std::error::Error for AssemblyError',
    expect: 'std::error::Error',
  },
  {
    name: '旧的“打印并继续”措辞复活',
    op: 'inject',
    file: 'lib',
    line: '// 同一底座不应装配两个插件运行时',
    expect: 'downgraded to an eprintln',
  },
  {
    name: 'Tauri setup 不再 try_state::<CommandState> 守卫',
    op: 'delete',
    file: 'tauri',
    needle: 'try_state::<CommandState>',
    expect: 'setup must guard',
  },
  {
    name: 'Tauri 装配结果不再声明 Result<CommandState, crate::AssemblyError>',
    op: 'delete',
    file: 'tauri',
    needle: 'Result<CommandState',
    expect: 'crate::AssemblyError',
  },
  {
    name: 'manage_states 直接拿未传播的装配调用',
    op: 'inject',
    file: 'tauri',
    line: 'app.manage_states(command_state_with_dir_and_config(cfg));',
    expect: 'manage_states must receive',
  },
];

function applyMutation(base, m) {
  const cur = base[m.file];
  if (m.op === 'delete') {
    if (!cur.includes(m.needle)) return null;
    return { ...base, [m.file]: cur.split(m.needle).join('') };
  }
  return { ...base, [m.file]: `${cur}\n${m.line}\n` };
}

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
  for (const m of MUTATIONS) {
    const mutated = applyMutation(base, m);
    if (!mutated) {
      bad.push(`${m.name}: 针已漂——needle \`${m.needle}\` 在 ${m.file} 中未出现`);
      continue;
    }
    const failures = runChecks(mutated);
    const bitten = failures.some((f) => f.includes(m.expect));
    if (!bitten) {
      bad.push(
        `${m.name}: 期望含 \`${m.expect}\` 变红，实际 ${failures.length ? failures.map((f) => f.slice(0, 40)).join(' | ') : 'GREEN(假绿!)'}`,
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
    `check-runtime-assembly-singleton self-test OK (${judged}/${MUTATIONS.length} 条变异各按预期把对应检查打红)`,
  );
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  const src = loadSources();
  finish('Runtime-Assembly-Singleton', runChecks(src), src.fileCount);
}
