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
//
// 用法：
//   node scripts/check-simulated-never-commits.mjs               # 校验
//   node scripts/check-simulated-never-commits.mjs --self-test   # 变异自证（每条针有牙）

import { readFileSync } from 'node:fs';

import { stripped, findBlock, finish } from './lib/gate-scan.mjs';

const hostFile = 'packages/tauron-host/src/auto-update-client.ts';
const adapterFile = 'crates/tauron-adapter/src/lib.rs';
// T-7 十七片之三：商城命令族（cmd_market_* + 双胞胎腿）已从 lib.rs 逐字节纯搬移到 market.rs，
// 故本门禁里针对这些命令**函数体**的 A2a/A2b 检查改读 market.rs；A2c 观察测试模块
// （round40_market_wiring_tests）仍留 lib.rs，继续读 adapter。判据一字未改，只换了归属文件。
const adapterMarketFile = 'crates/tauron-adapter/src/market.rs';
const marketFile = 'crates/tauron-market/src/api.rs';
const cliFile = 'packages/tauron-cli/src/plugin-lifecycle.ts';
const upgradeFile = 'crates/tauron-distribute/src/upgrade.rs';

const KEY_TO_PATH = {
  host: hostFile,
  adapter: adapterFile,
  adapterMarket: adapterMarketFile,
  market: marketFile,
  cli: cliFile,
  upgrade: upgradeFile,
};
const toLines = (text) => text.split(/\r?\n/);

const FORBIDDEN_EFFECTS = [
  'fs::',
  'std::fs',
  'tauron_distribute',
  'tauron_market',
  'Command::new',
  'reqwest',
  'update_state = None',
];

function loadSources() {
  const src = {};
  for (const [key, path] of Object.entries(KEY_TO_PATH)) src[key] = readFileSync(path, 'utf8');
  return src;
}

/** 对给定源码快照跑全部不变量，返回 failures（纯函数，供校验与 self-test 共用）。 */
function runChecks(src) {
  const failures = [];

  const view = (key) => {
    const file = KEY_TO_PATH[key];
    const raw = toLines(src[key]);
    return { file, key, raw, cut: stripped(raw) };
  };

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
  const host = view('host');

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
  const adapter = view('adapter');
  // 商城命令函数体自 T-7 十七片之三起住在 market.rs；A2a/A2b 的体读改指 adapterMarket，
  // A2c 的观察测试模块仍在 lib.rs（adapter）。
  const adapterMarket = view('adapterMarket');

  // A2a. `host_market_check` — deliberate stub, doctrine unchanged (round 29).
  {
    const fn = bodyOf(adapterMarket, /^\s*pub fn cmd_market_check\s*\(/, '`cmd_market_check()`');
    if (fn) {
      if (!fn.body.includes('simulated: true')) {
        failures.push(
          `${adapterMarketFile}:${fn.index + 1}: cmd_market_check is a stub; it must return \`simulated: ` +
            'true` together with a `reason`',
        );
      }
      for (const needle of FORBIDDEN_EFFECTS) {
        if (fn.body.includes(needle)) {
          failures.push(
            `${adapterMarketFile}:${fn.index + 1}: cmd_market_check performs a real effect ` +
              `(\`${needle}\`) while still marked simulated — either wire the delegate and drop ` +
              'the stub, or remove it',
          );
        }
      }
      if (!fn.body.includes('reason')) {
        failures.push(
          `${adapterMarketFile}:${fn.index + 1}: cmd_market_check must explain what it did not do ` +
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
    const fn = bodyOf(
      adapterMarket,
      new RegExp(`^\\s*pub fn ${dispatcher}\\s*\\(`),
      `\`${dispatcher}()\``,
    );
    if (fn) {
      for (const twin of [wired, simulated]) {
        if (!fn.body.includes(twin)) {
          failures.push(
            `${adapterMarketFile}:${fn.index + 1}: ${dispatcher} no longer routes to \`${twin}\` — the ` +
              'dispatcher must choose between exactly the wired and simulated twins',
          );
        }
      }
      // The wired twin may only run behind the positive probe (a wired-first
      // body would take the real path unconditionally)…
      before(fn.body, dispatcher, 'native_supported()', `${wired}(`, adapterMarketFile, fn.index);
      // …and the simulated twin is the fallback, not the first branch.
      before(fn.body, dispatcher, `${wired}(`, `${simulated}(`, adapterMarketFile, fn.index);
      for (const needle of FORBIDDEN_EFFECTS) {
        if (fn.body.includes(needle)) {
          failures.push(
            `${adapterMarketFile}:${fn.index + 1}: ${dispatcher} performs a real effect (\`${needle}\`) — ` +
              'effects belong to the wired twin behind the seam',
          );
        }
      }
      if (fn.body.includes('simulated: true') || fn.body.includes('simulated: false')) {
        failures.push(
          `${adapterMarketFile}:${fn.index + 1}: ${dispatcher} claims an outcome marker itself — the ` +
            'provenance claim belongs to the twins (the dispatcher has no answer of its own)',
        );
      }
    }

    const sim = bodyOf(
      adapterMarket,
      new RegExp(`^\\s*fn ${simulated}\\s*\\(`),
      `\`${simulated}()\``,
    );
    if (sim) {
      if (!sim.body.includes('simulated: true')) {
        failures.push(
          `${adapterMarketFile}:${sim.index + 1}: ${simulated} is the stub leg; it must return ` +
            '`simulated: true` together with a `reason`',
        );
      }
      if (!sim.body.includes('update_state_simulated = true')) {
        failures.push(
          `${adapterMarketFile}:${sim.index + 1}: ${simulated} must mark the ledger as simulated ` +
            '(`update_state_simulated = true`) — a simulated progress the status reader cannot ' +
            'distinguish is how round 33 happened',
        );
      }
      for (const needle of FORBIDDEN_EFFECTS) {
        if (sim.body.includes(needle)) {
          failures.push(
            `${adapterMarketFile}:${sim.index + 1}: ${simulated} performs a real effect (\`${needle}\`) ` +
              'while still marked simulated — either wire the delegate and drop the stub, or remove it',
          );
        }
      }
      if (!sim.body.includes('reason')) {
        failures.push(
          `${adapterMarketFile}:${sim.index + 1}: ${simulated} must explain what it did not do ` +
            '(`reason`), otherwise the frontend has to guess',
        );
      }
    }

    const w = bodyOf(adapterMarket, new RegExp(`^\\s*fn ${wired}\\s*\\(`), `\`${wired}()\``);
    if (w) {
      wiredTwins.push(w);
      if (!w.body.includes('simulated: false')) {
        failures.push(
          `${adapterMarketFile}:${w.index + 1}: ${wired} performs the real effect; it must claim ` +
            '`simulated: false` (with the effect observed by a test — the cross-check below only ' +
            'allows the claim inside this body)',
        );
      }
      if (!w.body.includes(effect)) {
        failures.push(
          `${adapterMarketFile}:${w.index + 1}: ${wired} no longer calls \`${effect}\` — the wired leg ` +
            'must take its effect from the seam, not from its own bookkeeping',
        );
      }
      // The provenance flip may only follow the successful effect: a claim
      // written before the seam call would mark "real" even when it fails.
      before(w.body, wired, effect, 'update_state_simulated = false', adapterMarketFile, w.index);
      if (w.body.includes('simulated: true')) {
        failures.push(
          `${adapterMarketFile}:${w.index + 1}: ${wired} is the real leg; \`simulated: true\` would ` +
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
  const market = view('market');
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
  const cli = view('cli');
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
  const upgrade = view('upgrade');
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
  // wired twins (round 40) carry an effect the gate has already corroborated.
  const wiredRanges = wiredTwins.map((w) => [w.start, w.end]);
  for (const v of [adapter, adapterMarket, market, host, cli]) {
    v.cut.forEach((line, index) => {
      if (!line.includes('simulated: false')) return;
      if (v.file === adapterMarketFile && wiredRanges.some(([s, e]) => index >= s && index <= e))
        return;
      failures.push(
        `${v.file}:${index + 1}: \`simulated: false\` asserts a real effect; landing that effect ` +
          'requires the delegate to be wired plus a test that observes it',
      );
    });
  }

  return failures;
}

// ── self-test：证明每条针有牙（delete 删全部出现、inject 注入被禁形状）────────
const MUTATIONS = [
  {
    name: 'A1 host 不再于 invoke 前判 simulated === true',
    op: 'delete',
    file: 'host',
    needle: 'simulated === true',
    expect: 'simulated === true',
  },
  {
    name: 'A1 host download/install 不再对模拟应答 throw',
    op: 'delete',
    file: 'host',
    needle: 'throw new Error',
    expect: 'must throw',
  },
  {
    name: 'A1 host relaunch 方法消失（门禁找不到其体）',
    op: 'delete',
    file: 'host',
    needle: 'async relaunch',
    expect: 'relaunch',
  },
  {
    name: 'A2a cmd_market_check 不再标 simulated: true',
    op: 'delete',
    file: 'adapterMarket',
    needle: 'simulated: true',
    expect: 'cmd_market_check',
  },
  {
    name: 'A2b wired download 不再调 seam download()',
    op: 'delete',
    file: 'adapterMarket',
    needle: 'upgrade_installer.download()',
    expect: 'cmd_market_download_wired',
  },
  {
    name: 'A2b simulated twin 不再标 ledger simulated',
    op: 'delete',
    file: 'adapterMarket',
    needle: 'update_state_simulated = true',
    expect: 'ledger as simulated',
  },
  {
    name: 'A2b dispatcher 不再路由到 wired twin',
    op: 'delete',
    file: 'adapterMarket',
    needle: 'cmd_market_download_wired',
    expect: 'no longer routes',
  },
  {
    name: 'A2c round40 观察测试整块消失',
    op: 'delete',
    file: 'adapter',
    needle: 'round40_market_wiring_tests',
    expect: 'observation tests are gone',
  },
  {
    name: 'A3 market mock 不再标 simulated: true',
    op: 'delete',
    file: 'market',
    needle: 'simulated: true',
    expect: 'in-process mock',
  },
  {
    name: 'A4 cli 无凭据分支不再 success: false 兜底',
    op: 'delete',
    file: 'cli',
    needle: 'success: false',
    expect: 'no real effect available',
  },
  {
    name: 'A4 pluginPublish 不再显式 published: false',
    op: 'delete',
    file: 'cli',
    needle: 'published: false',
    expect: 'published: false',
  },
  {
    name: 'B upgrade 执行侧长出 simulated 模式',
    op: 'inject',
    file: 'upgrade',
    line: 'let simulated = 0;',
    expect: 'grow a simulated mode',
  },
  {
    name: 'B run_phases 跳过 ctx.mark_committed()',
    op: 'delete',
    file: 'upgrade',
    needle: 'ctx.mark_committed()',
    expect: 'ctx.mark_committed',
  },
];

function applyMutation(base, m) {
  const cur = base[m.file];
  if (m.op === 'delete') {
    if (!cur.includes(m.needle)) return null;
    return { ...base, [m.file]: cur.split(m.needle).join('') };
  }
  return { ...base, [m.file]: `${cur}\n${m.line}\n` };
}

function selfTest() {
  const base = loadSources();
  const baseFailures = runChecks(base);
  if (baseFailures.length > 0) {
    console.error('self-test 前置失败：真实源码本应全绿，但门禁已红——');
    for (const failure of baseFailures) console.error(`- ${failure}`);
    process.exit(1);
  }
  let judged = 0;
  const bad = [];
  for (const m of MUTATIONS) {
    const mutated = applyMutation(base, m);
    if (!mutated) {
      bad.push(`${m.name}: 针已漂——needle \`${m.needle}\` 在 ${m.file} 中未出现`);
      continue;
    }
    const failures = runChecks(mutated);
    const bitten = failures.some((f) => f.includes(m.expect));
    if (!bitten) {
      bad.push(
        `${m.name}: 期望含 \`${m.expect}\` 变红，实际 ${failures.length ? failures.map((f) => f.slice(0, 40)).join(' | ') : 'GREEN(假绿!)'}`,
      );
      continue;
    }
    judged += 1;
  }
  if (bad.length > 0) {
    console.error(`self-test 失败（${bad.length} 条）：`);
    for (const line of bad) console.error(`- ${line}`);
    process.exit(1);
  }
  console.log(
    `check-simulated-never-commits self-test OK (${judged}/${MUTATIONS.length} 条变异各按预期把对应检查打红)`,
  );
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  const src = loadSources();
  finish('Simulated-Never-Commits', runChecks(src), Object.keys(src).length);
}
