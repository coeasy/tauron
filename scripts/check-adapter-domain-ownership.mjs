#!/usr/bin/env node
// V7 §10 第 9 条 / Batch 5 的「至少冻结 domain ownership」分支。
//
// **要钉住的失真**：adapter 的核心逻辑长期往 `lib.rs` 单文件里追加（本轮实测
// 17,596 行、顶层 `pub fn cmd_*` 136 个）。物理拆分是 Batch 5' 的活，这里不假装拆完了；
// 但「谁都能继续往同一个文件塞新逻辑」可以机器判定。台账把每个**顶层条目**登记成
// `{ 域, 归属文件集合, 数量高点 }`，两种形状都要登记：
//   - `impl` 块 → 以目标类型为键，数量 = 块内方法数（只数块体第一层的 `fn`）；
//   - 顶层 `fn`（自由函数）→ 以函数名首段（第一个 `_` 之前）为键，数量 = 函数个数。
//
// 规则：
//   R1 每个顶层条目都必须在台账里（未归类 = 0；impl 头解析失败也算红灯，不静默跳过）。
//   R2 一个键只能有一个域（重复登记 = 两个域声称拥有同一段逻辑）。
//   R3 实际归属文件集合必须与登记的**相等**（域已拆出去就不许多出一个落点）。
//   R4 每个键的数量**只减不增**（高点是基线；要加逻辑必须改台账，改台账在 PR 里看得见、要写理由）。
//   R5 每个文件的顶层条目总数各自封顶（换个新键名也不许让 god file 长高；把逻辑搬进
//      模块文件则两头同时下降）。
//
// **为什么数「条目」而不是「行」**：行数会被 rustfmt 的重排改动——一次无关换行就能让门禁
// 假红，或反过来把真实增长掩掉。轮 22/23 实测过两次「把排版当语义」的同类缺陷，这里只数
// **声明级**条目，与换行无关；`--self-test` 里专门有一条折行 `impl` 夹具钉住这点。
//
// 用法：
//   node scripts/check-adapter-domain-ownership.mjs               # 校验
//   node scripts/check-adapter-domain-ownership.mjs --record      # 按当前源码重录基线
//   node scripts/check-adapter-domain-ownership.mjs --self-test   # 变异自证（证明非空转）

import { existsSync, readFileSync, writeFileSync } from 'node:fs';

import { blockAt, collectFiles, finish, linesOf, stripped } from './lib/gate-scan.mjs';

const ROOT = process.cwd();
const SRC_DIR = 'crates/tauron-adapter/src';
const CONTRACT = 'contracts/adapter-domain-ownership.json';

const loadContract = () => JSON.parse(readFileSync(`${ROOT}/${CONTRACT}`, 'utf8'));

/** `impl<T: X> Foo<T>` / `impl std::fmt::Debug for Bar` → `Foo` / `Bar`。 */
function implTarget(header) {
  const head = header
    .replace(/^\s*impl\b/, '')
    .replace(/\{.*$/, '')
    .trim();
  const forIndex = head.lastIndexOf(' for ');
  const candidate = forIndex === -1 ? head : head.slice(forIndex + ' for '.length);
  const bare = candidate.replace(/<[^<>]*>/g, '').trim();
  const match = bare.match(/([A-Za-z_][A-Za-z0-9_]*)\s*$/);
  return match ? match[1] : null;
}

/** 顶层自由函数名（允许 `pub` / `pub(crate)` / `const` / `unsafe` 前缀）。 */
function fnName(line) {
  const match = line.match(
    /^(?:pub(?:\([^)]*\))?[\s]+)*(?:const[\s]+|unsafe[\s]+|async[\s]+)*fn[\s]+([A-Za-z_][A-Za-z0-9_]*)/,
  );
  return match ? match[1] : null;
}

/** 函数名首段 = 自由函数的登记键（`cmd_settings_set` → `cmd`）。 */
const fnGroup = (name) => name.split('_')[0];

/**
 * 数一个 `impl` 块里的方法声明：只认块体**第一层**（深度 1）的 `fn 名字(`。
 * 方法体内部的嵌套 `fn` / 闭包不算方法，嵌套 `impl` 也不算顶层域。
 */
function methodsIn(lines, block) {
  const projection = stripped(lines);
  const names = [];
  let depth = 0;
  for (let i = block.start; i <= block.end && i < projection.length; i += 1) {
    const line = projection[i];
    if (depth === 1) {
      const name = fnName(line.trimStart());
      if (name) names.push(name);
    }
    for (const ch of line) {
      if (ch === '{') depth += 1;
      else if (ch === '}') depth -= 1;
    }
  }
  return names;
}

/**
 * 解析一段源码里所有**顶层**（第 0 列）条目。
 *
 * rustfmt 会把长泛型头折行，`{` 可能在后一行，所以整段头拼起来再解析。只取首行会解析
 * 不出目标类型，而**静默跳过**正是轮 22/23 实测过的「集合悄悄变小」那一族缺陷——因此解析
 * 失败一律进 `unresolved`，由调用方变成红灯。
 */
function parseItems(lines) {
  const projection = stripped(lines);
  const items = [];
  const unresolved = [];
  for (let i = 0; i < projection.length; i += 1) {
    const line = projection[i];
    if (/^impl\b/.test(line)) {
      const block = blockAt(lines, i);
      if (!block) continue;
      const header = projection.slice(i, block.start + 1).join(' ');
      const target = implTarget(header);
      if (!target) unresolved.push(`${header.slice(0, 80)}（第 ${i + 1} 行）`);
      else items.push({ kind: 'impl', key: target, count: methodsIn(lines, block).length });
      i = block.end;
      continue;
    }
    const name = fnName(line);
    if (name) items.push({ kind: 'fn', key: fnGroup(name), count: 1 });
  }
  return { items, unresolved };
}

/** 观测：`kind:key` → { kind, key, owners（文件集合）, count 合计 } + 每文件条目数。 */
function observe() {
  const byKey = new Map();
  const perFile = new Map();
  const unresolved = [];
  const files = collectFiles([SRC_DIR], ['.rs']);
  for (const file of files) {
    const { items, unresolved: fileUnresolved } = parseItems(linesOf(file));
    for (const detail of fileUnresolved) unresolved.push(`${file}: ${detail}`);
    perFile.set(file, items.length);
    for (const item of items) {
      const mapKey = `${item.kind}:${item.key}`;
      const entry = byKey.get(mapKey) || { ...item, owners: new Set(), count: 0, blocks: 0 };
      entry.owners.add(file);
      entry.count += item.count;
      entry.blocks += 1;
      byKey.set(mapKey, entry);
    }
  }
  return { byKey, perFile, unresolved, fileCount: files.length };
}

/**
 * 键 → 域。台账里的 `domain` 是**人维护的语义标签**：`--record` 会保留已有条目的域，
 * 只有新键才落到这里的启发式默认值（`misc` 也一样受 R1/R4 约束，不是免检区）。
 */
function domainFor(kind, key) {
  if (kind === 'fn') {
    if (key === 'cmd') return 'command-handlers';
    if (key === 'host') return 'command-bridge';
    if (key === 'wire') return 'wire-contract';
    if (key === 'settings' || key === 'reconcile') return 'settings';
    if (key === 'recover' || key === 'quarantine' || key === 'parse') return 'recovery';
    if (key === 'wasm') return 'wasm-delivery';
    if (key === 'stale' || key === 'stage') return 'process-delivery';
    return 'misc';
  }
  if (key.endsWith('CallDelivery')) return 'delivery';
  if (key.endsWith('Sink') || key.endsWith('Reaper')) return 'platform-sinks';
  if (key === 'SubstrateState') return 'substrate-assembly';
  if (key === 'PluginRuntimeState') return 'runtime-facade';
  if (key === 'ProcRuntime' || key === 'RuntimeSpawnProfile') return 'process-runtime';
  if (key === 'AdapterConfig') return 'config';
  if (key === 'AssemblyToken' || key === 'AssemblyError') return 'assembly-token';
  if (key === 'Caller') return 'caller-identity';
  if (key === 'ContributesRegistry' || key === 'InstallCleanupStage') return 'install';
  if (key.includes('Endpoint') || key.includes('Updater')) return 'distribute';
  return 'misc';
}

function buildContract() {
  const { byKey, perFile, unresolved } = observe();
  if (unresolved.length > 0) {
    throw new Error(
      `有 ${unresolved.length} 个顶层条目解析不出来，先修门禁解析器再重录：\n- ${unresolved.join('\n- ')}`,
    );
  }
  // 保留旧台账里人工改过的域标签（只认 kind+key，其余字段一律按当前源码重算）。
  const previousDomains = new Map();
  if (existsSync(`${ROOT}/${CONTRACT}`)) {
    for (const entry of loadContract().entries)
      previousDomains.set(`${entry.kind}:${entry.key}`, entry.domain);
  }
  const entries = [...byKey.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([mapKey, info]) => ({
      kind: info.kind,
      key: info.key,
      domain: previousDomains.get(mapKey) ?? domainFor(info.kind, info.key),
      // 一个键的落点今天可能跨几个文件（`host` 组就是：lib.rs 与 tauri.rs 都有）。台账
      // 原样登记，规则因此是「观测集合 == 登记集合」：既不假装已拆干净，也不许新落点
      // 悄悄冒出来。
      owners: [...info.owners].sort(),
      maxCount: info.count,
    }));
  return {
    version: 1,
    note: 'V7 §10-9 adapter domain ownership 冻结。收紧靠直接编辑本文件；调高 maxCount / maxItems 必须在 PR 里写明理由（默认应当开新域/模块文件，而不是往 lib.rs 追加）。',
    // R5：每个文件的**顶层条目数**各自封顶。这条与 R4 合起来才是「别再往单文件塞核心
    // 逻辑」的可机器判定版本——R4 管住「同一个键不许长大」，R5 管住「换个新键名也不许
    // 让 god file 长高」；把逻辑从 lib.rs 搬进模块文件则两头同时下降。
    fileBudgets: [...perFile.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([file, items]) => ({ file, maxItems: items })),
    entries,
  };
}

function check() {
  const failures = [];
  const contract = loadContract();
  const { byKey, perFile, unresolved, fileCount } = observe();

  // R5：单文件顶层条目数封顶（god file 不许再长高）。
  const budgets = new Map((contract.fileBudgets || []).map((b) => [b.file, b.maxItems]));
  for (const [file, items] of perFile) {
    if (!budgets.has(file)) {
      failures.push(`${file}: 有 ${items} 个顶层条目但没登记文件预算——先加进 ${CONTRACT}`);
      continue;
    }
    const max = budgets.get(file);
    if (items > max) {
      failures.push(
        `${file}: 顶层条目 ${items} > 基线 ${max}。V7 Batch 5 的方向是把逻辑按域搬进模块文件，` +
          '不是继续让单文件长高；确需新增请把该域做成独立文件，或在台账写明理由后调高预算',
      );
    }
  }
  for (const file of budgets.keys()) {
    if (!perFile.has(file)) {
      failures.push(`${CONTRACT}: 登记的文件预算 ${file} 已不在 ${SRC_DIR}——台账漂移`);
    }
  }

  const declared = new Map();
  for (const entry of contract.entries) {
    const mapKey = `${entry.kind}:${entry.key}`;
    if (declared.has(mapKey)) {
      failures.push(
        `${CONTRACT}: \`${entry.kind} ${entry.key}\` 登记了多次（${declared.get(mapKey).domain} / ` +
          `${entry.domain}）——一个键只能有一个域`,
      );
    }
    declared.set(mapKey, entry);
  }
  for (const detail of unresolved) {
    failures.push(`${detail}：顶层条目解析失败，无法判定归属（不静默跳过）`);
  }

  for (const [mapKey, info] of byKey) {
    const entry = declared.get(mapKey);
    const observedOwners = [...info.owners].sort().join(', ');
    const unit = info.kind === 'impl' ? '方法数' : '函数数';
    if (!entry) {
      failures.push(
        `${observedOwners}: ${info.kind === 'impl' ? 'impl' : 'fn'} \`${info.key}\`` +
          `（${info.blocks} 个条目 / ${info.count} 个${unit}）未登记域归属——` +
          `先加进 ${CONTRACT}，别继续往单文件里塞`,
      );
      continue;
    }
    const declaredOwners = [...entry.owners].sort().join(', ');
    if (observedOwners !== declaredOwners) {
      failures.push(
        `${observedOwners}: \`${info.key}\`（域 ${entry.domain}）的实际归属是 [${observedOwners}]，` +
          `台账登记的是 [${declaredOwners}]——同一段逻辑多一个落点就是双 mirror site；` +
          '要改结构请把台账一起改',
      );
    }
    for (const owner of entry.owners) {
      if (!existsSync(`${ROOT}/${owner}`)) {
        failures.push(`${CONTRACT}: \`${info.key}\` 的归属文件 ${owner} 不存在`);
      }
    }
    if (info.count > entry.maxCount) {
      failures.push(
        `${observedOwners}: \`${info.key}\`（域 ${entry.domain}）${unit} ${info.count} > 基线 ` +
          `${entry.maxCount}。往单文件追加核心逻辑正是本台账要拦的事；确需新增请开` +
          '新域/模块文件，或在台账写明理由后调高基线',
      );
    }
  }

  for (const entry of contract.entries) {
    if (!byKey.has(`${entry.kind}:${entry.key}`)) {
      failures.push(
        `${CONTRACT}: 登记的 \`${entry.kind} ${entry.key}\` 已不在 ${SRC_DIR}——台账漂移，删掉或改名`,
      );
    }
  }

  if (failures.length > 0) finish('Adapter-Domain-Ownership', failures, fileCount);

  const implEntries = contract.entries.filter((e) => e.kind === 'impl');
  const fnEntries = contract.entries.filter((e) => e.kind === 'fn');
  const libBudget = (contract.fileBudgets || []).find((b) => b.file.endsWith('/lib.rs'));
  console.log(
    `Adapter-Domain-Ownership gate OK across ${fileCount} source files ` +
      `(${implEntries.length} 个 impl 类型 / ${implEntries.reduce((n, e) => n + e.maxCount, 0)} 个方法、` +
      `${fnEntries.length} 组顶层 fn / ${fnEntries.reduce((n, e) => n + e.maxCount, 0)} 个函数、` +
      `${new Set(contract.entries.map((e) => e.domain)).size} 个域、` +
      `lib.rs 顶层条目 ${libBudget ? libBudget.maxItems : '?'} 已封顶；数量全部 ≤ 基线)`,
  );
}

/**
 * 变异自证：门禁必须**真的会红**。夹具各打一条规则，且包含一条折行 `impl` 头
 * （排版不得改变判定）。任何一条判反都说明规则对该形状空转。
 */
function selfTest() {
  const contract = loadContract();
  // 取**方法/函数最多的**已登记键做夹具基准：基线大于 1 才能构造出「现状通过 / 超量变红」
  // 这对可判定的形状。
  const biggest = (kind) =>
    contract.entries
      .filter((e) => e.kind === kind && e.maxCount > 0)
      .sort((a, b) => b.maxCount - a.maxCount)[0];
  const knownImpl = biggest('impl');
  const knownFn = biggest('fn');
  if (!knownImpl || !knownFn) {
    console.error('self-test 需要台账里同时存在 impl 与 fn 两类条目');
    process.exit(1);
  }

  const fixtures = [
    {
      name: 'R1 未登记 impl 类型',
      source: 'impl BrandNewType {\n  pub fn extra(&self) {}\n}\n',
      expectFail: true,
    },
    {
      name: 'R1 未登记 fn 组',
      source: 'pub fn brandnew_thing() {}\n',
      expectFail: true,
    },
    {
      name: 'R4 impl 方法超基线',
      source: `impl ${knownImpl.key} {\n${'  pub fn m(&self) {}\n'.repeat(knownImpl.maxCount + 1)}}\n`,
      expectFail: true,
    },
    {
      name: 'R4 fn 组数量超基线',
      source: `pub fn ${knownFn.key}_a() {}\n`.repeat(knownFn.maxCount + 1),
      expectFail: true,
    },
    {
      name: '现状形状（已登记键、数量未超）',
      source: `impl ${knownImpl.key} {\n  pub fn m(&self) {}\n}\npub fn ${knownFn.key}_one() {}\n`,
      expectFail: false,
    },
    {
      name: '折行排版不得改变判定',
      source: `impl<T: Clone + Default>\n    ${knownImpl.key}<T>\n{\n  pub fn m(&self) {}\n}\n`,
      expectFail: false,
      expectParsed: 1,
    },
  ];

  let judged = 0;
  for (const fixture of fixtures) {
    const lines = fixture.source.split('\n');
    const { items, unresolved } = parseItems(lines);
    if (unresolved.length > 0) {
      console.error(`self-test ${fixture.name}: 条目解析失败 → ${unresolved[0]}`);
      process.exit(1);
    }
    if (fixture.expectParsed !== undefined && items.length !== fixture.expectParsed) {
      console.error(
        `self-test ${fixture.name}: 夹具应有 ${fixture.expectParsed} 个条目，实际 ${items.length}（解析器漏了）`,
      );
      process.exit(1);
    }
    // 与 check() 同构：先按 `kind:key` **聚合**再判数量高点——否则 137 个 `cmd_*`
    // 会被看成 137 条各自合格的条目，R4 对该形状就是空转。
    const totals = new Map();
    for (const item of items) {
      const mapKey = `${item.kind}:${item.key}`;
      totals.set(mapKey, (totals.get(mapKey) || 0) + item.count);
    }
    let failed = false;
    for (const [mapKey, count] of totals) {
      const [kind, key] = mapKey.split(':');
      const entry = contract.entries.find((e) => e.kind === kind && e.key === key);
      if (!entry || count > entry.maxCount) failed = true;
    }
    judged += 1;
    if (failed !== fixture.expectFail) {
      console.error(
        `self-test ${fixture.name}: 期望${fixture.expectFail ? '红' : '绿'}，实际` +
          `${failed ? '红' : '绿'}——规则对该形状空转`,
      );
      process.exit(1);
    }
  }
  console.log(`Adapter-Domain-Ownership self-test OK (${judged} 个变异形状各按规则判定)`);
}

const argv = process.argv.slice(2);
if (argv.includes('--record')) {
  const contract = buildContract();
  writeFileSync(`${ROOT}/${CONTRACT}`, `${JSON.stringify(contract, null, 2)}\n`);
  console.log(`${CONTRACT} 已按当前源码重写（${contract.entries.length} 个登记键）`);
  process.exit(0);
}
if (argv.includes('--self-test')) {
  selfTest();
  process.exit(0);
}
check();
