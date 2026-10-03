// ──────────────────────────────────────────────────────────────────────────
// 依赖 pin 的单一真源守卫（发版漂移门禁）。
//
// 为什么单独设一个测试，而不是让生成器各写各的字面量：脚手架一旦把依赖固定到
// **上一个**版本，第三方 `npm install && cargo check` 依然可能通过（装到的是
// 旧包），于是版本漂移以「绿色」的形式存活。这里反过来钉住：改 package.json 的
// 版本号却没同步 `FRAMEWORK_VERSION` → 当场红。
// ──────────────────────────────────────────────────────────────────────────

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import {
  FRAMEWORK_VERSION,
  PUBLISHED_FRAMEWORK_VERSION,
  REGISTRY_PIN_IS_PUBLISHED,
} from './framework-version.js';
import { generatePackageJson, generateFiles, validateConfig } from './scaffold.js';
import { generatePluginManifest } from './plugin.js';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PKG_ROOT = path.join(HERE, '..');
const REPO_ROOT = path.join(PKG_ROOT, '..', '..');

function versionOf(packageJsonPath: string): string {
  return JSON.parse(readFileSync(packageJsonPath, 'utf8')).version as string;
}

/** `[workspace.package]` 段的版本，不是某条依赖的 `version.workspace = true`。 */
function cargoWorkspaceVersion(): string {
  const cargo = readFileSync(path.join(REPO_ROOT, 'Cargo.toml'), 'utf8');
  const section = cargo.slice(cargo.indexOf('[workspace.package]'));
  const version = /^version = "([^"]+)"/m.exec(section)?.[1];
  if (!version) throw new Error('Cargo.toml 的 [workspace.package] 里没解析出版本');
  return version;
}

const registryConfig = validateConfig({
  name: 'pin-check',
  framework: 'vanilla',
  targets: ['desktop'],
  shell: 'tauri',
  capabilities: [],
});

describe('FRAMEWORK_VERSION 单一真源', () => {
  it('与本包 package.json 同版本（发版必须先改这里）', () => {
    expect(FRAMEWORK_VERSION).toBe(versionOf(path.join(PKG_ROOT, 'package.json')));
  });

  /**
   * 轮 17：registry 现值与源码版本是**两个事实**，各有其主。
   * `@tauron/cli` 的插件脚手架已经踩过一次（把生成工程钉在未发布的 CLI 版本上，
   * 用户按 README 跑 `npm install` 直接 ETARGET）；本包的应用脚手架是同一根因的
   * 第二实例，差别只在它降 pin 也不对（生成码面向源码 API），所以这里钉的是
   * 「未发布必须说出来」这条链路的数据源。
   */
  it('PUBLISHED_FRAMEWORK_VERSION 取自本包 package.json 的 tauron.publishedNpmVersion', () => {
    const pkg = JSON.parse(readFileSync(path.join(PKG_ROOT, 'package.json'), 'utf8')) as {
      version: string;
      tauron?: { publishedNpmVersion?: string };
    };
    expect(
      pkg.tauron?.publishedNpmVersion,
      '缺 tauron.publishedNpmVersion：未发布状态无从判起',
    ).toBeDefined();
    expect(PUBLISHED_FRAMEWORK_VERSION).toBe(pkg.tauron?.publishedNpmVersion ?? pkg.version);
    expect(PUBLISHED_FRAMEWORK_VERSION).toMatch(/^\d+\.\d+\.\d+$/);
  });

  it('REGISTRY_PIN_IS_PUBLISHED 完全由两个版本是否相等决定', () => {
    expect(REGISTRY_PIN_IS_PUBLISHED).toBe(PUBLISHED_FRAMEWORK_VERSION === FRAMEWORK_VERSION);
  });

  it('与仓库根 package.json 同版本', () => {
    expect(FRAMEWORK_VERSION).toBe(versionOf(path.join(REPO_ROOT, 'package.json')));
  });

  it('与 Cargo [workspace.package] 同版本', () => {
    expect(FRAMEWORK_VERSION).toBe(cargoWorkspaceVersion());
  });

  it('与 @tauron/host 的包版本同版本', () => {
    expect(FRAMEWORK_VERSION).toBe(
      versionOf(path.join(REPO_ROOT, 'packages/tauron-host/package.json')),
    );
  });

  it('生成的前端依赖 pin 取自该真源', () => {
    const files = generateFiles(registryConfig);
    const frontend = JSON.parse(files.get('package.json')!) as {
      dependencies: Record<string, string>;
    };
    expect(frontend.dependencies['@tauron/host']).toBe(FRAMEWORK_VERSION);
    expect(generatePackageJson(registryConfig)).toBe(
      files.get('package.json')!.replace('\r\n', '\n'),
    );
  });

  it('生成的 Cargo pin 取自该真源', () => {
    const cargoToml = generateFiles(registryConfig).get('src-tauri/Cargo.toml')!;
    expect(cargoToml).toContain(`tauron-shell = { version = "=${FRAMEWORK_VERSION}"`);
    expect(cargoToml).toContain(`tauron-adapter = { version = "=${FRAMEWORK_VERSION}"`);
  });

  it('插件 manifest 的 framework 区间取自该真源', () => {
    const manifest = JSON.parse(
      generatePluginManifest({
        id: 'pin.check',
        name: 'pin-check',
        pluginType: 'js',
        version: '0.1.0',
        permissions: [],
        enabled: true,
      }),
    ) as { framework: string };
    expect(manifest.framework).toBe(`^${FRAMEWORK_VERSION}`);
  });
});
