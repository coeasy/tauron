#!/usr/bin/env node
// V4 A99: keep the machine-readable OS×arch×profile×runtime matrix aligned with CI and Release.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const matrix = JSON.parse(readFileSync(join(ROOT, 'contracts/target-matrix.json'), 'utf8'));
const ci = readFileSync(join(ROOT, '.github/workflows/ci.yml'), 'utf8');
const release = readFileSync(join(ROOT, '.github/workflows/release.yml'), 'utf8');

if (matrix.schemaVersion !== 1 || !Array.isArray(matrix.rows) || matrix.rows.length === 0) {
  throw new Error('target matrix must be schemaVersion=1 with at least one row');
}

const labels = new Set();
for (const row of matrix.rows) {
  for (const field of ['label', 'runner', 'os', 'arch', 'rustTarget', 'coverage']) {
    if (!row[field] || String(row[field]).trim() === '') {
      throw new Error(`target matrix row is missing ${field}: ${JSON.stringify(row)}`);
    }
  }
  if (labels.has(row.label)) throw new Error(`duplicate target label: ${row.label}`);
  labels.add(row.label);
  if (row.coverage !== 'native-execution') {
    throw new Error(`${row.label} is official but is not native-execution coverage`);
  }
  for (const profile of ['substrate', 'ffi', 'extension', 'tauri-desktop']) {
    if (!row.profiles?.includes(profile)) {
      throw new Error(`${row.label} does not cover required profile ${profile}`);
    }
  }
  for (const runtime of ['process', 'wasm-broker']) {
    if (!row.runtimes?.includes(runtime)) {
      throw new Error(`${row.label} does not cover required runtime ${runtime}`);
    }
  }
}

function parseRows(text, runnerKey) {
  const rows = new Map();
  const re = new RegExp(
    `- label:\\s*([^\\s]+)[\\s\\S]{0,180}?\\n\\s*${runnerKey}:\\s*([^\\s#]+)`,
    'g',
  );
  for (const match of text.matchAll(re)) rows.set(match[1], match[2]);
  return rows;
}

const ciRows = parseRows(ci, 'runner');
const releaseRows = parseRows(release, 'platform');
const expectedRelease = new Set(matrix.rows.filter((r) => r.releaseArtifact).map((r) => r.label));

for (const row of matrix.rows) {
  if (ciRows.get(row.label) !== row.runner) {
    throw new Error(
      `CI target row drift for ${row.label}: expected runner=${row.runner}, actual=${ciRows.get(row.label) ?? 'missing'}`,
    );
  }
  if (row.releaseArtifact && releaseRows.get(row.label) !== row.runner) {
    throw new Error(
      `Release target row drift for ${row.label}: expected platform=${row.runner}, actual=${releaseRows.get(row.label) ?? 'missing'}`,
    );
  }
}

for (const label of ciRows.keys()) {
  if (!labels.has(label)) throw new Error(`CI contains undeclared target matrix label ${label}`);
}
for (const label of releaseRows.keys()) {
  if (!expectedRelease.has(label)) {
    throw new Error(`Release contains undeclared artifact target label ${label}`);
  }
}

console.log(
  `Target Matrix OK: ${matrix.rows.map((r) => `${r.label}@${r.runner}`).join(', ')}`,
);
