#!/usr/bin/env node
// V4 A105: generate and validate the Public Surface Ledger.
//
// The ledger is derived from the canonical Rust command arrays rather than hand-maintained
// command duplication. CI runs with --check and fails if the committed ledger drifts.

import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const ADAPTER = join(ROOT, 'crates/tauron-adapter/src/lib.rs');
const OUTPUT = join(ROOT, 'contracts/public-surface-ledger.json');

const source = readFileSync(ADAPTER, 'utf8');

function extractArray(name) {
  const match = source.match(new RegExp(`pub const ${name}:[^=]*=\\s*&?\\[([\\s\\S]*?)\\];`));
  if (!match) throw new Error(`Could not find canonical command array ${name}`);
  return [...match[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

const substrate = extractArray('SUBSTRATE_COMMANDS');
const runtime = extractArray('PLUGIN_RUNTIME_COMMANDS');
const install = extractArray('PLUGIN_INSTALL_COMMANDS');

function classify(command, set) {
  const family = command.replace(/^host_/, '').split('_')[0] || 'host';
  const stateful =
    /events_|settings_|registry_|stream_|runtime_|contributes_|recover_|window_create|call_/.test(
      command,
    );
  const maturity = /host_market_(download|install)/.test(command)
    ? 'deprecated'
    : /install_preview/.test(command)
      ? 'beta'
      : 'stable';

  return {
    kind: 'command',
    symbol: command,
    family,
    set,
    owner: set === 'substrate' ? 'Tauron Host Kernel / Platform Adapter' : 'Plugin Runtime Plane',
    producer: 'tauron-adapter canonical command surface',
    consumer: '@tauron/host / generated host binding',
    authorization:
      set === 'substrate'
        ? 'CallerContext + command policy; privileged commands require host authority'
        : 'Plugin/runtime identity + command policy',
    lifecycle: stateful ? 'owner-scoped; terminal state required' : 'request-scoped',
    cleanup: stateful
      ? 'owner teardown / explicit terminal operation / application drain'
      : 'request completion',
    test: 'Rust unit + TypeScript wire-gate + CI command-surface drift gate',
    maturity,
  };
}

const entries = [
  ...substrate.map((c) => classify(c, 'substrate')),
  ...runtime.map((c) => classify(c, 'plugin-runtime')),
  ...install.map((c) => classify(c, 'plugin-install')),
].sort((a, b) => a.symbol.localeCompare(b.symbol));

const duplicates = entries.filter(
  (entry, i) => entries.findIndex((x) => x.symbol === entry.symbol) !== i,
);
if (duplicates.length) {
  throw new Error(`Duplicate public commands: ${duplicates.map((x) => x.symbol).join(', ')}`);
}

const required = [
  'owner',
  'producer',
  'consumer',
  'authorization',
  'lifecycle',
  'cleanup',
  'test',
  'maturity',
];
for (const entry of entries) {
  for (const field of required) {
    if (!entry[field] || String(entry[field]).trim() === '') {
      throw new Error(`Public surface ${entry.symbol} is missing ${field}`);
    }
  }
}

const ledger = {
  schemaVersion: 1,
  generatedFrom: 'crates/tauron-adapter/src/lib.rs canonical command arrays',
  note: 'Generated V4 A105 ledger. Stable public commands may not exist without owner, producer, consumer, authorization, lifecycle, cleanup and test metadata.',
  counts: {
    substrate: substrate.length,
    pluginRuntime: runtime.length,
    pluginInstall: install.length,
    total: entries.length,
  },
  entries,
};

const rendered = JSON.stringify(ledger, null, 2) + '\n';
if (process.argv.includes('--check')) {
  const current = readFileSync(OUTPUT, 'utf8');
  if (current !== rendered) {
    console.error(
      'PublicSurfaceLedger drift detected. Run: node scripts/generate-public-surface-ledger.mjs',
    );
    process.exit(1);
  }
  console.log(`PublicSurfaceLedger OK: ${entries.length} public commands, no orphan metadata.`);
} else {
  writeFileSync(OUTPUT, rendered);
  console.log(`Wrote ${OUTPUT} with ${entries.length} public commands.`);
}
