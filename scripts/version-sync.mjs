#!/usr/bin/env node
// 发布链的**生产者**。`.github/workflows/release.yml` 的 version-check 会读六处版本号并
// 要求它们全等于 tag；此前仓内只有那个「校验器」，没有写齐它们的那一手——于是每次发版
// 都要人肉改六个文件，漏一个的表征正是 release.yml 注释里记着的老事故：
// 「tag v0.2.0、安装包却写 0.1.0」。
//
// 口径：**根 package.json 的 version 是唯一事实源**，其余全部向它对齐。
// 用法：
//   node scripts/version-sync.mjs --check          # 只判定，不写（CI 可用）
//   node scripts/version-sync.mjs --set 1.2.0      # 设新版本并写齐全部落点
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');

// 与 release.yml 的 version-check **同一组落点**，不多不少：校验器读什么，生产者就写什么。
const JSON_FILES = ['package.json', 'examples/minimal-app/package.json'];
const CONF_FILES = ['examples/minimal-app/src-tauri/tauri.conf.json'];
const CARGO_FILES = ['Cargo.toml', 'examples/minimal-app/src-tauri/Cargo.toml'];

const packageFiles = () => {
  const out = [];
  for (const dir of readdirSync(join(ROOT, 'packages'))) {
    const rel = `packages/${dir}/package.json`;
    if (existsSync(join(ROOT, rel))) out.push(rel);
  }
  return out.sort();
};

const targets = () => [...JSON_FILES, ...packageFiles(), ...CONF_FILES, ...CARGO_FILES];

/** Cargo.toml 的 `[workspace.package] version`：release.yml 用 `head -1` 取第一条，这里同口径。 */
const readCargoVersion = (text) => /^version = "([^"]*)"$/m.exec(text)?.[1];
const writeCargoVersion = (text, version) =>
  text.replace(/^version = "[^"]*"$/m, `version = "${version}"`);

const readVersion = (rel) => {
  const text = readFileSync(join(ROOT, rel), 'utf8');
  return rel.endsWith('.toml') ? readCargoVersion(text) : JSON.parse(text).version;
};

const writeVersion = (rel, version) => {
  const path = join(ROOT, rel);
  const text = readFileSync(path, 'utf8');
  if (rel.endsWith('.toml')) {
    writeFileSync(path, writeCargoVersion(text, version));
    return;
  }
  const doc = JSON.parse(text);
  doc.version = version;
  // 与 prettier 对 JSON 的默认口径一致（2 空格 + 行尾换行），否则 format:check 会红。
  writeFileSync(path, `${JSON.stringify(doc, null, 2)}\n`);
};

const args = process.argv.slice(2);
const setAt = args.indexOf('--set');
const check = args.includes('--check');
if (setAt !== -1 && !check) {
  const version = args[setAt + 1];
  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(String(version ?? ''))) {
    throw new Error(`--set 需要一个语义化版本号，收到：${version}`);
  }
  const all = targets();
  const changed = all.filter((rel) => readVersion(rel) !== version);
  for (const rel of changed) writeVersion(rel, version);
  console.log(`version-sync: ${version} 写入 ${changed.length}/${all.length} 个文件`);
  process.exit(0);
}

// --check：根版本号是事实源，其余必须逐一对齐。
const want = readVersion('package.json');
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(String(want))) {
  throw new Error(`根 package.json 的 version 不是语义化版本号：${want}`);
}
const drift = [];
for (const rel of targets()) {
  const got = readVersion(rel);
  if (got !== want) drift.push(`${rel} = ${got ?? '(缺失)'}，应为 ${want}`);
}
if (drift.length > 0) {
  console.error(`version-sync: ${drift.length} 处版本号掉队（事实源 package.json = ${want}）`);
  for (const line of drift) console.error(`  ${line}`);
  console.error('  修复：node scripts/version-sync.mjs --set <版本>');
  process.exit(1);
}
console.log(`version-sync OK: ${targets().length} 处版本号全部为 ${want}`);
