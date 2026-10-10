#!/usr/bin/env node
// R-5 货架规则门禁（T-5 落地）：面向消费者的"广告位"必须只包含真实可上架的包。
//
// 本仓 module-maturity.json 已明确拒绝手写 Stable/Preview/Experimental 分级——"成熟度
// 分级需要 conformance 证据背书，属 V5 Phase A 退出门槛，手写等级字段只会复制谎报模式"。
// 因此 T-5 不引入新的手写字段，只做**三条纯机械反推**：
//
//   · Inv-Sync   四段观察产出（README 的 `pnpm add` 行、create-tauron-app#dependencies、
//                所有 packages/*/package.json 的 private:true 集合、module-maturity 的
//                reference-only 集合）必须与源码逐条相等；台账只是复算结果、手写即红。
//   · Inv-B      private:true 的包不得出现在广告位——防止把纯仓内工具（@tauron/contract-tests）
//                当 SDK 卖给外部消费者。
//   · Inv-C      module-maturity `consumerStatus: "reference-only"` 的包不得进入脚手架
//                create-tauron-app 的 dependencies——脚手架不能拉一个"仅参考"骨架进项目起手式。
//
// 外加一条收录完整性（Inv-Known）：广告位里出现的 npm 名必须在 module-maturity packages
// 已知集合内——否则等于卖一个没有归属档案的包。
//
// 每条判据都配 --self-test 变异夹具，破坏不变量→对应规则必须转红，红不出来就是回归。
//
// 用法：
//   node scripts/check-shelf-rules.mjs                # 默认 --check
//   node scripts/check-shelf-rules.mjs --check
//   node scripts/check-shelf-rules.mjs --write        # 复算 + 落盘 ledger
//   node scripts/check-shelf-rules.mjs --self-test    # 7 条变异夹具

import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('..', import.meta.url));
const LEDGER_REL = 'contracts/shelf-advertising.json';
const README_REL = 'README.md';
const SCAFFOLD_REL = 'packages/create-tauron-app/package.json';
const MATURITY_REL = 'contracts/module-maturity.json';

function read(rel) {
  return readFileSync(join(ROOT, rel), 'utf8');
}

function listPackageManifests() {
  const dir = join(ROOT, 'packages');
  const out = [];
  for (const name of readdirSync(dir)) {
    const manifest = join(dir, name, 'package.json');
    if (existsSync(manifest) && statSync(manifest).isFile())
      out.push(`packages/${name}/package.json`);
  }
  return out.sort();
}

function extractReadmePnpmAdd(text) {
  const lines = text.split(/\r?\n/);
  const entries = [];
  for (let i = 0; i < lines.length; i += 1) {
    const m = /^\s*(?:\$\s+)?pnpm\s+add\s+(.+)$/u.exec(lines[i]);
    if (!m) continue;
    const tokens = m[1]
      .split(/\s+/)
      .filter((t) => /^(?:@tauron\/|tauron$|create-tauron-app$)/u.test(t));
    for (const t of tokens) entries.push({ package: t, line: i + 1 });
  }
  return entries;
}

function extractScaffoldDeps(text) {
  const pkg = JSON.parse(text);
  const deps = pkg.dependencies ?? {};
  return Object.keys(deps)
    .filter(
      (name) => name.startsWith('@tauron/') || name === 'tauron' || name === 'create-tauron-app',
    )
    .map((name) => ({ package: name, manifest: SCAFFOLD_REL }))
    .sort((a, b) => a.package.localeCompare(b.package));
}

function extractPrivatePackages(manifests, packageTexts) {
  const out = [];
  for (const rel of manifests) {
    try {
      const pkg = JSON.parse(packageTexts[rel]);
      if (pkg.private === true && typeof pkg.name === 'string') {
        out.push({ package: pkg.name, manifest: rel });
      }
    } catch {
      /* 忽略解析失败的 manifest——由其他 gate 负责 */
    }
  }
  return out.sort((a, b) => a.package.localeCompare(b.package));
}

function extractReferenceOnly(maturityText) {
  const maturity = JSON.parse(maturityText);
  const out = [];
  for (const [key, entry] of Object.entries(maturity.packages ?? {})) {
    if (entry?.consumerStatus === 'reference-only') out.push({ package: entry.name ?? key });
  }
  return out.sort((a, b) => a.package.localeCompare(b.package));
}

function knownPackageNames(maturityText) {
  const maturity = JSON.parse(maturityText);
  const names = new Set();
  for (const entry of Object.values(maturity.packages ?? {}))
    if (entry?.name) names.add(entry.name);
  names.add('create-tauron-app');
  return Array.from(names).sort();
}

function loadSources(overrides = {}) {
  const manifests = listPackageManifests();
  const readmeText = overrides[README_REL] ?? read(README_REL);
  const scaffoldText = overrides[SCAFFOLD_REL] ?? read(SCAFFOLD_REL);
  const maturityText = overrides[MATURITY_REL] ?? read(MATURITY_REL);
  const packageTexts = {};
  for (const rel of manifests) packageTexts[rel] = overrides[rel] ?? read(rel);

  const observed = {
    readmePnpmAdd: extractReadmePnpmAdd(readmeText),
    scaffoldDependencies: extractScaffoldDeps(scaffoldText),
    privatePackages: extractPrivatePackages(manifests, packageTexts),
    referenceOnlyPackages: extractReferenceOnly(maturityText),
    knownPackages: knownPackageNames(maturityText),
  };

  const ledgerRaw =
    overrides[LEDGER_REL] ?? (existsSync(join(ROOT, LEDGER_REL)) ? read(LEDGER_REL) : null);
  const ledger = ledgerRaw ? JSON.parse(ledgerRaw) : null;
  return { observed, ledger, manifests };
}

function canonEntries(arr) {
  if (!Array.isArray(arr)) return '';
  const key = (x) =>
    JSON.stringify(Object.fromEntries(Object.entries(x).sort(([p], [q]) => p.localeCompare(q))));
  return arr.map(key).sort().join('\n');
}

function sameEntries(a, b) {
  return canonEntries(a) === canonEntries(b);
}

function runChecks(src) {
  const failures = [];
  if (!src.ledger) {
    failures.push({ rule: 'ledger-missing', message: `${LEDGER_REL} 不存在——先 --write 生成` });
    return { failures };
  }
  const L = src.ledger;
  const o = src.observed;

  if (!sameEntries(L.readmePnpmAdd ?? [], o.readmePnpmAdd)) {
    failures.push({
      rule: 'sync-readme',
      message: `README pnpm-add 广告位与台账漂移（复算 ${o.readmePnpmAdd.length} / 台账 ${(L.readmePnpmAdd ?? []).length}）`,
    });
  }
  if (!sameEntries(L.scaffoldDependencies ?? [], o.scaffoldDependencies)) {
    failures.push({
      rule: 'sync-scaffold',
      message: `create-tauron-app#dependencies 与台账漂移（复算 ${o.scaffoldDependencies.length} / 台账 ${(L.scaffoldDependencies ?? []).length}）`,
    });
  }
  if (!sameEntries(L.privatePackages ?? [], o.privatePackages)) {
    failures.push({
      rule: 'sync-private',
      message: `private:true 集合与台账漂移（复算 ${o.privatePackages.length} / 台账 ${(L.privatePackages ?? []).length}）`,
    });
  }
  if (!sameEntries(L.referenceOnlyPackages ?? [], o.referenceOnlyPackages)) {
    failures.push({
      rule: 'sync-reference',
      message: `reference-only 集合与台账漂移（复算 ${o.referenceOnlyPackages.length} / 台账 ${(L.referenceOnlyPackages ?? []).length}）`,
    });
  }

  const advertised = new Set([
    ...o.readmePnpmAdd.map((x) => x.package),
    ...o.scaffoldDependencies.map((x) => x.package),
  ]);
  const privateSet = new Set(o.privatePackages.map((x) => x.package));
  const referenceSet = new Set(o.referenceOnlyPackages.map((x) => x.package));
  const knownSet = new Set(o.knownPackages);
  const scaffoldSet = new Set(o.scaffoldDependencies.map((x) => x.package));

  for (const p of advertised) {
    if (privateSet.has(p))
      failures.push({
        rule: 'private-advertised',
        message: `${p} 是 private:true，不应出现在面向消费者的广告位`,
      });
  }
  for (const p of scaffoldSet) {
    if (referenceSet.has(p))
      failures.push({
        rule: 'reference-in-scaffold',
        message: `${p} 是 reference-only，不应进入 create-tauron-app 依赖`,
      });
  }
  for (const p of advertised) {
    if (!knownSet.has(p))
      failures.push({
        rule: 'unknown-advert',
        message: `广告位出现未收录包名 ${p}（module-maturity packages 无对应条目）`,
      });
  }
  return { failures };
}

function write() {
  const src = loadSources();
  const out = {
    schemaVersion: 1,
    purpose:
      'R-5 货架规则台账：README `pnpm add` 广告位、create-tauron-app#dependencies、所有 packages/*/package.json 的 private:true 集合、module-maturity 的 reference-only 集合——四段"观察产出"，全部由 scripts/check-shelf-rules.mjs --write 复算落盘；手写即 bug（本仓 module-maturity.json purpose 已明确拒绝手写成熟度分级）。',
    generatedBy: 'scripts/check-shelf-rules.mjs --write',
    rules: {
      sync: '四段观察必须与源码复算逐条相等（漂移=红）',
      'private-advertised': 'private:true 的包不得出现在 pnpm add / 脚手架 deps',
      'reference-in-scaffold': 'reference-only 的包不得进入脚手架 deps',
      'unknown-advert': '广告位里的 npm 名必须在 module-maturity packages 或已知集合内',
    },
    readmePnpmAdd: src.observed.readmePnpmAdd,
    scaffoldDependencies: src.observed.scaffoldDependencies,
    privatePackages: src.observed.privatePackages,
    referenceOnlyPackages: src.observed.referenceOnlyPackages,
    knownPackages: src.observed.knownPackages,
  };
  writeFileSync(join(ROOT, LEDGER_REL), JSON.stringify(out, null, 2) + '\n');
  console.log(
    `Shelf ledger written: ${LEDGER_REL} (${out.readmePnpmAdd.length} README pnpm-add / ${out.scaffoldDependencies.length} scaffold deps / ${out.privatePackages.length} private / ${out.referenceOnlyPackages.length} reference-only / ${out.knownPackages.length} known)`,
  );
}

const MUTATIONS = [
  {
    name: 'M1 · 台账多出一条 pnpm-add 条目',
    expect: 'sync-readme',
    apply: (base) => {
      const ledger = JSON.parse(JSON.stringify(base.ledger));
      ledger.readmePnpmAdd.push({ package: '@tauron/host', line: 999 });
      return { [LEDGER_REL]: JSON.stringify(ledger) };
    },
  },
  {
    name: 'M2 · 台账 pnpm-add 行号漂一位',
    expect: 'sync-readme',
    apply: (base) => {
      const ledger = JSON.parse(JSON.stringify(base.ledger));
      if (ledger.readmePnpmAdd.length > 0) ledger.readmePnpmAdd[0].line += 1;
      return { [LEDGER_REL]: JSON.stringify(ledger) };
    },
  },
  {
    name: 'M3 · 台账 private 集合漏一条',
    expect: 'sync-private',
    apply: (base) => {
      const ledger = JSON.parse(JSON.stringify(base.ledger));
      ledger.privatePackages = ledger.privatePackages.slice(1);
      return { [LEDGER_REL]: JSON.stringify(ledger) };
    },
  },
  {
    name: 'M4 · README 塞一行 pnpm add @tauron/contract-tests',
    expect: 'private-advertised',
    apply: () => {
      const readme = read(README_REL) + '\npnpm add @tauron/contract-tests\n';
      return { [README_REL]: readme };
    },
  },
  {
    name: 'M5 · 脚手架 deps 塞 @tauron/adapter-react（reference-only）',
    expect: 'reference-in-scaffold',
    apply: () => {
      const scaffold = JSON.parse(read(SCAFFOLD_REL));
      scaffold.dependencies = {
        ...(scaffold.dependencies ?? {}),
        '@tauron/adapter-react': 'workspace:*',
      };
      return { [SCAFFOLD_REL]: JSON.stringify(scaffold, null, 2) };
    },
  },
  {
    name: 'M6 · README 塞一行 pnpm add @tauron/ghost-kit（未收录）',
    expect: 'unknown-advert',
    apply: () => {
      const readme = read(README_REL) + '\npnpm add @tauron/ghost-kit\n';
      return { [README_REL]: readme };
    },
  },
  {
    name: 'M7 · module-maturity 里 adapter-react 改成 repo-consumed',
    expect: 'sync-reference',
    apply: () => {
      const maturity = JSON.parse(read(MATURITY_REL));
      maturity.packages['tauron-adapter-react'].consumerStatus = 'repo-consumed';
      return { [MATURITY_REL]: JSON.stringify(maturity, null, 2) };
    },
  },
];

function selfTest() {
  const base = loadSources();
  if (!base.ledger) {
    console.error('Shelf self-test cannot start: ledger missing');
    process.exit(1);
  }
  const baseRun = runChecks(base);
  if (baseRun.failures.length > 0) {
    console.error('Shelf self-test baseline is not green:');
    for (const f of baseRun.failures) console.error(`  · [${f.rule}] ${f.message}`);
    process.exit(1);
  }
  const results = [];
  let pass = 0;
  for (const m of MUTATIONS) {
    const patch = m.apply(base);
    const mutated = loadSources(patch);
    const run = runChecks(mutated);
    const fired = run.failures.some((f) => f.rule === m.expect);
    results.push({ name: m.name, expect: m.expect, fired, seen: run.failures.map((f) => f.rule) });
    if (fired) pass += 1;
  }
  if (pass !== MUTATIONS.length) {
    console.error(
      `Shelf self-test FAILED: ${pass}/${MUTATIONS.length} mutations fired expected rule`,
    );
    for (const r of results) {
      console.error(
        `  · ${r.name} → expect=${r.expect} fired=${r.fired ? 'yes' : 'NO'} seen=[${r.seen.join(', ')}]`,
      );
    }
    process.exit(1);
  }
  console.log(`Shelf self-test OK: ${pass}/${MUTATIONS.length} mutations fired expected rule`);
}

function check() {
  const src = loadSources();
  const { failures } = runChecks(src);
  if (failures.length > 0) {
    console.error('Shelf-Rules gate FAILED:');
    for (const f of failures) console.error(`  · [${f.rule}] ${f.message}`);
    process.exit(1);
  }
  const o = src.observed;
  console.log(
    `Shelf-Rules gate OK：README 广告位 ${o.readmePnpmAdd.length} 条 / 脚手架 deps ${o.scaffoldDependencies.length} 条 / private ${o.privatePackages.length} 条 / reference-only ${o.referenceOnlyPackages.length} 条 / 已知包 ${o.knownPackages.length} 条——private 未上广告、reference 未进脚手架、名字全部收录。`,
  );
}

const argv = process.argv.slice(2);
if (argv.includes('--write')) write();
else if (argv.includes('--self-test')) selfTest();
else check();
