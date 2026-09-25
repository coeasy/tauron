import { describe, it, expect } from 'vitest';
import { createApp, IMPLEMENTED_TEMPLATES } from './scaffold.js';
import type { AppConfig } from './types.js';

const OPTS = { verbose: false, dryRun: false, force: false };

describe('createApp', () => {
  const baseConfig: AppConfig = {
    name: 'test-app',
    template: 'vanilla',
    pluginTypes: ['js'],
  };

  it('creates app with required files', () => {
    const result = createApp(baseConfig, OPTS);
    expect(result.files).toBeDefined();
    expect(result.files['package.json']).toBeDefined();
    expect(result.files['tauron.config.ts']).toBeDefined();
    expect(result.files['src/index.ts']).toBeDefined();
    expect(result.files['tsconfig.json']).toBeDefined();
    expect(result.files['.gitignore']).toBeDefined();
    expect(result.files['README.md']).toBeDefined();
  });

  it('sets correct app name in package.json', () => {
    const result = createApp(baseConfig, OPTS);
    const pkg = JSON.parse(result.files['package.json']!);
    expect(pkg.name).toBe('test-app');
  });

  it('includes @tauron/core dependency', () => {
    const result = createApp(baseConfig, OPTS);
    const pkg = JSON.parse(result.files['package.json']!);
    expect(pkg.dependencies['@tauron/core']).toBeDefined();
  });

  it('generates tauron.config.ts', () => {
    const result = createApp(baseConfig, OPTS);
    expect(result.files['tauron.config.ts']).toContain('export default');
  });

  it('returns correct dirName', () => {
    const result = createApp(baseConfig, OPTS);
    expect(result.dirName).toBe('test-app');
  });

  // ── 回归：生成物必须自洽可编译 ──────────────────────────────────────

  it('generates vite entry (index.html) and vite config', () => {
    // 回归判据：package.json 宣传 `dev: vite` / `build: vite build`，
    // 没有 index.html 时 vite 直接报 "Could not resolve entry module index.html"。
    const result = createApp(baseConfig, OPTS);
    expect(result.files['index.html']).toBeDefined();
    expect(result.files['index.html']).toContain('/src/main.ts');
    expect(result.files['vite.config.ts']).toBeDefined();
    expect(result.files['vite.config.ts']).toContain('defineConfig');
  });

  it('vanilla entry only imports real @tauron/core exports', () => {
    // 回归判据：历史上这里 import 过并不存在的 `TauronClient`。
    // 断言用正则匹配 **import 语句**而不是裸字符串——注释里提到这个名字是允许的。
    const result = createApp(baseConfig, OPTS);
    const entry = result.files['src/index.ts']!;
    expect(entry).not.toMatch(/import\s*\{[^}]*\bTauronClient\b/);
    expect(entry).toContain('createTauriBackend');
    expect(entry).toContain('invokePlugin');
  });

  it('tauron.config.ts does not import a non-existent defineConfig', () => {
    // 回归判据：`@tauron/core` 没有导出 `defineConfig`。
    const result = createApp(baseConfig, OPTS);
    const cfg = result.files['tauron.config.ts']!;
    expect(cfg).not.toMatch(/import\s*\{[^}]*\bdefineConfig\b/);
    expect(cfg).toContain('export default');
  });

  it('tsconfig has DOM lib and noEmit', () => {
    const result = createApp(baseConfig, OPTS);
    const ts = JSON.parse(result.files['tsconfig.json']!);
    expect(ts.compilerOptions.lib).toContain('DOM');
    expect(ts.compilerOptions.noEmit).toBe(true);
  });

  describe('react 模板', () => {
    const reactConfig: AppConfig = { ...baseConfig, template: 'react' };

    it('entry 落在 .tsx（JSX 写进 .ts 是编译错误）', () => {
      const result = createApp(reactConfig, OPTS);
      expect(result.files['src/index.tsx']).toBeDefined();
      expect(result.files['src/index.ts']).toBeUndefined();
      expect(result.files['src/index.tsx']).toContain('TauronProvider');
    });

    it('TauronProvider 传了必填的 backend prop', () => {
      // 回归判据：`TauronProviderProps.backend` 是必填，漏传编译失败。
      const result = createApp(reactConfig, OPTS);
      expect(result.files['src/index.tsx']).toContain('backend={createTauriBackend()}');
    });

    it('生成 App.tsx（入口 import ./App 需要有落点）', () => {
      const result = createApp(reactConfig, OPTS);
      expect(result.files['src/App.tsx']).toBeDefined();
      expect(result.files['src/App.tsx']).toContain('usePluginId');
    });

    it('package.json 含 react 与适配器依赖', () => {
      // 回归判据：入口 import 了 react / react-dom / @tauron/adapter-react，
      // 依赖清单漏掉它们则装不上。
      const result = createApp(reactConfig, OPTS);
      const pkg = JSON.parse(result.files['package.json']!);
      expect(pkg.dependencies.react).toBeDefined();
      expect(pkg.dependencies['react-dom']).toBeDefined();
      expect(pkg.dependencies['@tauron/adapter-react']).toBeDefined();
      expect(pkg.devDependencies['@types/react']).toBeDefined();
    });

    it('tsconfig 开 jsx 且 include 覆盖 .tsx', () => {
      const result = createApp(reactConfig, OPTS);
      const ts = JSON.parse(result.files['tsconfig.json']!);
      expect(ts.compilerOptions.jsx).toBe('react-jsx');
      expect(ts.include.some((p: string) => p.includes('.tsx'))).toBe(true);
    });

    it('vanilla 与 react 的入口内容不同', () => {
      const vanilla = createApp(baseConfig, OPTS);
      const react = createApp(reactConfig, OPTS);
      expect(vanilla.files['src/index.ts']).not.toEqual(react.files['src/index.tsx']);
    });
  });

  it('IMPLEMENTED_TEMPLATES 只含已有骨架实现的模板', () => {
    expect([...IMPLEMENTED_TEMPLATES]).toEqual(['vanilla', 'react']);
  });
});
