#!/usr/bin/env node
// 轮 67 / V9 N-03：升级 journal「启动恢复腿」的机械门禁。
//
// **背景（被钉的失真）**：V7 Batch 3 要求「update operation 持久化并可在重启后
// resume/recover」，但写侧（`UpgradeRunner` 每步落 journal）与读侧长期不对称——
// 读回 journal 的只有测试，没有任何生产启动路径在 boot 时对账（N-03 判据原文：
// 「journal 只被测试读过」）。轮 67 补了 `tauron-distribute::recovery`（对账实现）
// 与适配层 `upgrade_recovery`（boot 消费点）。本门禁不重言 Rust 已测的行为，只钉
// 四条 Rust 用例**证不了**的结构性不变量——它们一旦漂，恢复腿就又变成「假装有」：
//
//   C1 **单一恢复实现**：`restore_backup_tree` 全 crate 只定义一次，且恢复腿是
//      **复用**它而不是抄第二份字节一致性判定（第二份 = 两条路径可对同一棵树给相反结论）。
//   C2 **boot 读取有非测试消费者**：`UpgradeReconciler::scan` 被适配层 boot 调用，
//      且该调用点在 `#[cfg(test)]` **之前**（否则「只被测试读过」的断链原地复活）。
//   C3 **撕裂日志 fail-closed 隔离**：解析失败的 journal 走 `quarantine_corrupt`
//      改名成 `.corrupt-` 而不是被当作干净态继续（坏数据不盲动、不据此改安装树）。
//   C4 **恢复事实并进既有读数、缺省不改形状**：`cmd_updater_status` 只在
//      `has_pending()` 为真时并入纯文本；缺省宿主（报告 `None`）逐字不碰 `reason`。
//
// 用法：
//   node scripts/check-upgrade-resume.mjs               # 校验
//   node scripts/check-upgrade-resume.mjs --self-test   # 变异自证（证明每条针有牙）

import { readFileSync } from 'node:fs';

import { collectFiles } from './lib/gate-scan.mjs';

const RECOVERY = 'crates/tauron-distribute/src/recovery.rs';
const UPGRADE = 'crates/tauron-distribute/src/upgrade.rs';
const ADAPTER_RECOVERY = 'crates/tauron-adapter/src/upgrade_recovery.rs';
const ADAPTER_LIB = 'crates/tauron-adapter/src/lib.rs';

const read = (file) => readFileSync(file, 'utf8');

/** 载入本门禁读到的全部源文件；self-test 用它做「原始 → 变异」的内存快照。 */
function loadSources() {
  const distributeFiles = collectFiles(['crates/tauron-distribute/src'], ['.rs']);
  const src = {
    recovery: read(RECOVERY),
    upgrade: read(UPGRADE),
    adapterRecovery: read(ADAPTER_RECOVERY),
    adapterLib: read(ADAPTER_LIB),
    distributeDefs: distributeFiles
      .filter((f) => read(f).match(/^\s*(?:pub(?:\([^)]*\))?\s+)?fn\s+restore_backup_tree\b/m))
      .map((f) => f.replace(/\\/g, '/')),
  };
  return src;
}

function countOccurrences(text, re) {
  const flags = re.flags.includes('g') ? re.flags : `${re.flags}g`;
  return [...text.matchAll(new RegExp(re.source, flags))].length;
}

function checkC1(src, failures) {
  const defs = src.distributeDefs;
  if (defs.length !== 1 || defs[0] !== UPGRADE) {
    failures.push(
      `C1 restore_backup_tree 必须全 crate 只定义一次且落在 ${UPGRADE}；实测定义处 [${defs.join(', ')}]（多一处就是第二套字节一致性判定）`,
    );
  }
  if (!/restore_backup_tree\s*\(/.test(src.recovery)) {
    failures.push(
      'C1 恢复腿（recovery.rs）必须**调用**共享的 restore_backup_tree，而不是自带回滚逻辑',
    );
  }
  if (/fn\s+restore_backup_tree\b/.test(src.recovery)) {
    failures.push(
      'C1 recovery.rs 里出现了第二份 restore_backup_tree 定义——恢复必须复用 upgrade.rs 那一份',
    );
  }
}

function checkC2(src, failures) {
  if (!/UpgradeReconciler::new\s*\(/.test(src.adapterRecovery)) {
    failures.push('C2 适配层 boot 消费者必须构造 UpgradeReconciler（缺失 = 恢复实现没人调用）');
  }
  if (!/\.scan\s*\(/.test(src.adapterRecovery)) {
    failures.push('C2 适配层 boot 消费者必须调用 UpgradeReconciler::scan 读回 journal');
  }
  // 只认**装配调用点** `.map(upgrade_recovery::scan_upgrade_recovery)`，不认裸 token：
  // 同名的 rustdoc 链接 `[`upgrade_recovery::scan_upgrade_recovery`]` 也在 lib.rs 里且也在
  // cfg(test) 之前，裸 token 针会被它喂饱（假绿）。装配调用点带 `.map(` 前缀，文档喂不出。
  const mapRe = /\.map\(\s*upgrade_recovery::scan_upgrade_recovery\s*\)/;
  if (!mapRe.test(src.adapterLib)) {
    failures.push(
      'C2 lib.rs 未在 boot 装配里 map(upgrade_recovery::scan_upgrade_recovery)——boot 装配点断了',
    );
    return;
  }
  const callIdx = src.adapterLib.search(mapRe);
  const testMod = src.adapterLib.search(/^\s*#\[cfg\(test\)\]/m);
  if (testMod !== -1 && callIdx > testMod) {
    failures.push(
      'C2 boot 消费点落在 #[cfg(test)] 之后——「journal 只被测试读过」的 N-03 断链会原地复活',
    );
  }
}

function checkC3(src, failures) {
  if (!/fn\s+quarantine_corrupt\b/.test(src.recovery)) {
    failures.push('C3 恢复腿缺 quarantine_corrupt：撕裂 journal 无 fail-closed 隔离路径');
  }
  // 钉**代码里**的隔离改名 `format!("{name}.corrupt-{}", …)` 的 `.corrupt-{}` 字面：
  // 文档/测试里的 `.corrupt-<ts>` 与 `.corrupt-")` 都不是这个形状，喂不饱这枚针。
  if (!/\.corrupt-\{\}/.test(src.recovery)) {
    failures.push('C3 撕裂 journal 必须把文件名改名隔离为 .corrupt-{}（而不是被删/被当干净态）');
  }
  // classify 的 Err 分支必须真的调 quarantine_corrupt（否则函数写了却没人用）。
  if (!/quarantine_corrupt\(&journal_path\)/.test(src.recovery)) {
    failures.push('C3 recovery.rs 的解析失败分支没有实际调用 quarantine_corrupt');
  }
}

function checkC4(src, failures) {
  const body = src.adapterLib.slice(
    src.adapterLib.indexOf('pub fn cmd_updater_status'),
    src.adapterLib.indexOf('pub fn cmd_updater_status') === -1
      ? -1
      : src.adapterLib.indexOf('pub fn cmd_updater_status') + 6000,
  );
  if (body.length === 0) {
    failures.push('C4 找不到 cmd_updater_status 函数体');
    return;
  }
  if (!/\.has_pending\(\)/.test(body)) {
    failures.push(
      'C4 cmd_updater_status 必须用 has_pending() 作门控（无条件并入会把干净历史也标成挂起）',
    );
  }
  if (!/上次升级有未收尾操作：未提交交换/.test(body)) {
    failures.push('C4 cmd_updater_status 缺恢复读数文案（宿主拿不到「上次升级没收尾」的事实）');
  }
  if (!/upgrade_recovery\.lock\(\)\.as_ref\(\)/.test(body)) {
    failures.push(
      'C4 cmd_updater_status 未经 upgrade_recovery.lock().as_ref() 读取——缺省宿主（None）不再逐字保持 reason 不变',
    );
  }
}

function runChecks(src) {
  const failures = [];
  checkC1(src, failures);
  checkC2(src, failures);
  checkC3(src, failures);
  checkC4(src, failures);
  return failures;
}

// ── self-test：对真实源码做内存变异，证明每条针都会红（非空转）────────────────
const MUTATIONS = [
  {
    name: 'C1a 恢复腿自带第二份 restore_backup_tree 定义',
    file: 'recovery',
    from: 'fn now_ms() -> u64 {',
    to: 'fn restore_backup_tree() {}\nfn now_ms() -> u64 {',
    expect: 'C1',
  },
  {
    name: 'C1b 恢复腿不再调用共享 restore_backup_tree',
    file: 'recovery',
    from: 'restore_backup_tree(&self.paths.install_dir, &backup_tree, operation_id)?;',
    to: 'let _ = (&backup_tree, operation_id);',
    expect: 'C1',
  },
  {
    name: 'C2a 适配层 boot 不再调用 scan',
    file: 'adapterRecovery',
    from: 'reconciler.scan().unwrap_or_else(|error| {',
    to: 'let _ = &reconciler; ReconcileReport::default().unwrap_or_else(|error| {',
    expect: 'C2',
  },
  {
    name: 'C2b lib.rs 删掉 boot 装配点',
    file: 'adapterLib',
    from: '.map(upgrade_recovery::scan_upgrade_recovery),',
    to: '.map(|_| Default::default()),',
    expect: 'C2',
  },
  {
    name: 'C3a 撕裂隔离不再改名成 .corrupt-',
    file: 'recovery',
    from: 'let quarantined = path.with_file_name(format!("{name}.corrupt-{}", now_ms()));',
    to: 'let quarantined = path.with_file_name(format!("{name}.kept-{}", now_ms()));',
    expect: 'C3',
  },
  {
    name: 'C3b 解析失败分支不再调用 quarantine_corrupt',
    file: 'recovery',
    from: 'let quarantined = quarantine_corrupt(&journal_path);',
    to: 'let quarantined: Option<std::path::PathBuf> = None;',
    expect: 'C3',
  },
  {
    name: 'C4a cmd_updater_status 去掉 has_pending 门控',
    file: 'adapterLib',
    from: 'if report.has_pending() {',
    to: 'if true {',
    expect: 'C4',
  },
  {
    name: 'C4b 删掉恢复读数文案',
    file: 'adapterLib',
    from: '上次升级有未收尾操作：未提交交换 {}、staging 残骸 {}、撕裂日志已隔离 {}',
    to: '（读数已隐藏）',
    expect: 'C4',
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
    const mutated = { ...base, distributeDefs: [...base.distributeDefs] };
    if (!mutated.hasOwnProperty(mutation.file) || typeof mutated[mutation.file] !== 'string') {
      bad.push(`${mutation.name}: 变异目标文件键不存在（脚本病，非门禁）`);
      continue;
    }
    const original = mutated[mutation.file];
    const occurrences = original.split(mutation.from).length - 1;
    if (occurrences !== 1) {
      bad.push(
        `${mutation.name}: 针已漂——from 串在 ${mutation.file} 出现 ${occurrences} 次（期望 1）`,
      );
      continue;
    }
    mutated[mutation.file] = original.replace(mutation.from, mutation.to);
    const failures = runChecks(mutated);
    const bitten = failures.some((f) => f.startsWith(mutation.expect));
    if (!bitten) {
      bad.push(
        `${mutation.name}: 期望 ${mutation.expect} 变红，实际 ${failures.length ? failures.map((f) => f.slice(0, 3)) : 'GREEN(假绿!)'}`,
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
    `check-upgrade-resume self-test OK (${judged}/${MUTATIONS.length} 条变异各按预期把对应检查打红)`,
  );
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  const failures = runChecks(loadSources());
  if (failures.length > 0) {
    console.error('check-upgrade-resume gate failed:');
    for (const failure of failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log(
    'check-upgrade-resume gate OK: 单一恢复实现 + boot 非测试消费者 + 撕裂 fail-closed 隔离 + 读数门控并入/缺省不改形状',
  );
}
