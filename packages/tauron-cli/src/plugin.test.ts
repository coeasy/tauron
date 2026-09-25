import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  pluginNew,
  generatePluginManifest,
  derivePluginId,
  DEFAULT_MANIFEST_PERMISSIONS,
} from './plugin.js';
import type { PluginConfig } from './types.js';

const OPTS = { verbose: false, dryRun: false, force: false };

/** 读取真实的权限词表，用于断言脚手架默认权限**真的**合法。 */
const PERMISSION_INDEX: { entries: { identifier: string }[] } = JSON.parse(
  readFileSync(join(__dirname, '..', '..', '..', 'schema', 'permissions.index.json'), 'utf8'),
);
const INDEX_IDS = new Set(PERMISSION_INDEX.entries.map((e) => e.identifier));

describe('pluginNew', () => {
  const baseConfig: PluginConfig = { name: 'test-plugin', type: 'js' };

  it('creates plugin with required files', () => {
    const result = pluginNew(baseConfig, OPTS);
    expect(result.files['package.json']).toBeDefined();
    expect(result.files['tauron.plugin.json']).toBeDefined();
    expect(result.files['src/index.js']).toBeDefined();
    expect(result.files['README.md']).toBeDefined();
  });

  // ── 回归：断链（plugin new 不产 tauron.plugin.json） ─────────────────
  it('生成 tauron.plugin.json（生命周期命令读的就是它）', () => {
    // 回归判据：`plugin dev|test|pack|sign|publish` 全部 `readFileSync(cwd/tauron.plugin.json)`，
    // 并在缺失时提示「Run "tauron plugin new" to create a plugin first」。
    // 早期 `plugin new` 只产 package.json，于是那条提示是个死循环指令。
    const result = pluginNew(baseConfig, OPTS);
    const raw = result.files['tauron.plugin.json'];
    expect(raw).toBeDefined();
    const manifest = JSON.parse(raw!);
    expect(manifest.id).toBeTruthy();
    expect(manifest.framework).toBeTruthy();
    expect(manifest.entry).toBeDefined();
  });

  it('sets correct plugin name in package.json', () => {
    const result = pluginNew(baseConfig, OPTS);
    const pkg = JSON.parse(result.files['package.json']!);
    expect(pkg.name).toBe('test-plugin');
  });

  it('generates JS plugin entry for js type', () => {
    const result = pluginNew(baseConfig, OPTS);
    expect(result.files['src/index.js']).toContain('registerPlugin');
    expect(result.files['src/index.js']).toContain("name: 'test-plugin'");
  });

  it('generates Cargo.toml for wasm type', () => {
    const result = pluginNew({ ...baseConfig, type: 'wasm' }, OPTS);
    expect(result.files['Cargo.toml']).toBeDefined();
    expect(result.files['src/lib.rs']).toBeDefined();
  });

  it('wasm 骨架不引用不存在的 tauron_wasm::plugin 宏', () => {
    // 回归判据：`tauron-wasm` 既不是 proc-macro crate，也没有 `plugin` 属性宏；
    // 早期生成的 `use tauron_wasm::plugin; #[plugin] fn …` 编译不过。
    const result = pluginNew({ ...baseConfig, type: 'wasm' }, OPTS);
    const lib = result.files['src/lib.rs']!;
    expect(lib).not.toContain('#[plugin]');
    expect(lib).not.toContain('use tauron_wasm::plugin');
    // 也不该声明未发布的 crates.io 依赖
    expect(result.files['Cargo.toml']).not.toContain('tauron-wasm = "0.1"');
  });

  it('generates main.js for process type', () => {
    const result = pluginNew({ ...baseConfig, type: 'process' }, OPTS);
    expect(result.files['main.js']).toBeDefined();
    expect(result.files['main.js']).toContain('createServer');
  });

  it('returns correct dirName', () => {
    const result = pluginNew(baseConfig, OPTS);
    expect(result.dirName).toBe('test-plugin');
  });
});

describe('generatePluginManifest', () => {
  const baseConfig: PluginConfig = { name: 'test-plugin', type: 'js' };

  it('generates valid manifest', () => {
    const manifest = generatePluginManifest(baseConfig);
    expect(manifest.name).toBe('test-plugin');
    expect(manifest.version).toBe('0.1.0');
    expect(manifest.type).toBe('js');
    expect(manifest.entry).toEqual({ js: 'src/index.js' });
    expect(manifest.framework).toBe('>=2.0 <3.0');
  });

  it('默认权限取自真实权限词表（表外权限会被宿主拒绝）', () => {
    // 回归判据：早期默认写的是框架 ACL 词表 `store:read` / `http:fetch`，
    // 它们不在 schema/permissions.index.json 里，照抄会被 validate() 拒掉。
    const manifest = generatePluginManifest(baseConfig);
    expect(manifest.permissions.length).toBeGreaterThan(0);
    for (const p of manifest.permissions) {
      expect(INDEX_IDS.has(p), `${p} 不在 schema/permissions.index.json 中`).toBe(true);
    }
    for (const p of DEFAULT_MANIFEST_PERMISSIONS) {
      expect(INDEX_IDS.has(p), `默认权限 ${p} 不在词表中`).toBe(true);
    }
  });

  it('id 是合法的反域名', () => {
    const manifest = generatePluginManifest(baseConfig);
    const parts = manifest.id.split('.');
    expect(parts.length).toBeGreaterThanOrEqual(2);
    for (const p of parts) {
      expect(p).toMatch(/^[a-z0-9-]+$/);
    }
  });

  it('wasm 类型声明 abi.wasm（D13 加载期硬校验）', () => {
    const manifest = generatePluginManifest({ ...baseConfig, type: 'wasm' });
    expect(manifest.abi?.wasm).toBeTruthy();
    expect(manifest.entry.wasm).toBeTruthy();
  });

  it('js / process 类型不声明 abi（非 A/D 类声明 abi 会被拒）', () => {
    expect(generatePluginManifest({ ...baseConfig, type: 'js' }).abi).toBeUndefined();
    expect(generatePluginManifest({ ...baseConfig, type: 'process' }).abi).toBeUndefined();
  });

  it('process 类型用 sidecar 入口', () => {
    expect(generatePluginManifest({ ...baseConfig, type: 'process' }).entry).toEqual({
      sidecar: 'main.js',
    });
  });

  it('package.json 的 type 是 module（不是插件形态）', () => {
    // 回归判据：早期这里写的是 `type: config.type`（`js`/`process`/`wasm`）——
    // 那是 npm 的模块系统字段，写错会让 Node 按 CommonJS 解析 `.js`，
    // 而入口用的是 ESM `import`，加载直接失败。
    const result = pluginNew(baseConfig, OPTS);
    const pkg = JSON.parse(result.files['package.json']!);
    expect(pkg.type).toBe('module');
    for (const t of ['js', 'process', 'wasm']) {
      expect(pkg.type).not.toBe(t);
    }
  });

  it('package.json 不重复声明 permissions（避免两处漂移）', () => {
    const result = pluginNew(baseConfig, OPTS);
    const pkg = JSON.parse(result.files['package.json']!);
    expect(pkg.permissions).toBeUndefined();
    // 权限只写在宿主清单里
    const manifest = JSON.parse(result.files['tauron.plugin.json']!);
    expect(manifest.permissions.length).toBeGreaterThan(0);
  });
});

describe('derivePluginId', () => {
  it('普通名字加 com.example. 前缀', () => {
    expect(derivePluginId('my-plugin')).toBe('com.example.my-plugin');
  });

  it('非法字符替换为连字符', () => {
    expect(derivePluginId('My Plugin!')).toBe('com.example.my-plugin');
  });

  it('已是反域名则原样保留', () => {
    expect(derivePluginId('com.acme.formatter')).toBe('com.acme.formatter');
  });

  it('空名/纯符号名不产出非法 id', () => {
    const id = derivePluginId('!!!');
    const parts = id.split('.');
    expect(parts.length).toBeGreaterThanOrEqual(2);
    for (const p of parts) expect(p).toMatch(/^[a-z0-9-]+$/);
  });
});
