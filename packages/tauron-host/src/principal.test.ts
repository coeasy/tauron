// @vitest-environment happy-dom
// ──────────────────────────────────────────────────────────────────────────
// R4 身份主体模型测试：Principal。
//
// 覆盖三件事：
// 1. MockBackend 的主体/派生 id 一致（`pluginId()` 是 `principal()` 的派生）；
// 2. 主窗是**一等主体**（`{ kind: 'main-window' }`），不再是「null 身份」；
// 3. 畸形 `plugin-` label 归 `'invalid'`，**不得**按主窗放行（提权防线）。
// ──────────────────────────────────────────────────────────────────────────

import { describe, expect, it } from 'vitest';

import { MockBackend } from './backend.js';
import { HostClient } from './host.js';
import { principalFromLabel, TauriBackend } from './tauri-backend.js';

describe('Principal（R4 身份主体模型）', () => {
  it('MockBackend 默认是主窗主体（而不是「null 身份」）', () => {
    const backend = new MockBackend();
    expect(backend.principal()).toEqual({ kind: 'main-window', origin: null });
    // 派生便利方法保持一致。
    expect(backend.pluginId()).toBeNull();
  });

  it('pluginId 便捷选项构造插件主体，且 pluginId() 是其派生值', () => {
    const backend = new MockBackend({ pluginId: 'com.example.a' });
    expect(backend.principal()).toEqual({ kind: 'plugin', id: 'com.example.a' });
    expect(backend.pluginId()).toBe('com.example.a');
  });

  it('显式 principal 优先于 pluginId 选项', () => {
    const backend = new MockBackend({
      pluginId: 'ignored.by.principal',
      principal: { kind: 'plugin', id: 'com.example.real' },
    });
    expect(backend.principal()).toEqual({ kind: 'plugin', id: 'com.example.real' });
    expect(backend.pluginId()).toBe('com.example.real');
  });

  it('invalid 主体不派生出 pluginId（调用方应拒绝而非按主窗放行）', () => {
    const backend = new MockBackend({ principal: { kind: 'invalid', label: 'plugin-' } });
    expect(backend.principal().kind).toBe('invalid');
    expect(backend.pluginId()).toBeNull();
  });

  it('HostClient.principal 透传 Backend 主体，pluginId 由其派生', () => {
    const backend = new MockBackend({ pluginId: 'p.shell' });
    const client = new HostClient({ backend });
    expect(client.principal).toEqual({ kind: 'plugin', id: 'p.shell' });
    expect(client.pluginId).toBe('p.shell');

    const main = new HostClient({ backend: new MockBackend() });
    expect(main.principal).toEqual({ kind: 'main-window', origin: null });
    expect(main.pluginId).toBeNull();
  });
});

describe('principalFromLabel（label → 主体，与 Rust authz::resolve_principal 对应）', () => {
  it('非 plugin- label 归主窗主体', () => {
    for (const label of ['main', 'settings', 'w1']) {
      const p = principalFromLabel(label);
      expect(p.kind, `label ${label}`).toBe('main-window');
    }
  });

  it('plugin-<id> 归插件主体并带出 id', () => {
    expect(principalFromLabel('plugin-com.example.a')).toEqual({
      kind: 'plugin',
      id: 'com.example.a',
    });
  });

  it('畸形 plugin- label 归 invalid，绝不降级为主窗（提权防线）', () => {
    const p = principalFromLabel('plugin-');
    expect(p.kind).toBe('invalid');
    expect(p).not.toEqual({ kind: 'main-window', origin: null });
    // 带后缀的子窗口 label 在 TS 侧仍解析出 id（更深校验由宿主 PluginId::new 拒绝）。
    expect(principalFromLabel('plugin-com.example.a:settings')).toEqual({
      kind: 'plugin',
      id: 'com.example.a:settings',
    });
  });

  it('TauriBackend 在无 webview 上下文时返回主窗主体且 origin 未知', () => {
    const backend = new TauriBackend();
    expect(backend.principal()).toEqual({ kind: 'main-window', origin: null });
    expect(backend.pluginId()).toBeNull();
  });
});
