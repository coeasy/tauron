// PluginJsRuntime 单元测试

import { describe, expect, it, beforeEach } from 'vitest';
import { PluginJsRuntime } from '../src/plugin-js';

describe('PluginJsRuntime', () => {
  let runtime: PluginJsRuntime;

  beforeEach(() => {
    runtime = new PluginJsRuntime();
  });

  describe('初始状态', () => {
    it('默认配置正确', () => {
      expect(runtime.config.maxActiveIdentities).toBe(8);
      expect(runtime.config.idleTimeout).toBe(300000);
      expect(runtime.config.enableStateSnapshot).toBe(true);
      expect(runtime.config.maxStateSnapshots).toBe(16);
    });

    it('空身份单元列表', () => {
      expect(runtime.identities.length).toBe(0);
      expect(runtime.activeCount).toBe(0);
    });

    it('支持自定义配置', () => {
      const runtime = new PluginJsRuntime({ maxActiveIdentities: 4, enableStateSnapshot: false });
      expect(runtime.config.maxActiveIdentities).toBe(4);
      expect(runtime.config.enableStateSnapshot).toBe(false);
    });
  });

  describe('createIdentity', () => {
    it('正常创建', () => {
      const result = runtime.createIdentity('test.plugin');
      expect(result.ok).toBe(true);
      expect(runtime.identities.length).toBe(1);
      expect(runtime.activeCount).toBe(1);
    });

    it('生成 label', () => {
      runtime.createIdentity('test.plugin');
      const identity = runtime.identities[0];
      expect(identity?.label).toBe('plugin-test.plugin');
    });

    it('无效插件 ID 拒绝', () => {
      const invalidIds = ['invalid', 'UPPER', '123', 'test', 'test-plugin', '-test', 'test-', 'test..plugin'];
      for (const id of invalidIds) {
        const result = runtime.createIdentity(id);
        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.code).toBe('E_INVALID_PLUGIN_ID');
      }
    });

    it('重复创建拒绝', () => {
      runtime.createIdentity('test.plugin');
      const result = runtime.createIdentity('test.plugin');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_ALREADY_EXISTS');
    });

    it('达到上限时 LRU 驱逐', () => {
      const runtime = new PluginJsRuntime({ maxActiveIdentities: 2 });
      runtime.createIdentity('test.a');
      runtime.createIdentity('test.b');
      // 创建第三个，应驱逐最旧的
      const result = runtime.createIdentity('test.c');
      expect(result.ok).toBe(true);
      expect(runtime.identities.length).toBe(2);
      // 检查是否驱逐了 test.a（最旧的）
      const ids = runtime.identities.map((i) => i.pluginId);
      expect(ids).not.toContain('test.a');
      expect(ids).toContain('test.b');
      expect(ids).toContain('test.c');
    });
  });

  describe('mountView/unmountView', () => {
    beforeEach(() => {
      runtime.createIdentity('test.plugin');
    });

    it('正常挂载视图', () => {
      const result = runtime.mountView('test.plugin', 'https://example.com');
      expect(result.ok).toBe(true);
      expect(runtime.identities[0]?.state).toBe('active');
    });

    it('固定 sandbox="allow-scripts"', () => {
      runtime.mountView('test.plugin', 'https://example.com');
      const identity = runtime.identities[0];
      if (identity?.viewConfig) {
        expect(identity.viewConfig.sandbox).toBe('allow-scripts');
        expect(identity.viewConfig.allowSameOrigin).toBe(false);
      }
    });

    it('不存在的身份单元拒绝', () => {
      const result = runtime.mountView('test.nonexistent', 'https://example.com');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_FOUND');
    });

    it('已活跃时拒绝', () => {
      runtime.mountView('test.plugin', 'https://example.com');
      const result = runtime.mountView('test.plugin', 'https://example.com');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_ALREADY_ACTIVE');
    });

    it('正常卸载视图', () => {
      runtime.mountView('test.plugin', 'https://example.com');
      const result = runtime.unmountView('test.plugin');
      expect(result.ok).toBe(true);
      expect(runtime.identities[0]?.state).toBe('idle');
    });

    it('未活跃时拒绝卸载', () => {
      const result = runtime.unmountView('test.plugin');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_ACTIVE');
    });
  });

  describe('destroyIdentity', () => {
    it('正常销毁', () => {
      runtime.createIdentity('test.plugin');
      const result = runtime.destroyIdentity('test.plugin');
      expect(result.ok).toBe(true);
      expect(runtime.identities.length).toBe(0);
    });

    it('不存在的身份单元拒绝', () => {
      const result = runtime.destroyIdentity('test.nonexistent');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_FOUND');
    });

    it('销毁时清理 contributes', () => {
      runtime.createIdentity('test.plugin');
      runtime.registerContributes('test.plugin', 'view', 'main', {});
      runtime.destroyIdentity('test.plugin');
      expect(runtime.contributes.length).toBe(0);
    });

    it('销毁活跃身份单元', () => {
      runtime.createIdentity('test.plugin');
      runtime.mountView('test.plugin', 'https://example.com');
      const result = runtime.destroyIdentity('test.plugin');
      expect(result.ok).toBe(true);
      expect(runtime.identities.length).toBe(0);
    });
  });

  describe('LRU 驱逐', () => {
    it('驱逐时保留状态快照', () => {
      const runtime = new PluginJsRuntime({ maxActiveIdentities: 2, enableStateSnapshot: true });
      runtime.createIdentity('test.a');
      runtime.mountView('test.a', 'https://example.com');
      runtime.createIdentity('test.b');
      runtime.mountView('test.b', 'https://example.com');
      // 创建第三个，应驱逐 test.a
      runtime.createIdentity('test.c');
      const snapshot = runtime.getStateSnapshot('test.a');
      expect(snapshot).toBeDefined();
    });

    it('禁用快照时不保留', () => {
      const runtime = new PluginJsRuntime({ maxActiveIdentities: 2, enableStateSnapshot: false });
      runtime.createIdentity('test.a');
      runtime.mountView('test.a', 'https://example.com');
      runtime.createIdentity('test.b');
      runtime.mountView('test.b', 'https://example.com');
      runtime.createIdentity('test.c');
      const snapshot = runtime.getStateSnapshot('test.a');
      expect(snapshot).toBeUndefined();
    });
  });

  describe('registerContributes/unregisterContributes', () => {
    beforeEach(() => {
      runtime.createIdentity('test.plugin');
    });

    it('正常注册', () => {
      const result = runtime.registerContributes('test.plugin', 'view', 'main', { title: 'Main View' });
      expect(result.ok).toBe(true);
      expect(runtime.contributes.length).toBe(1);
    });

    it('重复注册拒绝', () => {
      runtime.registerContributes('test.plugin', 'view', 'main', {});
      const result = runtime.registerContributes('test.plugin', 'view', 'main', {});
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_ALREADY_EXISTS');
    });

    it('不存在的身份单元拒绝', () => {
      const result = runtime.registerContributes('test.nonexistent', 'view', 'main', {});
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_FOUND');
    });

    it('正常注销', () => {
      runtime.registerContributes('test.plugin', 'view', 'main', {});
      const result = runtime.unregisterContributes('test.plugin', 'view', 'main');
      expect(result.ok).toBe(true);
      expect(runtime.contributes.length).toBe(0);
    });

    it('不存在的 contributes 拒绝', () => {
      const result = runtime.unregisterContributes('test.plugin', 'view', 'main');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_FOUND');
    });
  });

  describe('状态快照', () => {
    it('获取快照', () => {
      const runtime = new PluginJsRuntime({ maxActiveIdentities: 2 });
      runtime.createIdentity('test.a');
      runtime.mountView('test.a', 'https://example.com');
      runtime.createIdentity('test.b');
      runtime.mountView('test.b', 'https://example.com');
      runtime.createIdentity('test.c');
      const snapshot = runtime.getStateSnapshot('test.a');
      expect(snapshot).toBeDefined();
      if (snapshot) {
        expect(snapshot.savedAt).toBeGreaterThan(0);
      }
    });

    it('清除快照', () => {
      const runtime = new PluginJsRuntime({ maxActiveIdentities: 2 });
      runtime.createIdentity('test.a');
      runtime.mountView('test.a', 'https://example.com');
      runtime.createIdentity('test.b');
      runtime.mountView('test.b', 'https://example.com');
      runtime.createIdentity('test.c');
      runtime.clearStateSnapshot('test.a');
      const snapshot = runtime.getStateSnapshot('test.a');
      expect(snapshot).toBeUndefined();
    });

    it('快照数量上限', () => {
      const runtime = new PluginJsRuntime({ maxActiveIdentities: 2, maxStateSnapshots: 2 });
      // 创建多个身份单元并驱逐
      runtime.createIdentity('test.a');
      runtime.mountView('test.a', 'https://example.com');
      runtime.createIdentity('test.b');
      runtime.mountView('test.b', 'https://example.com');
      runtime.createIdentity('test.c');
      // 驱逐 test.a，快照数量 = 1
      runtime.createIdentity('test.d');
      // 驱逐 test.b，快照数量 = 2
      runtime.createIdentity('test.e');
      // 驱逐 test.c，应删除最旧的快照
      const snapshotC = runtime.getStateSnapshot('test.c');
      expect(snapshotC).toBeUndefined();
    });
  });

  describe('subscribe', () => {
    it('订阅事件', () => {
      const events: string[] = [];
      const unsub = runtime.subscribe((event) => events.push(event));
      runtime.createIdentity('test.plugin');
      expect(events.length).toBe(1);
      expect(events[0]).toBe('identity:created:test.plugin');
      unsub();
      runtime.mountView('test.plugin', 'https://example.com');
      expect(events.length).toBe(1);
    });
  });
});
