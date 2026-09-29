#!/usr/bin/env node
// V4 A110: release evidence bundle generator.
//
// This does not claim a gate passed merely because a file exists. It records immutable build
// inputs and fails closed on reproducibility facts that can be checked mechanically here.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const outArg = process.argv.find((arg) => arg.startsWith('--out='));
const OUT = resolve(ROOT, outArg?.slice('--out='.length) ?? 'release-evidence');
const checkOnly = process.argv.includes('--check');

const read = (path) => readFileSync(join(ROOT, path), 'utf8');
const pkg = JSON.parse(read('package.json'));
const cargo = read('Cargo.toml');
const toolchain = read('rust-toolchain.toml');
const ci = read('.github/workflows/ci.yml');
const release = read('.github/workflows/release.yml');

const cargoVersion = /^version\s*=\s*"([^"]+)"/m.exec(cargo)?.[1];
if (!cargoVersion || cargoVersion !== pkg.version) {
  throw new Error(`workspace version drift: Cargo=${cargoVersion ?? 'missing'} package=${pkg.version}`);
}

const rustVersion =
  /^channel\s*=\s*"([^"]+)"/m.exec(toolchain)?.[1] ??
  /toolchain:\s*([0-9][^\s]*)/.exec(ci)?.[1] ??
  'unknown';

const packageManager = pkg.packageManager ?? 'unknown';
const nodeVersion = /node-version:\s*([^\s#]+)/.exec(ci)?.[1] ?? 'unknown';

function actionPins(text, workflow) {
  return [...text.matchAll(/^\s*-\s+uses:\s+([^\s#]+)(?:\s+#.*)?$/gm)].map((m) => {
    const spec = m[1];
    const at = spec.lastIndexOf('@');
    const ref = at >= 0 ? spec.slice(at + 1) : '';
    return {
      workflow,
      spec,
      pinnedToCommit: /^[0-9a-f]{40}$/i.test(ref),
    };
  });
}

const actions = [...actionPins(ci, 'ci.yml'), ...actionPins(release, 'release.yml')];
const floatingActions = actions.filter((action) => !action.pinnedToCommit);
if (floatingActions.length) {
  throw new Error(
    `workflow actions are not commit-pinned: ${floatingActions.map((x) => x.spec).join(', ')}`,
  );
}

const targetLabels = [...release.matchAll(/- label:\s*([^\s]+)/g)].map((m) => m[1]);
if (targetLabels.length === 0) throw new Error('release target matrix is empty');

const evidence = {
  schemaVersion: 1,
  product: 'tauron',
  version: pkg.version,
  source: {
    repository: pkg.repository?.url ?? 'https://github.com/coeasy/tauron',
    commit: process.env.GITHUB_SHA ?? process.env.TAURON_SOURCE_SHA ?? 'local-unset',
    ref: process.env.GITHUB_REF ?? 'local-unset',
  },
  reproducibility: {
    rust: rustVersion,
    node: nodeVersion,
    packageManager,
    actions,
    allActionsCommitPinned: true,
  },
  targetMatrix: targetLabels,
  claims: {
    industrialGrade: false,
    note:
      'Evidence collection is implemented; the industrial-grade claim remains false until every required V4 conformance, failure-injection, performance, security and release gate is green.',
  },
  requiredReports: [
    'compatibility-report',
    'conformance-report',
    'security-report',
    'performance-size-report',
    'known-limitations',
  ],
};

if (checkOnly) {
  console.log(
    `release evidence inputs OK: version=${pkg.version}, rust=${rustVersion}, node=${nodeVersion}, targets=${targetLabels.join(',')}, pinnedActions=${actions.length}`,
  );
  process.exit(0);
}

mkdirSync(OUT, { recursive: true });
writeFileSync(join(OUT, 'release-evidence.json'), JSON.stringify(evidence, null, 2) + '\n');
writeFileSync(
  join(OUT, 'README.md'),
  [
    '# Tauron Release Evidence',
    '',
    `- Version: ${evidence.version}`,
    `- Source commit: ${evidence.source.commit}`,
    `- Rust: ${rustVersion}`,
    `- Node: ${nodeVersion}`,
    `- Package manager: ${packageManager}`,
    `- Official build targets: ${targetLabels.join(', ')}`,
    `- Workflow actions commit-pinned: yes (${actions.length})`,
    '',
    '## Claim status',
    '',
    evidence.claims.note,
    '',
    '## Required report slots',
    '',
    ...evidence.requiredReports.map((name) => `- ${name}`),
    '',
  ].join('\n'),
);
console.log(`Release evidence bundle written to ${OUT}`);
