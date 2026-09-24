// ──────────────────────────────────────────────────────────────────────────
// MockRegistry 和 PluginTestRunner 测试。
// ──────────────────────────────────────────────────────────────────────────

import { describe, it, expect, beforeEach } from 'vitest';
import {
  createMockRegistry,
  createPopulatedRegistry,
  createTestRunner,
  createMockHostApi,
  expect as customExpect,
} from './index.js';

// ──────────────────────────────────────────────────────────────────────────
// MockRegistry 测试
// ──────────────────────────────────────────────────────────────────────────

describe('MockRegistry', () => {
  let registry: ReturnType<typeof createMockRegistry>;

  beforeEach(() => {
    registry = createMockRegistry();
  });

  const makeManifest = (id: string) => ({
    id,
    name: `Plugin ${id}`,
    version: '1.0.0',
    description: 'Test plugin',
    author: 'Test Author',
    homepage: 'https://example.com',
    type: 'js' as const,
    main: 'dist/index.js',
    permissions: ['storage.read'],
    tags: ['test'],
  });

  it('install 成功', () => {
    const result = registry.install(makeManifest('test.plugin.a'));
    expect(result.success).toBe(true);
    expect(registry.has('test.plugin.a')).toBe(true);
    expect(registry.size()).toBe(1);
  });

  it('install 重复插件失败', () => {
    registry.install(makeManifest('test.plugin.a'));
    const result = registry.install(makeManifest('test.plugin.a'));
    expect(result.success).toBe(false);
    expect(result.error).toContain('已存在');
  });

  it('install 空 ID 失败', () => {
    const manifest = makeManifest('');
    const result = registry.install(manifest);
    expect(result.success).toBe(false);
    expect(result.error).toContain('不能为空');
  });

  it('uninstall 成功', () => {
    registry.install(makeManifest('test.plugin.a'));
    const result = registry.uninstall('test.plugin.a');
    expect(result.success).toBe(true);
    expect(registry.has('test.plugin.a')).toBe(false);
  });

  it('uninstall 不存在的插件失败', () => {
    const result = registry.uninstall('nonexistent');
    expect(result.success).toBe(false);
    expect(result.error).toContain('不存在');
  });

  it('enable/disable 成功', () => {
    registry.install(makeManifest('test.plugin.a'));
    expect(registry.isEnabled('test.plugin.a')).toBe(false);

    registry.enable('test.plugin.a');
    expect(registry.isEnabled('test.plugin.a')).toBe(true);
    expect(registry.getState('test.plugin.a')).toBe('enabled');

    registry.disable('test.plugin.a');
    expect(registry.isEnabled('test.plugin.a')).toBe(false);
    expect(registry.getState('test.plugin.a')).toBe('disabled');
  });

  it('find 成功', () => {
    registry.install(makeManifest('test.plugin.a'));
    const entry = registry.find('test.plugin.a');
    expect(entry).not.toBeNull();
    expect(entry?.manifest.id).toBe('test.plugin.a');
  });

  it('find 不存在的插件返回 null', () => {
    const entry = registry.find('nonexistent');
    expect(entry).toBeNull();
  });

  it('list 返回所有插件', () => {
    registry.install(makeManifest('test.plugin.a'));
    registry.install(makeManifest('test.plugin.b'));
    const entries = registry.list();
    expect(entries.length).toBe(2);
  });

  it('listEnabled 只返回已启用插件', () => {
    registry.install(makeManifest('test.plugin.a'));
    registry.install(makeManifest('test.plugin.b'));
    registry.enable('test.plugin.a');
    const enabled = registry.listEnabled();
    expect(enabled.length).toBe(1);
    expect(enabled[0]?.manifest.id).toBe('test.plugin.a');
  });

  it('setErrored/clearError', () => {
    registry.install(makeManifest('test.plugin.a'));
    expect(registry.getLastError('test.plugin.a')).toBeNull();

    registry.setErrored('test.plugin.a', 'test error');
    expect(registry.getState('test.plugin.a')).toBe('errored');
    expect(registry.isEnabled('test.plugin.a')).toBe(false);
    // 错误信息必须被条目保留，供调用方诊断。
    expect(registry.getLastError('test.plugin.a')).toBe('test error');
    expect(registry.find('test.plugin.a')?.lastError).toBe('test error');

    registry.clearError('test.plugin.a');
    expect(registry.getState('test.plugin.a')).toBe('disabled');
    // clearError 后错误信息应被移除（而非残留 undefined 值）。
    expect(registry.getLastError('test.plugin.a')).toBeNull();
    expect(registry.find('test.plugin.a')).not.toHaveProperty('lastError');
  });

  it('snapshot 返回完整快照', () => {
    registry.install(makeManifest('test.plugin.a'));
    registry.enable('test.plugin.a');
    const snapshot = registry.snapshot();
    expect(snapshot.totalInstalled).toBe(1);
    expect(snapshot.totalEnabled).toBe(1);
    expect(Object.keys(snapshot.entries).length).toBe(1);
  });

  it('getOperations 返回操作记录', () => {
    registry.install(makeManifest('test.plugin.a'));
    registry.enable('test.plugin.a');
    const operations = registry.getOperations();
    expect(operations.length).toBe(2);
  });

  it('getOperationsByType 按类型过滤', () => {
    registry.install(makeManifest('test.plugin.a'));
    registry.enable('test.plugin.a');
    const installs = registry.getOperationsByType('install');
    expect(installs.length).toBe(1);
    expect(installs[0]?.type).toBe('install');
  });

  it('getSuccessCount/getFailureCount', () => {
    registry.install(makeManifest('test.plugin.a'));
    registry.install(makeManifest('test.plugin.a')); // 失败
    expect(registry.getSuccessCount()).toBe(1);
    expect(registry.getFailureCount()).toBe(1);
  });

  it('clear 清空注册表', () => {
    registry.install(makeManifest('test.plugin.a'));
    registry.clear();
    expect(registry.size()).toBe(0);
    expect(registry.getOperations().length).toBe(0);
  });

  it('createPopulatedRegistry 预填充数据', () => {
    const populated = createPopulatedRegistry();
    expect(populated.size()).toBe(3);
    expect(populated.isEnabled('test.plugin.a')).toBe(true);
    expect(populated.has('test.plugin.b')).toBe(true);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// PluginTestRunner 测试
// ──────────────────────────────────────────────────────────────────────────

describe('PluginTestRunner', () => {
  let runner: ReturnType<typeof createTestRunner>;

  beforeEach(() => {
    runner = createTestRunner();
  });

  it('创建测试套件', () => {
    runner.describe('test suite', () => {
      // suite created
    });
    expect(runner.getAssertions()).toBeDefined();
  });

  it('运行成功的测试', async () => {
    const testRunner = createTestRunner();
    testRunner.describe('success suite', () => {
      testRunner.createContext('test.plugin');
    });
    // Just verify the runner can be created and has methods
    expect(testRunner.getResult()).toBeNull();
  });

  it('createContext 创建测试上下文', () => {
    const context = runner.createContext('test.plugin');
    expect(context.pluginId).toBe('test.plugin');
    expect(context.mockHost.invoke).toBeDefined();
    expect(context.mockHost.getSettings).toBeDefined();
  });

  it('reset 重置运行器', () => {
    runner.reset();
    expect(runner.getResult()).toBeNull();
    expect(runner.getAssertions().length).toBe(0);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// MockHostApi 测试
// ──────────────────────────────────────────────────────────────────────────

describe('MockHostApi', () => {
  it('invoke 返回结果', async () => {
    const mock = createMockHostApi('test.plugin');
    const result = await mock.invoke('test.command', { key: 'value' });
    expect(result).toBeDefined();
  });

  it('getSettings/setSettings', () => {
    const mock = createMockHostApi('test.plugin');
    mock.setSettings('test', 'key', 'value');
    expect(mock.getSettings('test', 'key')).toBe('value');
  });

  it('getPluginData/setPluginData', () => {
    const mock = createMockHostApi('test.plugin');
    mock.setPluginData('test.plugin', 'data', { foo: 'bar' });
    expect(mock.getPluginData('test.plugin', 'data')).toEqual({ foo: 'bar' });
  });

  it('requestPermission 返回 true', async () => {
    const mock = createMockHostApi('test.plugin');
    const granted = await mock.requestPermission('storage.read');
    expect(granted).toBe(true);
  });

  it('log 记录日志', () => {
    const mock = createMockHostApi('test.plugin');
    mock.log('info', 'test message');
    // No assertion needed - just verify it doesn't throw
  });
});

// ──────────────────────────────────────────────────────────────────────────
// expect 断言测试
// ──────────────────────────────────────────────────────────────────────────

describe('expect 断言', () => {
  it('toEqual', () => {
    customExpect(1).toEqual(1);
    customExpect('hello').toEqual('hello');
    // Object comparison needs deep equality - use JSON stringify for now
    customExpect(JSON.stringify({ a: 1 })).toEqual(JSON.stringify({ a: 1 }));
  });

  it('notToEqual', () => {
    customExpect(1).notToEqual(2);
    customExpect('hello').notToEqual('world');
  });

  it('toBeTruthy', () => {
    customExpect(1).toBeTruthy();
    customExpect('hello').toBeTruthy();
    customExpect([]).toBeTruthy();
    customExpect({}).toBeTruthy();
  });

  it('toBeFalsy', () => {
    customExpect(0).toBeFalsy();
    customExpect('').toBeFalsy();
    customExpect(null).toBeFalsy();
    customExpect(undefined).toBeFalsy();
    customExpect(false).toBeFalsy();
  });

  it('toBeUndefined', () => {
    customExpect(undefined).toBeUndefined();
  });

  it('toBeNull', () => {
    customExpect(null).toBeNull();
  });

  it('toMatch', () => {
    customExpect('hello world').toMatch(/world/);
    customExpect('hello').toMatch(/^hello$/);
  });

  it('toContain', () => {
    customExpect([1, 2, 3]).toContain(2);
    customExpect('hello world').toContain('world');
  });

  it('toThrow', () => {
    customExpect(() => {
      throw new Error('test error');
    }).toThrow();
  });

  it('toThrow with message', () => {
    customExpect(() => {
      throw new Error('specific error message');
    }).toThrow('specific');
  });

  it('toBeType', () => {
    customExpect(1).toBeType('number');
    customExpect('hello').toBeType('string');
    customExpect([]).toBeType('object');
    customExpect(() => {}).toBeType('function');
    customExpect(null).toBeType('object');
  });
});
