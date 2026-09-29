#!/usr/bin/env node
// V4 A110: prove that the exact source SHA has a successful required CI run before Release.

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

const args = new Map(
  process.argv
    .slice(2)
    .filter((x) => x.startsWith('--') && x.includes('='))
    .map((x) => {
      const i = x.indexOf('=');
      return [x.slice(2, i), x.slice(i + 1)];
    }),
);

const repo = process.env.GITHUB_REPOSITORY;
const sha = process.env.GITHUB_SHA;
const token = process.env.GITHUB_TOKEN;
const out = resolve(args.get('out') ?? 'release-evidence-input/source-ci-proof.json');

if (!repo || !sha || !token) {
  throw new Error('GITHUB_REPOSITORY, GITHUB_SHA and GITHUB_TOKEN are required');
}

const headers = {
  Accept: 'application/vnd.github+json',
  Authorization: `Bearer ${token}`,
  'X-GitHub-Api-Version': '2022-11-28',
  'User-Agent': 'tauron-release-proof',
};

async function get(path) {
  const response = await fetch(`https://api.github.com/repos/${repo}${path}`, { headers });
  if (!response.ok) {
    throw new Error(`GitHub API ${path} failed: ${response.status} ${await response.text()}`);
  }
  return response.json();
}

const runs = await get(
  `/actions/workflows/ci.yml/runs?head_sha=${encodeURIComponent(sha)}&per_page=100`,
);
const run = runs.workflow_runs
  .filter((x) => x.head_sha === sha && x.status === 'completed' && x.conclusion === 'success')
  .sort((a, b) => b.id - a.id)[0];

if (!run) throw new Error(`no successful CI workflow run exists for exact source SHA ${sha}`);

const jobsResponse = await get(`/actions/runs/${run.id}/jobs?per_page=100`);
const jobs = jobsResponse.jobs ?? [];
const byName = new Map(jobs.map((job) => [job.name, job]));

const requiredJobs = [
  'TypeScript',
  'Rust (default features)',
  'Rust (tauri feature)',
  '示例工程 substrate-only',
  'cargo fmt --check',
  'cargo clippy',
  'cargo-deny（hard gate）',
  'Target Matrix (linux-x64)',
  'Target Matrix (windows-x64)',
  'Target Matrix (macos-arm64)',
  'Target Matrix (macos-x64)',
];

for (const name of requiredJobs) {
  const job = byName.get(name);
  if (!job) throw new Error(`successful source CI is missing required job: ${name}`);
  if (job.status !== 'completed' || job.conclusion !== 'success') {
    throw new Error(`required source CI job did not succeed: ${name} (${job.conclusion})`);
  }
}

const requiredSteps = {
  'Rust (default features)': [
    'V4 Host Conformance（transport-neutral）',
    'FFI Ownership / ASan（V4 A72/A107）',
    'Local Host UDS reference E2E（V4 A106）',
    'Local Host broker chaos / peer-auth（V4 A108 local）',
  ],
  TypeScript: [
    'Public Surface Ledger（V4 A105）',
    'Target Matrix Drift（V4 A99）',
    'Release Evidence 输入校验（V4 A110）',
    'No-Lock-Across-Await（V4 A104）',
    '跨语言契约门禁（wire-gate）',
  ],
};

for (const [jobName, stepNames] of Object.entries(requiredSteps)) {
  const job = byName.get(jobName);
  const steps = new Map((job.steps ?? []).map((step) => [step.name, step]));
  for (const stepName of stepNames) {
    const step = steps.get(stepName);
    if (!step || step.conclusion !== 'success') {
      throw new Error(`source CI proof missing successful step: ${jobName} / ${stepName}`);
    }
  }
}

const proof = {
  schemaVersion: 1,
  repository: repo,
  sourceSha: sha,
  workflowRunId: run.id,
  workflowUrl: run.html_url,
  event: run.event,
  conclusion: run.conclusion,
  requiredJobs,
  requiredSteps,
  observedJobs: jobs.map((job) => ({
    name: job.name,
    conclusion: job.conclusion,
    runnerName: job.runner_name ?? null,
  })),
};

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify(proof, null, 2) + '\n');

if (process.env.GITHUB_OUTPUT) {
  const fs = await import('node:fs');
  fs.appendFileSync(process.env.GITHUB_OUTPUT, `ci_run_id=${run.id}\nci_run_url=${run.html_url}\n`);
}
console.log(`Exact-SHA source CI proof OK: ${sha} -> run ${run.id}`);
