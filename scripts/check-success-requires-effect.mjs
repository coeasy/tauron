#!/usr/bin/env node
// V7 §9 `success_requires_effect`: a success-shaped result must be backed by an
// effect token, a commit marker, or a real runtime observation — otherwise it
// has to say `simulated: true` in the same object.
//
// Why this gate exists (the regression it is named after, all of them real and
// all of them already fixed by hand in earlier rounds):
//   - `tauron-market`'s `install` / `uninstall` returned `success: true` from an
//     in-process mock that downloads, verifies and writes nothing.
//   - `tauron-cli`'s `plugin publish` returned `success: true` plus a fabricated
//     `registryUrl` without sending a single request.
//   - `tauron-adapter`'s `host_market_*` stubs returned bare `ok: true`, leaving
//     the frontend to guess whether bytes moved. (Round 40 split them into
//     dispatcher + twins: a `_wired` leg's seam call is its effect token, and
//     the sibling gate pins the `simulated: false` claim to those two bodies.)
// Text-level rules only — the behavioural half lives in
// `check-simulated-never-commits.mjs`.

import { collectFiles, linesOf, stripped, blockAt, findBlock, finish } from './lib/gate-scan.mjs';

const failures = [];

function projection(file) {
  const raw = linesOf(file);
  return { raw, cut: stripped(raw), file };
}

function slice(view, block) {
  return view.cut.slice(block.start, block.end + 1).join('\n');
}

/// Every `pub struct <name> … { … }` block in the file.
function rustStructs(view, namePattern) {
  const out = [];
  for (let i = 0; i < view.cut.length; i += 1) {
    const match = view.cut[i].match(/^\s*pub struct\s+(\w+)\s*\{?/);
    if (!match || !namePattern.test(match[1])) continue;
    const block = blockAt(view.raw, i);
    if (!block) continue;
    out.push({ name: match[1], index: i, body: slice(view, block) });
  }
  return out;
}

/// Every construction site `Name {` that is not the declaration itself.
function rustLiterals(view, names) {
  const out = [];
  for (let i = 0; i < view.cut.length; i += 1) {
    const match = view.cut[i].match(/(\w+)\s*\{/);
    if (!match) continue;
    const name = match[1];
    if (!names.includes(name)) continue;
    if (/^\s*pub struct\s/.test(view.cut[i])) continue;
    const block = blockAt(view.raw, i);
    if (!block) continue;
    out.push({ name, index: i, body: slice(view, block) });
  }
  return out;
}

// ── 1. tauron-market: every `*Response` that carries `success` must carry an
//      effect marker (`simulated`) or a real effect-token field.
//
// `RevokeResponse` is the allowlisted case: `revoke()` mutates the crate's own
// revocation list, which is the authoritative structure `is_plugin_revoked()`
// reads back — that is a real in-scope effect, so `revoked_at` is its token.
const EFFECT_TOKENS = new Map([['RevokeResponse', 'revoked_at:']]);
const marketView = projection('crates/tauron-market/src/api.rs');
const marketResponses = rustStructs(marketView, /Response$/);
if (marketResponses.length === 0) {
  failures.push('crates/tauron-market/src/api.rs: no `pub struct *Response` found — gate blind');
}
const declared = new Map();
for (const struct of marketResponses) {
  if (!struct.body.includes('pub success:')) {
    declared.set(struct.name, null);
    continue;
  }
  const effectToken = EFFECT_TOKENS.get(struct.name);
  const hasSimulated = struct.body.includes('pub simulated:');
  if (!hasSimulated && !effectToken) {
    failures.push(
      `crates/tauron-market/src/api.rs:${struct.index + 1}: \`${struct.name}\` declares ` +
        '`success` but no effect marker — it must carry `simulated: bool` (or a declared ' +
        'effect-token field)',
    );
  }
  declared.set(struct.name, { hasSimulated, effectToken: effectToken ?? null });
}
for (const literal of rustLiterals(marketView, [...declared.keys()])) {
  const info = declared.get(literal.name);
  if (!info) continue;
  if (info.hasSimulated && !literal.body.includes('simulated: true')) {
    failures.push(
      `crates/tauron-market/src/api.rs:${literal.index + 1}: \`${literal.name}\` is built by a ` +
        'mock client, so it must be honest: `simulated: true`',
    );
  }
  if (!info.hasSimulated && info.effectToken && !literal.body.includes(info.effectToken)) {
    failures.push(
      `crates/tauron-market/src/api.rs:${literal.index + 1}: \`${literal.name}\` reports ` +
        `success and must carry its effect token \`${info.effectToken}\``,
    );
  }
}

// ── 2. tauron-adapter: the stub vocabulary is `ok` / `available` + `simulated`.
const adapterFiles = collectFiles(['crates/tauron-adapter/src'], ['.rs']);
for (const file of adapterFiles) {
  const view = projection(file);
  view.cut.forEach((line, index) => {
    if (line.includes('success: true')) {
      failures.push(
        `${file}:${index + 1}: \`success: true\` in the adapter is an unbacked success claim — ` +
          'this crate reports `ok` / `available` plus `simulated`, never bare success',
      );
    }
    for (const banned of [
      'MarketplaceApi',
      'create_default_marketplace_api',
      'InstallRequest',
      'InstallResponse',
    ]) {
      if (line.includes(banned)) {
        failures.push(
          `${file}:${index + 1}: adapter references \`${banned}\` — the in-process mock ` +
            'marketplace API must never become a production consumer of `host_market_*`',
        );
      }
    }
  });
}
const adapterLib = projection('crates/tauron-adapter/src/lib.rs');
const stubResults = ['MarketCheckResult', 'MarketUpdateResult'];
for (const struct of rustStructs(adapterLib, /^(MarketCheckResult|MarketUpdateResult)$/)) {
  if (!struct.body.includes('pub simulated:')) {
    failures.push(
      `crates/tauron-adapter/src/lib.rs:${struct.index + 1}: \`${struct.name}\` must declare ` +
        '`pub simulated: bool` so a stub result can never be read as an effect',
    );
  }
}
// Round 40: two `MarketUpdateResult` construction sites are the wired twins of
// the dispatch pair (`_wired` legs). They may say `simulated: false` because
// the seam call in the same function is the effect; the sibling gate pins that
// claim to these exact bodies and to the name-pinned observation tests. Any
// other `simulated: false` construction site is still an unbacked claim.
const wiredLegs = [];
for (const name of ['cmd_market_download_wired', 'cmd_market_install_wired']) {
  const block = findBlock(adapterLib.raw, new RegExp(`^\\s*fn ${name}\\s*\\(`));
  if (block) wiredLegs.push(block);
}
for (const literal of rustLiterals(adapterLib, stubResults)) {
  const leg = wiredLegs.find((b) => literal.index >= b.start && literal.index <= b.end);
  if (leg) {
    if (!/upgrade_installer\.(download|install)\(\)/.test(slice(adapterLib, leg))) {
      failures.push(
        `crates/tauron-adapter/src/lib.rs:${literal.index + 1}: \`${literal.name}\` claims a ` +
          'real effect (`simulated: false`) inside a wired leg, but the function has no seam ' +
          'call (`upgrade_installer.download()` / `.install()`) — the claim needs a real effect ' +
          'in the same function',
      );
    }
    continue;
  }
  if (!literal.body.includes('simulated: true')) {
    failures.push(
      `crates/tauron-adapter/src/lib.rs:${literal.index + 1}: \`${literal.name}\` is built by a ` +
        'stub that performs no network / filesystem work — it must say `simulated: true` ' +
        'and carry a `reason`',
    );
  }
}

// ── 3. tauron-cli: a `success: true` result must state its simulation flag.
const cliFile = 'packages/tauron-cli/src/plugin-lifecycle.ts';
const cliView = projection(cliFile);
cliView.cut.forEach((line, index) => {
  if (line.includes('simulated: false')) {
    failures.push(
      `${cliFile}:${index + 1}: \`simulated: false\` claims a real effect; the CLI has no ` +
        'downloader / uploader / archive writer, so it must not assert one',
    );
  }
});
let successBlocks = 0;
for (let i = 0; i < cliView.cut.length; i += 1) {
  if (!/\breturn\s*\{/.test(cliView.cut[i])) continue;
  const block = blockAt(cliView.raw, i);
  if (!block) continue;
  const body = slice(cliView, block);
  if (!body.includes('success: true')) continue;
  successBlocks += 1;
  if (!body.includes('simulated: true')) {
    failures.push(
      `${cliFile}:${i + 1}: this \`return { success: true }\` has no effect token — it must ` +
        'carry `simulated: true` (and describe what actually landed) in the same result',
    );
  }
}
if (successBlocks === 0) {
  failures.push(`${cliFile}: no \`return { success: true }\` found — gate blind on the CLI path`);
}

finish('Success-Requires-Effect', failures, adapterFiles.length + 3);
