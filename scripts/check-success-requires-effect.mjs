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
//
// 用法：
//   node scripts/check-success-requires-effect.mjs               # 校验
//   node scripts/check-success-requires-effect.mjs --self-test   # 变异自证（证明每条针有牙）

import { readFileSync } from 'node:fs';

import { collectFiles, stripped, blockAt, findBlock, finish } from './lib/gate-scan.mjs';

const MARKET = 'crates/tauron-market/src/api.rs';
const CLI = 'packages/tauron-cli/src/plugin-lifecycle.ts';
const LIB = 'crates/tauron-adapter/src/lib.rs';
// T-7 十七片之三：商城命令族（MarketCheckResult / MarketUpdateResult 结构、cmd_market_* 双胞胎腿
// 与其构造点）已从 lib.rs 逐字节纯搬移到 market.rs。下面 section 2 里针对这些**结构 / 腿 / 构造点**
// 的判据改读 market.rs（否则搬移后 lib 里找不到它们、检查会静默失明＝削弱门禁）。第 143 行的
// 违禁词扫描本就 collectFiles 自动覆盖 market.rs，无需改动。
const MARKET_ADAPTER = 'crates/tauron-adapter/src/market.rs';
const adapterRoots = ['crates/tauron-adapter/src'];

// `RevokeResponse` is the allowlisted case: `revoke()` mutates the crate's own
// revocation list, which is the authoritative structure `is_plugin_revoked()`
// reads back — that is a real in-scope effect, so `revoked_at` is its token.
const EFFECT_TOKENS = new Map([['RevokeResponse', 'revoked_at:']]);
const BANNED_ADAPTER = [
  'MarketplaceApi',
  'create_default_marketplace_api',
  'InstallRequest',
  'InstallResponse',
];
const stubResults = ['MarketCheckResult', 'MarketUpdateResult'];
const wiredLegNames = ['cmd_market_download_wired', 'cmd_market_install_wired'];

const toLines = (text) => text.split(/\r?\n/);

function loadSources() {
  const adapterPaths = collectFiles(adapterRoots, ['.rs']);
  const adapter = {};
  for (const p of adapterPaths) adapter[p] = readFileSync(p, 'utf8');
  return {
    market: readFileSync(MARKET, 'utf8'),
    cli: readFileSync(CLI, 'utf8'),
    adapter,
    adapterPaths,
  };
}

/// Every `pub struct <name> … { … }` block in the view.
function rustStructs(view, namePattern) {
  const out = [];
  for (let i = 0; i < view.cut.length; i += 1) {
    const match = view.cut[i].match(/^\s*pub struct\s+(\w+)\s*\{?/);
    if (!match || !namePattern.test(match[1])) continue;
    const block = blockAt(view.raw, i);
    if (!block) continue;
    out.push({
      name: match[1],
      index: i,
      body: view.cut.slice(block.start, block.end + 1).join('\n'),
    });
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
    out.push({ name, index: i, body: view.cut.slice(block.start, block.end + 1).join('\n') });
  }
  return out;
}

const proj = (file, text) => {
  const raw = toLines(text);
  return { raw, cut: stripped(raw), file };
};

/** 对给定源码快照跑全部不变量，返回 failures（纯函数，供校验与 self-test 共用）。 */
function runChecks(src) {
  const failures = [];

  // ── 1. tauron-market: every `*Response` that carries `success` must carry an
  //      effect marker (`simulated`) or a real effect-token field.
  const marketView = proj(MARKET, src.market);
  const marketResponses = rustStructs(marketView, /Response$/);
  if (marketResponses.length === 0) {
    failures.push(`${MARKET}: no \`pub struct *Response\` found — gate blind`);
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
        `${MARKET}:${struct.index + 1}: \`${struct.name}\` declares ` +
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
        `${MARKET}:${literal.index + 1}: \`${literal.name}\` is built by a ` +
          'mock client, so it must be honest: `simulated: true`',
      );
    }
    if (!info.hasSimulated && info.effectToken && !literal.body.includes(info.effectToken)) {
      failures.push(
        `${MARKET}:${literal.index + 1}: \`${literal.name}\` reports ` +
          `success and must carry its effect token \`${info.effectToken}\``,
      );
    }
  }

  // ── 2. tauron-adapter: the stub vocabulary is `ok` / `available` + `simulated`.
  for (const file of src.adapterPaths) {
    const view = proj(file, src.adapter[file]);
    view.cut.forEach((line, index) => {
      if (line.includes('success: true')) {
        failures.push(
          `${file}:${index + 1}: \`success: true\` in the adapter is an unbacked success claim — ` +
            'this crate reports `ok` / `available` plus `simulated`, never bare success',
        );
      }
      for (const banned of BANNED_ADAPTER) {
        if (line.includes(banned)) {
          failures.push(
            `${file}:${index + 1}: adapter references \`${banned}\` — the in-process mock ` +
              'marketplace API must never become a production consumer of `host_market_*`',
          );
        }
      }
    });
  }
  const adapterMarket = proj(MARKET_ADAPTER, src.adapter[MARKET_ADAPTER]);
  for (const struct of rustStructs(adapterMarket, /^(MarketCheckResult|MarketUpdateResult)$/)) {
    if (!struct.body.includes('pub simulated:')) {
      failures.push(
        `${MARKET_ADAPTER}:${struct.index + 1}: \`${struct.name}\` must declare ` +
          '`pub simulated: bool` so a stub result can never be read as an effect',
      );
    }
  }
  const wiredLegs = [];
  for (const name of wiredLegNames) {
    const block = findBlock(adapterMarket.raw, new RegExp(`^\\s*fn ${name}\\s*\\(`));
    if (block) wiredLegs.push(block);
  }
  const sliceLeg = (leg) => adapterMarket.cut.slice(leg.start, leg.end + 1).join('\n');
  for (const literal of rustLiterals(adapterMarket, stubResults)) {
    const leg = wiredLegs.find((b) => literal.index >= b.start && literal.index <= b.end);
    if (leg) {
      if (!/upgrade_installer\.(download|install)\(\)/.test(sliceLeg(leg))) {
        failures.push(
          `${MARKET_ADAPTER}:${literal.index + 1}: \`${literal.name}\` claims a ` +
            'real effect (`simulated: false`) inside a wired leg, but the function has no seam ' +
            'call (`upgrade_installer.download()` / `.install()`) — the claim needs a real effect ' +
            'in the same function',
        );
      }
      continue;
    }
    if (!literal.body.includes('simulated: true')) {
      failures.push(
        `${MARKET_ADAPTER}:${literal.index + 1}: \`${literal.name}\` is built by a ` +
          'stub that performs no network / filesystem work — it must say `simulated: true` ' +
          'and carry a `reason`',
      );
    }
  }

  // ── 3. tauron-cli: a `success: true` result must state its simulation flag.
  const cliView = proj(CLI, src.cli);
  cliView.cut.forEach((line, index) => {
    if (line.includes('simulated: false')) {
      failures.push(
        `${CLI}:${index + 1}: \`simulated: false\` claims a real effect; the CLI has no ` +
          'downloader / uploader / archive writer, so it must not assert one',
      );
    }
  });
  let successBlocks = 0;
  for (let i = 0; i < cliView.cut.length; i += 1) {
    if (!/\breturn\s*\{/.test(cliView.cut[i])) continue;
    const block = blockAt(cliView.raw, i);
    if (!block) continue;
    const body = cliView.cut.slice(block.start, block.end + 1).join('\n');
    if (!body.includes('success: true')) continue;
    successBlocks += 1;
    if (!body.includes('simulated: true')) {
      failures.push(
        `${CLI}:${i + 1}: this \`return { success: true }\` has no effect token — it must ` +
          'carry `simulated: true` (and describe what actually landed) in the same result',
      );
    }
  }
  if (successBlocks === 0) {
    failures.push(`${CLI}: no \`return { success: true }\` found — gate blind on the CLI path`);
  }

  return failures;
}

// ── self-test：证明每条针有牙（无唯一性要求）──────────────────────────────
// 两种变异原语：`delete` 删掉某必需串在目标文件里的**全部**出现，断言规则因此变红；
// `inject` 往目标文件追加一行被禁的形状，断言"必须缺席"的规则变红。任一 check 若在
// 重构中被整块丢掉，对应变异不会变红 → self-test 判红。
const MUTATIONS = [
  {
    name: '市场 Response 去掉 pub simulated: 声明',
    op: 'delete',
    file: 'market',
    needle: 'pub simulated:',
    expect: 'no effect marker',
  },
  {
    name: '市场 mock 构造点不再说 simulated: true',
    op: 'delete',
    file: 'market',
    needle: 'simulated: true',
    expect: 'must be honest',
  },
  {
    name: 'RevokeResponse 丢掉效果凭据字段 revoked_at:',
    op: 'delete',
    file: 'market',
    needle: 'revoked_at:',
    expect: 'effect token',
  },
  {
    name: '适配器里冒出裸 success: true（无凭据成功宣称）',
    op: 'inject',
    file: 'adapter',
    needle: LIB,
    line: 'pub fn _gate_probe() { let x = S { success: true }; }',
    expect: 'unbacked success',
  },
  {
    name: '适配器引用了进程内 mock MarketplaceApi',
    op: 'inject',
    file: 'adapter',
    needle: LIB,
    line: 'pub fn _gate_probe() { let _ = MarketplaceApi; }',
    expect: 'in-process mock',
  },
  {
    name: 'MarketCheckResult/UpdateResult 不再声明 pub simulated:',
    op: 'delete',
    file: 'adapterNeedle',
    needle: 'pub simulated:',
    expect: 'must declare',
  },
  {
    name: 'wired leg 去掉 seam 调用却仍称 simulated: false',
    op: 'delete',
    file: 'adapterNeedle',
    needle: 'upgrade_installer.download()',
    expect: 'no seam call',
  },
  {
    name: 'stub 构造点不再说 simulated: true',
    op: 'delete',
    file: 'adapterNeedle',
    needle: 'simulated: true',
    expect: 'stub that performs',
  },
  {
    name: 'CLI 断言 simulated: false（无上传器却称真实效果）',
    op: 'inject',
    file: 'cli',
    line: 'const _probe = { simulated: false };',
    expect: 'claims a real effect',
  },
  {
    name: 'CLI success: true 返回不再带 simulated: true',
    op: 'delete',
    file: 'cli',
    needle: 'simulated: true',
    expect: 'has no effect token',
  },
];

function applyMutation(base, mutation) {
  if (mutation.op === 'delete') {
    if (mutation.file === 'market')
      return { ...base, market: base.market.split(mutation.needle).join('') };
    if (mutation.file === 'cli') return { ...base, cli: base.cli.split(mutation.needle).join('') };
    if (mutation.file === 'adapterNeedle') {
      return {
        ...base,
        adapter: {
          ...base.adapter,
          [MARKET_ADAPTER]: base.adapter[MARKET_ADAPTER].split(mutation.needle).join(''),
        },
      };
    }
  }
  if (mutation.op === 'inject') {
    if (mutation.file === 'cli') return { ...base, cli: `${base.cli}\n${mutation.line}\n` };
    if (mutation.file === 'adapter') {
      const target = mutation.needle;
      return {
        ...base,
        adapter: { ...base.adapter, [target]: `${base.adapter[target]}\n${mutation.line}\n` },
      };
    }
  }
  return null;
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
  for (const mutation of MUTATIONS) {
    const mutated = applyMutation(base, mutation);
    if (!mutated) {
      bad.push(`${mutation.name}: 变异构造失败（脚本病，非门禁）`);
      continue;
    }
    if (mutation.op === 'delete' && !containsNeedle(mutated, base, mutation)) {
      bad.push(
        `${mutation.name}: 针已漂——needle \`${mutation.needle}\` 在目标里本就未出现，删除不产生任何变化`,
      );
      continue;
    }
    const failures = runChecks(mutated);
    const bitten = failures.some((f) => f.includes(mutation.expect));
    if (!bitten) {
      bad.push(
        `${mutation.name}: 期望含 \`${mutation.expect}\` 变红，实际 ${failures.length ? failures.map((f) => f.slice(0, 40)).join(' | ') : 'GREEN(假绿!)'}`,
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
    `check-success-requires-effect self-test OK (${judged}/${MUTATIONS.length} 条变异各按预期把对应检查打红)`,
  );
}

// delete 变异有效性：needle 至少出现在被删的那个作用域文件里（否则删除是空操作，
// 不能证明任何针）。
function containsNeedle(mutated, base, mutation) {
  const eq = (a, b) => a === b;
  if (mutation.file === 'market') return !eq(mutated.market, base.market);
  if (mutation.file === 'cli') return !eq(mutated.cli, base.cli);
  if (mutation.file === 'adapterNeedle')
    return !eq(mutated.adapter[MARKET_ADAPTER], base.adapter[MARKET_ADAPTER]);
  return true;
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  const src = loadSources();
  finish('Success-Requires-Effect', runChecks(src), src.adapterPaths.length + 3);
}
