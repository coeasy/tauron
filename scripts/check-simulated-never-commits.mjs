#!/usr/bin/env node
// V7 §9 `simulated_never_commits`: a simulated result must not be able to reach
// install / update / restart, and a real commit must not be able to happen
// without its commit marker.
//
// The two halves of the invariant, both already violated once in this repo:
//   A. **Simulated must not commit.** `host_market_download` / `host_market_install`
//      used to be pure stubs; the host client would advance its own state machine
//      to `downloaded` / `ready` purely because the stub returned `ok: true`, and
//      `relaunch()` would then quit the app on a fake "update installed".
//      The landed shape: the TS client throws on `simulated` **before** it
//      invokes the host and again on the host's answer, so the status machine
//      never leaves the error state. Round 40 gave the adapter side a real leg
//      (dispatcher + wired/simulated twins, see A2): the simulated twins keep
//      the stub rule literally, and the wired twins are the sanctioned effect
//      path — gated by the probe and observed by name-pinned tests.
//   B. **Real must not skip its marker.** The upgrade runner is the only place
//      that may claim an upgrade happened, and only after the journal's commit
//      marker survived a reload from disk. It has no `simulated` mode at all —
//      so a `simulated` marker appearing there would mean the execution side
//      was downgraded to a mock.
//
// The companion gate `check-success-requires-effect.mjs` checks the *shape* of
// results; this one checks the *behaviour* around them.

import { linesOf, stripped, findBlock, finish } from './lib/gate-scan.mjs';

const failures = [];

function view(file) {
  const raw = linesOf(file);
  return { file, raw, cut: stripped(raw) };
}

function bodyOf(v, headerPattern, label) {
  const block = findBlock(v.raw, headerPattern);
  if (!block) {
    failures.push(`${v.file}: ${label} not found — the gate cannot verify its invariants`);
    return null;
  }
  return {
    index: block.headerIndex,
    start: block.start,
    end: block.end,
    // `body` is the comment/string-blanked projection: safe for code needles.
    body: v.cut.slice(block.start, block.end + 1).join('\n'),
    // `rawBody` keeps string literals: needed when the needle *is* a literal
    // (host command names, status-machine states). Inline comments then count
    // as text — acceptable here because these bodies carry no such comment.
    rawBody: v.raw.slice(block.start, block.end + 1).join('\n'),
  };
}

/// `needle` must occur before `anchor` inside `body`.
function before(where, label, needle, anchor, file, index) {
  const needleAt = where.indexOf(needle);
  const anchorAt = where.indexOf(anchor);
  if (needleAt === -1) {
    failures.push(
      `${file}:${index + 1}: ${label} must \`${needle}\` — ${anchor} may not run first`,
    );
    return;
  }
  if (anchorAt !== -1 && needleAt > anchorAt) {
    failures.push(
      `${file}:${index + 1}: ${label} must reject **before** \`${anchor}\` (found \`${needle}\` ` +
        'after it — a simulated answer already reached the effect)',
    );
  }
}

// ── A1. Host auto-update client: download / install / relaunch.
const hostFile = 'packages/tauron-host/src/auto-update-client.ts';
const host = view(hostFile);

const download = bodyOf(host, /^\s*async downloadUpdate\s*\(/, '`downloadUpdate()`');
if (download) {
  before(
    download.body,
    'downloadUpdate',
    'simulated === true',
    'invoke<',
    hostFile,
    download.index,
  );
  before(
    download.rawBody,
    'downloadUpdate',
    'result?.simulated',
    "this._setStatus('downloaded')",
    hostFile,
    download.index,
  );
  if (!download.body.includes('throw new Error')) {
    failures.push(
      `${hostFile}:${download.index + 1}: downloadUpdate must throw on a simulated answer, ` +
        'not swallow it',
    );
  }
}

const install = bodyOf(host, /^\s*async installUpdate\s*\(/, '`installUpdate()`');
if (install) {
  before(install.body, 'installUpdate', 'simulated === true', 'invoke<', hostFile, install.index);
  before(
    install.rawBody,
    'installUpdate',
    'result?.simulated',
    "this._setStatus('ready')",
    hostFile,
    install.index,
  );
  // `ready` is the state the UI reads as "restart to update"; it may only be
  // reached through a non-simulated install answer.
  if (!install.rawBody.includes("this._setStatus('ready')")) {
    failures.push(
      `${hostFile}:${install.index + 1}: installUpdate never reached 'ready' — the gate cannot ` +
        'prove the effect state is gated',
    );
  }
  if (!install.body.includes('throw new Error')) {
    failures.push(
      `${hostFile}:${install.index + 1}: installUpdate must throw on a simulated answer`,
    );
  }
}

const relaunch = bodyOf(host, /^\s*async relaunch\s*\(/, '`relaunch()`');
if (relaunch) {
  if (!relaunch.rawBody.includes('host_window_relaunch')) {
    failures.push(
      `${hostFile}:${relaunch.index + 1}: relaunch must go through \`host_window_relaunch\` — ` +
        '`host_window_quit` exits without coming back, which is not a restart',
    );
  }
  for (const needle of ['host_market_install', "_setStatus('ready')", 'downloadUpdate(']) {
    if (relaunch.rawBody.includes(needle)) {
      failures.push(
        `${hostFile}:${relaunch.index + 1}: relaunch must be a pure passthrough of the host ` +
          `outcome; \`${needle}\` means it started to decide the update state itself`,
      );
    }
  }
}

// ── A2. Adapter market surface: one stub + two dispatchers over a twin pair.
// Round 40 landed the assembly leg: `host_market_download` / `host_market_install`
// now split into a wired twin (real effect through the `UpgradeInstaller` seam,
// `simulated: false`) and a simulated twin (the old stub, `simulated: true`),
// with the dispatcher choosing by `native_supported()`. The old blanket rule
// ("these fns must say simulated: true") would now flag the dispatchers, so the
// gate proves the *split* instead: the stub stays effect-free, each dispatcher
// gates the wired twin behind the positive probe and never carries the effect
// itself, each simulated twin stays a stub, each wired twin claims
// `simulated: false` only after the seam call, and the observation tests that
// prove all of this exist by name.
const adapterFile = 'crates/tauron-adapter/src/lib.rs';
const adapter = view(adapterFile);
const FORBIDDEN_EFFECTS = [
  'fs::',
  'std::fs',
  'tauron_distribute',
  'tauron_market',
  'Command::new',
  'reqwest',
  'update_state = None',
];

// A2a. `host_market_check` — deliberate stub, doctrine unchanged (round 29).
{
  const fn = bodyOf(adapter, /^\s*pub fn cmd_market_check\s*\(/, '`cmd_market_check()`');
  if (fn) {
    if (!fn.body.includes('simulated: true')) {
      failures.push(
        `${adapterFile}:${fn.index + 1}: cmd_market_check is a stub; it must return \`simulated: ` +
          'true` together with a `reason`',
      );
    }
    for (const needle of FORBIDDEN_EFFECTS) {
      if (fn.body.includes(needle)) {
        failures.push(
          `${adapterFile}:${fn.index + 1}: cmd_market_check performs a real effect ` +
            `(\`${needle}\`) while still marked simulated — either wire the delegate and drop ` +
            'the stub, or remove it',
        );
      }
    }
    if (!fn.body.includes('reason')) {
      failures.push(
        `${adapterFile}:${fn.index + 1}: cmd_market_check must explain what it did not do ` +
          '(`reason`), otherwise the frontend has to guess',
      );
    }
  }
}

// A2b. Dispatchers + twins, pair by pair.
const wiredTwins = [];
for (const [dispatcher, wired, simulated, effect] of [
  [
    'cmd_market_download',
    'cmd_market_download_wired',
    'cmd_market_download_simulated',
    'upgrade_installer.download()',
  ],
  [
    'cmd_market_install',
    'cmd_market_install_wired',
    'cmd_market_install_simulated',
    'upgrade_installer.install()',
  ],
]) {
  const fn = bodyOf(adapter, new RegExp(`^\\s*pub fn ${dispatcher}\\s*\\(`), `\`${dispatcher}()\``);
  if (fn) {
    for (const twin of [wired, simulated]) {
      if (!fn.body.includes(twin)) {
        failures.push(
          `${adapterFile}:${fn.index + 1}: ${dispatcher} no longer routes to \`${twin}\` — the ` +
            'dispatcher must choose between exactly the wired and simulated twins',
        );
      }
    }
    // The wired twin may only run behind the positive probe (a wired-first
    // body would take the real path unconditionally)…
    before(fn.body, dispatcher, 'native_supported()', `${wired}(`, adapterFile, fn.index);
    // …and the simulated twin is the fallback, not the first branch.
    before(fn.body, dispatcher, `${wired}(`, `${simulated}(`, adapterFile, fn.index);
    for (const needle of FORBIDDEN_EFFECTS) {
      if (fn.body.includes(needle)) {
        failures.push(
          `${adapterFile}:${fn.index + 1}: ${dispatcher} performs a real effect (\`${needle}\`) — ` +
            'effects belong to the wired twin behind the seam',
        );
      }
    }
    if (fn.body.includes('simulated: true') || fn.body.includes('simulated: false')) {
      failures.push(
        `${adapterFile}:${fn.index + 1}: ${dispatcher} claims an outcome marker itself — the ` +
          'provenance claim belongs to the twins (the dispatcher has no answer of its own)',
      );
    }
  }

  const sim = bodyOf(adapter, new RegExp(`^\\s*fn ${simulated}\\s*\\(`), `\`${simulated}()\``);
  if (sim) {
    if (!sim.body.includes('simulated: true')) {
      failures.push(
        `${adapterFile}:${sim.index + 1}: ${simulated} is the stub leg; it must return ` +
          '`simulated: true` together with a `reason`',
      );
    }
    if (!sim.body.includes('update_state_simulated = true')) {
      failures.push(
        `${adapterFile}:${sim.index + 1}: ${simulated} must mark the ledger as simulated ` +
          '(`update_state_simulated = true`) — a simulated progress the status reader cannot ' +
          'distinguish is how round 33 happened',
      );
    }
    for (const needle of FORBIDDEN_EFFECTS) {
      if (sim.body.includes(needle)) {
        failures.push(
          `${adapterFile}:${sim.index + 1}: ${simulated} performs a real effect (\`${needle}\`) ` +
            'while still marked simulated — either wire the delegate and drop the stub, or remove it',
        );
      }
    }
    if (!sim.body.includes('reason')) {
      failures.push(
        `${adapterFile}:${sim.index + 1}: ${simulated} must explain what it did not do ` +
          '(`reason`), otherwise the frontend has to guess',
      );
    }
  }

  const w = bodyOf(adapter, new RegExp(`^\\s*fn ${wired}\\s*\\(`), `\`${wired}()\``);
  if (w) {
    wiredTwins.push(w);
    if (!w.body.includes('simulated: false')) {
      failures.push(
        `${adapterFile}:${w.index + 1}: ${wired} performs the real effect; it must claim ` +
          '`simulated: false` (with the effect observed by a test — the cross-check below only ' +
          'allows the claim inside this body)',
      );
    }
    if (!w.body.includes(effect)) {
      failures.push(
        `${adapterFile}:${w.index + 1}: ${wired} no longer calls \`${effect}\` — the wired leg ` +
          'must take its effect from the seam, not from its own bookkeeping',
      );
    }
    // The provenance flip may only follow the successful effect: a claim
    // written before the seam call would mark "real" even when it fails.
    before(w.body, wired, effect, 'update_state_simulated = false', adapterFile, w.index);
    if (w.body.includes('simulated: true')) {
      failures.push(
        `${adapterFile}:${w.index + 1}: ${wired} is the real leg; \`simulated: true\` would ` +
          'misreport a committed effect as a simulation',
      );
    }
  }
}

// A2c. The observation tests are part of the gate: name-pinned, in the adapter.
{
  const all = adapter.cut.join('\n');
  if (!all.includes('mod round40_market_wiring_tests')) {
    failures.push(
      `${adapterFile}: the round-40 wiring observation tests are gone — the wire-gate allowances ` +
        'rest on effects that a test must observe',
    );
  }
  for (const test of [
    'wired_download_stages_verifies_and_flips_provenance',
    'wired_download_verify_failure_leaves_ledger_untouched',
    'wired_install_swaps_commits_restarts_and_flips_provenance',
    'wired_install_without_staged_package_is_typed_error',
    'wired_install_health_failure_rolls_back_and_leaves_ledger_untouched',
    'wired_market_commands_gate_and_audit_hold',
  ]) {
    if (!new RegExp(`fn ${test}\\b`).test(all)) {
      failures.push(
        `${adapterFile}: observation test \`${test}\` is missing — the wired-leg claims lose ` +
          'their behavioural proof',
      );
    }
  }
}

// ── A3. Market mock: honest markers, and no filesystem writes.
const marketFile = 'crates/tauron-market/src/api.rs';
const market = view(marketFile);
for (const name of ['install', 'uninstall']) {
  const fn = bodyOf(
    market,
    new RegExp(`^\\s*pub fn ${name}\\s*\\(\\s*&mut self`),
    `\`MarketplaceApi::${name}()\``,
  );
  if (!fn) continue;
  if (!fn.body.includes('simulated: true')) {
    failures.push(
      `${marketFile}:${fn.index + 1}: ${name} is an in-process mock; it must return \`simulated: ` +
        'true`',
    );
  }
  for (const needle of ['fs::write', 'fs::create_dir', 'File::create', 'fs::remove', 'std::fs']) {
    if (fn.body.includes(needle)) {
      failures.push(
        `${marketFile}:${fn.index + 1}: mock ${name} performed a filesystem write/remove ` +
          `(\`${needle}\`) — the simulated path must not commit`,
      );
    }
  }
}

// ── A4. CLI: the un-backed commands keep failing closed.
const cliFile = 'packages/tauron-cli/src/plugin-lifecycle.ts';
const cli = view(cliFile);
for (const name of ['pluginDev', 'pluginTest', 'pluginPack', 'pluginPublish']) {
  const fn = bodyOf(cli, new RegExp(`^\\s*export function ${name}\\s*\\(`), `\`${name}()\``);
  if (!fn) continue;
  if (!fn.body.includes('success: false')) {
    failures.push(
      `${cliFile}:${fn.index + 1}: ${name} has no real effect available, so every branch must ` +
        'return `success: false` (non-zero exit), never a simulated success',
    );
  }
  if (fn.body.includes('success: true')) {
    failures.push(
      `${cliFile}:${fn.index + 1}: ${name} returned ` +
        '`success: true` — that command cannot commit anything in this repo',
    );
  }
}
const publish = bodyOf(cli, /^\s*export function pluginPublish\s*\(/, '`pluginPublish()`');
if (publish && !publish.body.includes('published: false')) {
  failures.push(
    `${cliFile}:${publish.index + 1}: pluginPublish must state \`published: false` +
      '` explicitly; an absent flag reads as "published" to a script',
  );
}

// ── B. The real execution side has no simulated mode, and success needs a marker.
const upgradeFile = 'crates/tauron-distribute/src/upgrade.rs';
const upgrade = view(upgradeFile);
upgrade.cut.forEach((line, index) => {
  if (line.includes('simulated')) {
    failures.push(
      `${upgradeFile}:${index + 1}: the upgrade execution side must not grow a simulated mode ` +
        '— a real runner either commits through the journal or returns an error',
    );
  }
});
const phases = bodyOf(upgrade, /^\s*(pub )?fn run_phases\s*\(/, '`UpgradeRunner::run_phases`');
if (phases) {
  before(
    phases.body,
    'run_phases',
    'ctx.mark_committed()',
    'success: true',
    upgradeFile,
    phases.index,
  );
}
const mark = bodyOf(upgrade, /^\s*fn mark_committed\s*\(/, '`OpContext::mark_committed`');
if (mark) {
  for (const needle of ['commit_marker = true', 'journal.save(', 'UpgradeJournal::load(']) {
    if (!mark.body.includes(needle)) {
      failures.push(
        `${upgradeFile}:${mark.index + 1}: mark_committed must ${needle} — the commit marker is ` +
          'the only effect token an upgrade success is allowed to rest on',
      );
    }
  }
}

// ── Cross-check: a `simulated: false` claim asserts a real effect. Only the two
// wired twins (round 40) carry an effect the gate has already corroborated —
// the seam call, gated by the dispatcher probe and observed by the pinned
// tests — so the claim is allowed there and rejected everywhere else in the
// gated sources.
const wiredRanges = wiredTwins.map((w) => [w.start, w.end]);
for (const v of [adapter, market, host, cli]) {
  v.cut.forEach((line, index) => {
    if (!line.includes('simulated: false')) return;
    if (v.file === adapterFile && wiredRanges.some(([s, e]) => index >= s && index <= e)) return;
    failures.push(
      `${v.file}:${index + 1}: \`simulated: false\` asserts a real effect; landing that effect ` +
        'requires the delegate to be wired plus a test that observes it',
    );
  });
}

finish('Simulated-Never-Commits', failures, 6);
