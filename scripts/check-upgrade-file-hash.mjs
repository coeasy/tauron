#!/usr/bin/env node
// V7-P1 / V7 §9 `upgrade_file_hash`: an upgrade only counts when file hashes,
// the manifest and the state journal agree — before and after.
//
// `crates/tauron-distribute/src/upgrade.rs` is the execution side that earlier
// rounds landed. This gate keeps its four load-bearing properties from rotting
// (round 40 note: the hash → manifest → verifier chain was extracted into the
// shared `download_bounded` / `verify_package` free functions so the adapter
// download leg calls the *same* code; the gate now pins the call sites in
// `run_phases` and the implementation inside those shared functions):
//   1. The downloaded package is hashed and compared with the manifest before
//      anything is touched (`sha256_file` → `PackageHashMismatch`), the injected
//      signature verifier runs after that, and the accepted hash is written to
//      the journal. `run_phases` must reach this through `verify_package` — a
//      re-inlined local copy is a second implementation, not a fix.
//   2. Nothing is swapped in until the backup tree hash set equals the source
//      tree, and the swap happens after the hash gate, never before.
//   3. `success: true` exists in exactly one place, after `mark_committed`, and
//      `mark_committed` refuses to proceed unless the commit marker survives an
//      on-disk reload.
//   4. Rollback restores the previous tree **and** re-hashes it against the
//      pre-upgrade source tree; an unverified restore is reported as a failure.
//
// 用法：
//   node scripts/check-upgrade-file-hash.mjs               # 校验
//   node scripts/check-upgrade-file-hash.mjs --self-test   # 变异自证（证明每条针有牙）

import { readFileSync } from 'node:fs';

import { findBlock, finish } from './lib/gate-scan.mjs';

const file = 'crates/tauron-distribute/src/upgrade.rs';

/** 载入本门禁读到的源文件；self-test 用它做「原始 → 变异」的内存快照。 */
function loadSources() {
  return { upgrade: readFileSync(file, 'utf8') };
}

/** 对给定源码快照跑全部不变量，返回 failures（纯函数，供校验与 self-test 共用）。 */
function runChecks(src) {
  const failures = [];
  const lines = src.upgrade.split(/\r?\n/);

  function requireBlock(headerPattern, label) {
    const block = findBlock(lines, headerPattern);
    if (!block) {
      failures.push(`${file}: ${label} not found — the gate cannot verify its invariants`);
      return null;
    }
    return block;
  }

  function order(block, label, sequence) {
    const body = block.body.join('\n');
    let previous = -1;
    let previousName = '';
    for (const [name, needle] of sequence) {
      const at = body.indexOf(needle);
      if (at === -1) {
        failures.push(`${file}:${block.headerIndex + 1}: ${label} must ${name} (\`${needle}\`)`);
        continue;
      }
      if (previous !== -1 && at < previous) {
        failures.push(
          `${file}:${block.headerIndex + 1}: ${label} must ${name} **before** ${previousName} ` +
            `(out-of-order \`${needle}\`)`,
        );
      }
      previous = at;
      previousName = name;
    }
    return body;
  }

  // 1 + 2 + 3. The phase chain.
  const phases = requireBlock(/^\s*(pub )?fn run_phases\s*\(/, '`UpgradeRunner::run_phases`');
  let phasesBlock = null;
  if (phases) {
    phasesBlock = order(phases, 'run_phases', [
      ['download through the shared bounded downloader', 'self.download_bounded('],
      ['run the shared package gate (SHA-256 → manifest → verifier)', 'verify_package('],
      ['record the accepted package hash in the journal', 'journal.package_sha256 = Some('],
      ['hash the installed tree before touching it', 'tree_hashes(&current_dir)'],
      ['swap the staged tree in', 'fs::rename(&current_dir, &previous_dir)'],
      ['re-hash the live tree and compare it with the staged tree', 'live_hashes != staged_hashes'],
      ['commit through the journal', 'ctx.mark_committed()'],
      ['only then report success', 'success: true'],
    ]);
    const extractAt = phasesBlock.indexOf('extractor.extract(');
    const gateAt = phasesBlock.indexOf('verify_package(');
    const swapAt = phasesBlock.indexOf('fs::rename(&current_dir, &previous_dir)');
    if (extractAt !== -1 && gateAt !== -1 && gateAt > extractAt) {
      failures.push(
        `${file}:${phases.headerIndex + 1}: the package hash gate must run before extraction, ` +
          'otherwise unverified bytes are unpacked into staging',
      );
    }
    if (swapAt !== -1 && gateAt !== -1 && gateAt > swapAt) {
      failures.push(
        `${file}:${phases.headerIndex + 1}: the live install tree was swapped before the package ` +
          'hash gate ran',
      );
    }
    if (phasesBlock.includes('sha256_file(')) {
      failures.push(
        `${file}:${phases.headerIndex + 1}: run_phases re-inlined \`sha256_file(\` — the package ` +
          'gate must stay the shared `verify_package` implementation (the adapter download leg ' +
          'calls the same one; two copies drift)',
      );
    }
    if (phasesBlock.includes('verifier.verify(')) {
      failures.push(
        `${file}:${phases.headerIndex + 1}: run_phases calls \`verifier.verify(\` directly — the ` +
          'signature check belongs to the shared `verify_package` gate (full-package hash first)',
      );
    }
  }

  // 1 (implementation). The shared gate itself owns the hash → compare → verify
  // chain; nothing above may bypass it.
  const verify = requireBlock(/^\s*pub fn verify_package\s*\(/, '`verify_package`');
  if (verify) {
    order(verify, 'verify_package', [
      ['hash the downloaded package', 'sha256_file('],
      ['reject a hash mismatch as a hard error', 'PackageHashMismatch'],
      ['verify the signature afterwards', 'verifier.verify('],
    ]);
  }

  // 3. The commit gate itself: write, reload, refuse on a missing marker.
  const commit = requireBlock(/^\s*fn mark_committed\s*\(/, '`OpContext::mark_committed`');
  if (commit) {
    const body = order(commit, 'mark_committed', [
      ['set the commit marker', 'commit_marker = true'],
      ['persist the journal', 'journal.save('],
      ['re-read the journal from disk', 'UpgradeJournal::load('],
    ]);
    if (!/if\s*!\s*reloaded\.commit_marker/.test(body)) {
      failures.push(
        `${file}:${commit.headerIndex + 1}: mark_committed must refuse to succeed when the ` +
          're-read journal lacks the commit marker',
      );
    }
  }

  // 4. Rollback restores AND verifies.
  const rollback = requireBlock(
    /^\s*fn rollback_after_swap\s*\(/,
    '`UpgradeRunner::rollback_after_swap`',
  );
  if (rollback) {
    const body = order(rollback, 'rollback_after_swap', [
      ['quarantine the broken new tree', 'fs::rename(&current_dir, &quarantine)'],
      ['move the previous tree back', 'fs::rename(&previous_dir, &current_dir)'],
      ['re-hash the restored tree', 'tree_hashes(&current_dir)'],
    ]);
    if (!body.includes('source_hashes')) {
      failures.push(
        `${file}:${rollback.headerIndex + 1}: rollback must compare the restored tree with the ` +
          'pre-upgrade source hashes',
      );
    }
  }

  // Journal persistence on every state transition.
  const record = requireBlock(
    /^\s*fn record\(&mut self, state: UpgradeState\)/,
    '`OpContext::record`',
  );
  if (record && !record.body.join('\n').includes('journal.save(')) {
    failures.push(
      `${file}:${record.headerIndex + 1}: every recorded state must be persisted via journal.save(`,
    );
  }

  // 3 (again, file-wide): exactly one success-shaped result, inside run_phases.
  const successLines = lines
    .map((line, index) => ({ line, index }))
    .filter(({ line }) => line.includes('success: true'));
  for (const { index } of successLines) {
    if (!phases || index < phases.start || index > phases.end) {
      failures.push(
        `${file}:${index + 1}: \`success: true\` outside \`run_phases\` is an unbacked success ` +
          'claim — the runner must be the only place that reports an upgrade as done',
      );
    }
  }

  return failures;
}

// ── self-test：对真实源码做内存变异，证明每条针都会红（非空转）────────────────
// 每个 requireBlock 目标 + 重载守卫正则 + 全文件 success 扫描都至少有一条变异覆盖：
// 一旦重构把某个 check 整个丢掉，对应变异不会变红 → self-test 判红。
const MUTATIONS = [
  {
    name: 'run_phases 不再比对 live 与 staged 树哈希',
    from: 'live_hashes != staged_hashes',
    to: 'live_hashes == staged_hashes',
    expect: 'run_phases',
  },
  {
    name: 'run_phases 不再经 journal 提交',
    from: 'ctx.mark_committed()',
    to: 'ctx.forget_commit()',
    expect: 'run_phases',
  },
  {
    name: 'run_phases 绕过共享有界下载器',
    from: 'self.download_bounded(',
    to: 'self.bypass_download(',
    expect: 'run_phases',
  },
  {
    name: 'run_phases 记录包哈希时不落 journal（Some→None）',
    from: 'journal.package_sha256 = Some(',
    to: 'journal.package_sha256 = None',
    expect: 'run_phases',
  },
  {
    name: 'verify_package 不再调签名校验器',
    from: 'verifier.verify(',
    to: 'verifier.skip(',
    expect: 'verify_package',
  },
  {
    name: 'mark_committed 不落提交标记',
    from: 'commit_marker = true',
    to: 'commit_marker = false',
    expect: 'mark_committed',
  },
  {
    name: 'mark_committed 的重载守卫不再据 reloaded.commit_marker 拒绝',
    from: 'reloaded.commit_marker',
    to: 'stored.commit_marker',
    expect: 'mark_committed',
  },
  {
    name: 'rollback_after_swap 实现整块消失（改名令 header 不匹配）',
    from: 'fn rollback_after_swap',
    to: 'fn rollback_after_swap_retired',
    expect: 'rollback_after_swap',
  },
  {
    name: 'record 签名变了，门禁找不到每步落盘的 record',
    from: 'fn record(&mut self, state: UpgradeState)',
    to: 'fn record(&mut self, state: String)',
    expect: 'OpContext::record',
  },
  {
    name: 'run_phases 之外出现 success: true（无凭据的成功宣称）',
    from: 'commit_marker = true',
    to: 'commit_marker = true; let report = Report { success: true };',
    expect: 'unbacked success',
  },
];

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
    // 每个变异都在原始文本副本上做，互不影响。
    const original = base.upgrade;
    const occurrences = original.split(mutation.from).length - 1;
    if (occurrences !== 1) {
      bad.push(`${mutation.name}: 针已漂——from 串在 upgrade.rs 出现 ${occurrences} 次（期望 1）`);
      continue;
    }
    const mutated = { upgrade: original.replace(mutation.from, mutation.to) };
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
    `check-upgrade-file-hash self-test OK (${judged}/${MUTATIONS.length} 条变异各按预期把对应检查打红)`,
  );
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  finish('Upgrade-File-Hash', runChecks(loadSources()), 1);
}
