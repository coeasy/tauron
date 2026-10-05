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

import { collectFiles, linesOf, stripped, findBlock, finish } from './lib/gate-scan.mjs';

const roots = ['crates/tauron-adapter/src'];
const files = collectFiles(roots, ['.rs']);
const failures = [];
const lib = linesOf('crates/tauron-adapter/src/lib.rs');
const tauri = linesOf('crates/tauron-adapter/src/tauri.rs');

function signatureOf(lines, headerPattern, label) {
  const projection = stripped(lines);
  const headerIndex = projection.findIndex((line) => headerPattern.test(line));
  if (headerIndex === -1) {
    failures.push(`crates/tauron-adapter/src: ${label} not found`);
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
      `crates/tauron-adapter/src/lib.rs:${found.index + 1}: ${label} must return ` +
        '`Result<Self, AssemblyError>` (a conflict must not hand back a runtime)',
    );
  }
}

// 2 + 3. Claim ordering and the typed conflict inside the assembly body.
const assembly = findBlock(lib, /^\s*pub fn with_substrate_and_spawner\s*\(\s*$/);
if (!assembly) {
  failures.push('crates/tauron-adapter/src/lib.rs: `with_substrate_and_spawner` body not found');
} else {
  const body = assembly.body.join('\n');
  const claimedAt = body.indexOf('plugin_runtime_assembly');
  const claimSets = body.match(/plugin_runtime_assembly\s*\.\s*set\s*\(/g) ?? [];
  const registryAt = body.indexOf('Registry::new');
  if (claimSets.length === 0) {
    failures.push(
      `crates/tauron-adapter/src/lib.rs:${assembly.headerIndex + 1}: assembly must claim the ` +
        'token with `plugin_runtime_assembly.set(..)` (its return value is the invariant)',
    );
  }
  if (claimedAt !== -1 && registryAt !== -1 && claimedAt > registryAt) {
    failures.push(
      `crates/tauron-adapter/src/lib.rs:${assembly.headerIndex + 1}: token claim must precede ` +
        '`Registry::new`, otherwise a rejected attempt leaves an orphan registry + reaper',
    );
  }
  if (!body.includes('AssemblyError::AlreadyAssembled')) {
    failures.push(
      `crates/tauron-adapter/src/lib.rs:${assembly.headerIndex + 1}: a repeated assembly must ` +
        'return `AssemblyError::AlreadyAssembled`',
    );
  }
  if (!body.includes('Ok(Self {') && !body.includes('Ok(\n            Self {')) {
    failures.push(
      `crates/tauron-adapter/src/lib.rs:${assembly.headerIndex + 1}: assembly must wrap the ` +
        'runtime in `Ok(..)` — a bare `Self {` means the signature regressed',
    );
  }
}

const errorImpl = stripped(lib).findIndex((line) =>
  /^impl\s+std::error::Error\s+for\s+AssemblyError\s*\{/.test(line),
);
if (errorImpl === -1) {
  failures.push(
    'crates/tauron-adapter/src/lib.rs: `AssemblyError` must implement `std::error::Error` ' +
      'so Tauri setup can return it as a structured error',
  );
}

// 4. The print-and-continue wording is the exact regression we are gating.
const printedConflicts = ['同一底座不应装配两个插件运行时', '本次插件运行时不会覆盖既有事实'];
for (const [name, lines] of [
  ['lib.rs', linesOf('crates/tauron-adapter/src/lib.rs')],
  ['tauri.rs', linesOf('crates/tauron-adapter/src/tauri.rs')],
]) {
  const text = lines.join('\n');
  for (const phrase of printedConflicts) {
    if (text.includes(phrase)) {
      failures.push(
        `crates/tauron-adapter/src/${name}: assembly conflict was downgraded to an eprintln ` +
          `("${phrase}") — it must be an \`AssemblyError\``,
      );
    }
  }
  lines.forEach((line, index) => {
    if (line.includes('eprintln!') && line.includes('同一底座')) {
      failures.push(
        `crates/tauron-adapter/src/${name}:${index + 1}: duplicate assembly must be a typed ` +
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
    `crates/tauron-adapter/src/tauri.rs:${tauriAssembly.index + 1}: assembly must return ` +
      '`Result<CommandState, crate::AssemblyError>`',
  );
}
for (const needle of ['try_state::<CommandState>', 'try_state::<SubstrateState>']) {
  if (!tauri.some((line) => line.includes(needle))) {
    failures.push(
      `crates/tauron-adapter/src/tauri.rs: setup must guard with \`${needle}\` before manage`,
    );
  }
}
tauri.forEach((line, index) => {
  if (line.includes('manage_states(') && line.includes('command_state_with_dir_and_config(')) {
    failures.push(
      `crates/tauron-adapter/src/tauri.rs:${index + 1}: manage_states must receive an already ` +
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
      `crates/tauron-adapter/src/tauri.rs:${index + 1}: assembly call must propagate with ` +
        '`?` so setup fails before any managed state is exposed',
    );
  }
}

finish('Runtime-Assembly-Singleton', failures, files.length);
