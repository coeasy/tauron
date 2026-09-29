#!/usr/bin/env node
// V4 A110: release evidence bundle generator.
//
// Static --check validates reproducibility inputs without pretending a release happened.
// A real release passes --ci-proof from verify-source-ci.mjs and runs only after the complete
// release build matrix succeeds.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const outArg = process.argv.find((arg) => arg.startsWith('--out='));
const ciProofArg = process.argv.find((arg) => arg.startsWith('--ci-proof='));
const OUT = resolve(ROOT, outArg?.slice('--out='.length) ?? 'release-evidence');
const checkOnly = process.argv.includes('--check');

const read = (path) => readFileSync(join(ROOT, path), 'utf8');
const pkg = JSON.parse(read('package.json'));
const cargo = read('Cargo.toml');
const toolchain = read('rust-toolchain.toml');
const ci = read('.github/workflows/ci.yml');
const release = read('.github/workflows/release.yml');
const targetMatrix = JSON.parse(read('contracts/target-matrix.json'));

const cargoVersion = /^version\s*=\s*"([^"]+)"/m.exec(cargo)?.[1];
if (!cargoVersion || cargoVersion !== pkg.version) {
  throw new Error(
    `workspace version drift: Cargo=${cargoVersion ?? 'missing'} package=${pkg.version}`,
  );
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
    return { workflow, spec, pinnedToCommit: /^[0-9a-f]{40}$/i.test(ref) };
  });
}

const actions = [...actionPins(ci, 'ci.yml'), ...actionPins(release, 'release.yml')];
const floatingActions = actions.filter((action) => !action.pinnedToCommit);
if (floatingActions.length) {
  throw new Error(
    `workflow actions are not commit-pinned: ${floatingActions.map((x) => x.spec).join(', ')}`,
  );
}

if (targetMatrix.schemaVersion !== 1 || !Array.isArray(targetMatrix.rows)) {
  throw new Error('contracts/target-matrix.json is invalid');
}

const sourceSha = process.env.GITHUB_SHA ?? process.env.TAURON_SOURCE_SHA ?? 'local-unset';
let sourceCi = null;
if (ciProofArg) {
  const proofPath = resolve(ROOT, ciProofArg.slice('--ci-proof='.length));
  sourceCi = JSON.parse(readFileSync(proofPath, 'utf8'));
  if (sourceCi.sourceSha !== sourceSha) {
    throw new Error(
      `source CI proof SHA mismatch: proof=${sourceCi.sourceSha} release=${sourceSha}`,
    );
  }
  if (sourceCi.conclusion !== 'success') {
    throw new Error('source CI proof is not successful');
  }
}

const releaseBuildMatrixPassed = process.env.TAURON_RELEASE_BUILD_MATRIX_PASSED === 'true';
if (!checkOnly && (!sourceCi || !releaseBuildMatrixPassed)) {
  throw new Error(
    'real release evidence requires exact-SHA source CI proof and successful build matrix',
  );
}

const limitations = [
  'Remote Host transport is not implemented; A108 remote chaos remains open.',
  'The Local Host reference transport is implemented on Linux UDS/SO_PEERCRED only; Windows Named Pipe/SID and macOS peer-credential reference transports remain open.',
  'The built-in CommandSpawner honestly reports process sandbox enforcement as unsupported; Production rejects process-runtime startup unless a hard ProcessSandboxProvider is injected.',
  'Performance/size release baselines are not yet a hard gate, so industrialGrade remains false.',
];

const evidence = {
  schemaVersion: 2,
  product: 'tauron',
  version: pkg.version,
  source: {
    repository: pkg.repository?.url ?? 'https://github.com/coeasy/tauron',
    commit: sourceSha,
    ref: process.env.GITHUB_REF ?? 'local-unset',
  },
  reproducibility: {
    rust: rustVersion,
    node: nodeVersion,
    packageManager,
    actions,
    allActionsCommitPinned: true,
  },
  sourceCi,
  targetMatrix: targetMatrix.rows,
  releaseBuildMatrixPassed,
  claims: {
    industrialGrade: false,
    reason:
      'Performance/size gate and remaining Remote/non-Linux Local Host security conformance are still open.',
  },
  reportFiles: [
    'compatibility-report.json',
    'conformance-report.json',
    'security-report.json',
    'performance-size-report.json',
    'known-limitations.md',
  ],
};

if (checkOnly) {
  console.log(
    `release evidence static inputs OK: version=${pkg.version}, rust=${rustVersion}, node=${nodeVersion}, targets=${targetMatrix.rows.map((x) => x.label).join(',')}, pinnedActions=${actions.length}`,
  );
  process.exit(0);
}

mkdirSync(OUT, { recursive: true });

const compatibility = {
  schemaVersion: 1,
  sourceSha,
  sourceCiRunId: sourceCi.workflowRunId,
  releaseBuildMatrixPassed,
  targets: targetMatrix.rows.map((row) => ({
    label: row.label,
    os: row.os,
    arch: row.arch,
    coverage: row.coverage,
    profiles: row.profiles,
    runtimes: row.runtimes,
  })),
  status: 'passed',
};

const conformance = {
  schemaVersion: 1,
  sourceSha,
  sourceCiRunId: sourceCi.workflowRunId,
  requiredJobs: sourceCi.requiredJobs,
  requiredSteps: sourceCi.requiredSteps,
  status: 'passed',
};

const security = {
  schemaVersion: 1,
  sourceSha,
  sourceCiRunId: sourceCi.workflowRunId,
  evidence: [
    'cargo-deny hard gate',
    'FFI Ownership / ASan',
    'Local Host UDS peer-auth E2E',
    'Local Host broker chaos / replay rejection',
    'Production process-sandbox fail-closed tests inside workspace Rust tests',
  ],
  status: 'passed-with-known-limitations',
  limitations: limitations.slice(0, 3),
};

const performance = {
  schemaVersion: 1,
  sourceSha,
  status: 'open-gate',
  verified: false,
  reason:
    'No release-blocking RSS/CPU/startup/shutdown/artifact-size regression baseline is wired yet.',
};

writeFileSync(join(OUT, 'tauron-release-evidence.json'), JSON.stringify(evidence, null, 2) + '\n');
writeFileSync(
  join(OUT, 'compatibility-report.json'),
  JSON.stringify(compatibility, null, 2) + '\n',
);
writeFileSync(join(OUT, 'conformance-report.json'), JSON.stringify(conformance, null, 2) + '\n');
writeFileSync(join(OUT, 'security-report.json'), JSON.stringify(security, null, 2) + '\n');
writeFileSync(
  join(OUT, 'performance-size-report.json'),
  JSON.stringify(performance, null, 2) + '\n',
);
writeFileSync(
  join(OUT, 'known-limitations.md'),
  ['# Known Limitations', '', ...limitations.map((x) => `- ${x}`), ''].join('\n'),
);
writeFileSync(
  join(OUT, 'tauron-release-evidence.md'),
  [
    '# Tauron Release Evidence',
    '',
    `- Version: ${evidence.version}`,
    `- Source commit: ${evidence.source.commit}`,
    `- Source CI run: ${sourceCi.workflowRunId}`,
    `- Rust: ${rustVersion}`,
    `- Node: ${nodeVersion}`,
    `- Package manager: ${packageManager}`,
    `- Native conformance targets: ${targetMatrix.rows.map((x) => x.label).join(', ')}`,
    `- Release build matrix passed: ${releaseBuildMatrixPassed ? 'yes' : 'no'}`,
    `- Workflow actions commit-pinned: yes (${actions.length})`,
    '',
    '## Claim status',
    '',
    `Industrial-grade claim: **NO** — ${evidence.claims.reason}`,
    '',
    '## Reports',
    '',
    ...evidence.reportFiles.map((name) => `- ${name}`),
    '',
  ].join('\n'),
);
console.log(`Release evidence bundle written to ${OUT}`);
