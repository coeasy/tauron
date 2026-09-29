#!/usr/bin/env node
// V4 A104: conservative no-lock-across-await source gate.
//
// This is intentionally a narrow mechanical gate, not a Rust parser. It catches the dangerous
// shape we want to forbid in Kernel/Adapter code: a named parking_lot/std lock guard acquired in
// an async function and still live when .await is reached. More complex async ownership should
// use actors/serialized owners or be covered by a dedicated lint later.

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const roots = ['crates/tauron-host/src', 'crates/tauron-adapter/src'];
const rustFiles = [];

function walk(path) {
  for (const name of readdirSync(path)) {
    const item = join(path, name);
    const stat = statSync(item);
    if (stat.isDirectory()) walk(item);
    else if (item.endsWith('.rs')) rustFiles.push(item);
  }
}

for (const root of roots) walk(root);

const failures = [];
for (const file of rustFiles) {
  const lines = readFileSync(file, 'utf8').split(/\r?\n/);
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

if (failures.length > 0) {
  console.error('No-Lock-Across-Await gate failed:');
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}
console.log(`No-Lock-Across-Await gate OK across ${rustFiles.length} Rust source files.`);
