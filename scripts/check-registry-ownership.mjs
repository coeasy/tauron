#!/usr/bin/env node
// Authenticated preflight: ensure registry names are free or writable by this publisher.
import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const npmToken = process.env.NPM_TOKEN;
const cargoToken = process.env.CARGO_REGISTRY_TOKEN;
if (!npmToken || !cargoToken) throw new Error('NPM_TOKEN and CARGO_REGISTRY_TOKEN are required.');

function run(command, args) {
  return execFileSync(command, args, { cwd: ROOT, encoding: 'utf8', env: process.env });
}

function missingOnNpm(name) {
  try {
    run('npm', ['view', name, 'name', '--json', '--fetch-retries=0']);
    return false;
  } catch (error) {
    const output = `${error.stdout ?? ''}\n${error.stderr ?? ''}`;
    if (/E404|404 Not Found/.test(output)) return true;
    throw new Error(`Could not verify npm package ${name}: ${output.trim()}`);
  }
}

function assertNpmWritable(name, npmUser) {
  if (missingOnNpm(name)) {
    console.log(`npm ${name}: name is not registered`);
    return;
  }
  let access;
  try {
    access = JSON.parse(run('npm', ['access', 'list', 'collaborators', name, npmUser, '--json']));
  } catch (error) {
    throw new Error(`npm ${name} exists, but write access could not be verified: ${error.message}`);
  }
  const role = access[npmUser];
  if (role !== 'read-write') {
    throw new Error(`npm ${name} exists but ${npmUser} has no verified read-write access.`);
  }
  console.log(`npm ${name}: ${npmUser} has read-write access`);
}

const npmUser = run('npm', ['whoami', '--registry=https://registry.npmjs.org/']).trim();
const packageFiles = readdirSync(join(ROOT, 'packages')).map((directory) =>
  join(ROOT, 'packages', directory, 'package.json'),
);
const packages = packageFiles
  .map((file) => JSON.parse(readFileSync(file, 'utf8')))
  .filter((pkg) => pkg.private !== true);

if (packages.some((pkg) => pkg.name.startsWith('@tauron/')) && npmUser !== 'tauron') {
  let hasWriteScopeAccess = false;
  const scopeCheckErrors = [];
  try {
    const membership = run('npm', ['org', 'ls', 'tauron', npmUser]);
    hasWriteScopeAccess = /owner|admin/i.test(membership);
  } catch (error) {
    hasWriteScopeAccess = false;
    scopeCheckErrors.push(`${error.stdout ?? ''}\n${error.stderr ?? ''}`);
  }
  if (!hasWriteScopeAccess) {
    try {
      const developers = run('npm', ['team', 'ls', '@tauron:developers']);
      hasWriteScopeAccess = developers
        .split(/\s+/)
        .some((entry) => entry.replace(/^@/, '') === npmUser);
    } catch (error) {
      hasWriteScopeAccess = false;
      scopeCheckErrors.push(`${error.stdout ?? ''}\n${error.stderr ?? ''}`);
    }
  }
  if (!hasWriteScopeAccess) {
    const registryDenied = scopeCheckErrors.some((output) =>
      /E403|403 Forbidden|You may not perform that action/i.test(output),
    );
    if (registryDenied) {
      console.warn(
        `npm authenticated as ${npmUser}, but npm denied organization membership metadata checks for @tauron (HTTP 403). This does not prove the account lacks access. Continuing to per-package access checks; npm publish will authoritatively verify permission for new package names.`,
      );
    } else {
      throw new Error(
        `npm authenticated as ${npmUser}, but its membership in the @tauron publishing organization could not be verified. Grant this account publishing access in npm organization settings and rerun the release preflight.`,
      );
    }
  }
}
for (const pkg of packages) assertNpmWritable(pkg.name, npmUser);

const cratesDir = join(ROOT, 'crates');
const crateNames = readdirSync(cratesDir, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => {
    const manifest = readFileSync(join(cratesDir, entry.name, 'Cargo.toml'), 'utf8');
    const name = /^name\s*=\s*"([^"]+)"/m.exec(manifest)?.[1];
    if (!name) throw new Error(`Could not read crate name from crates/${entry.name}/Cargo.toml`);
    return name;
  });
// crates.io does not accept API-token authentication on its account identity
// endpoint. Probe the publish endpoint with an empty body instead: authentication
// runs before archive validation, so a 400/422 confirms publish access without
// creating a crate or uploading a package.
const publishProbe = await fetch('https://crates.io/api/v1/crates/new', {
  method: 'PUT',
  headers: {
    accept: 'application/json',
    authorization: cargoToken,
    'content-type': 'application/octet-stream',
    'user-agent': 'Tauron SDK release preflight',
  },
  body: new Uint8Array(),
});
if (publishProbe.status === 401 || publishProbe.status === 403) {
  throw new Error(
    `CARGO_REGISTRY_TOKEN was rejected by the crates.io publish endpoint (HTTP ${publishProbe.status}). Create a token with permission to publish new crates (and a crate scope covering the Tauron crate names), update the repository secret, and rerun the release preflight.`,
  );
}
if (![200, 400, 422].includes(publishProbe.status)) {
  const detail = (await publishProbe.text()).slice(0, 500);
  throw new Error(
    `Could not verify crates.io publishing access (HTTP ${publishProbe.status}): ${detail}`,
  );
}
console.log(
  `crates.io publish token: accepted (empty-body probe returned HTTP ${publishProbe.status}; no crate was created)`,
);
const expectedCratesUser = process.env.CRATES_IO_PUBLISHER?.trim();
for (const name of crateNames) {
  const response = await fetch(`https://crates.io/api/v1/crates/${name}`, {
    headers: { 'user-agent': 'Tauron SDK release preflight' },
  });
  if (response.status === 404) {
    console.log(`crates.io ${name}: name is not registered`);
    continue;
  }
  if (!response.ok) throw new Error(`Could not verify crates.io ${name}: HTTP ${response.status}`);
  if (!expectedCratesUser) {
    throw new Error(
      `crate ${name} already exists. Set CRATES_IO_PUBLISHER to the expected crates.io owner login so its ownership can be checked before publishing.`,
    );
  }
  let owners;
  try {
    owners = run('cargo', ['owner', '--list', name]);
  } catch (error) {
    throw new Error(
      `crate ${name} exists, but publisher ownership could not be verified: ${error.message}`,
    );
  }
  const normalized = owners.toLowerCase();
  if (!normalized.includes(expectedCratesUser.toLowerCase())) {
    throw new Error(
      `crate ${name} exists; expected crates.io publisher ${expectedCratesUser} is not listed as its owner.`,
    );
  }
  console.log(`crates.io ${name}: publisher ${expectedCratesUser} is an owner`);
}

console.log(
  `Registry ownership preflight passed for ${packages.length} npm packages and ${crateNames.length} Rust crates.`,
);
