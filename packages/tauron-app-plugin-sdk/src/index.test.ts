import { describe, expect, it } from 'vitest';
import { createPlugin, createPluginContext } from './index.js';
import { createPluginTestContext, createMockContext } from './testing.js';
import type { PluginDefinition } from './types.js';

describe('createPlugin', () => {
  const def: PluginDefinition = {
    id: 'com.example.test',
    name: 'Test Plugin',
    version: '1.0.0',
  };

  it('创建实例', () => {
    const plugin = createPlugin(def);
    expect(plugin.id).toBe('com.example.test');
    expect(plugin.name).toBe('Test Plugin');
    expect(plugin.version).toBe('1.0.0');
    expect(plugin.isActive).toBe(false);
  });

  it('activate 调用钩子', async () => {
    let activated = false;
    const plugin = createPlugin({
      ...def,
      async activate() {
        activated = true;
      },
    });
    const ctx = createMockContext();
    await plugin.activate(ctx);
    expect(activated).toBe(true);
    expect(plugin.isActive).toBe(true);
  });

  it('deactivate 调用钩子', async () => {
    let deactivated = false;
    const plugin = createPlugin({
      ...def,
      async activate() {},
      async deactivate() {
        deactivated = true;
      },
    });
    const ctx = createMockContext();
    await plugin.activate(ctx);
    await plugin.deactivate(ctx);
    expect(deactivated).toBe(true);
    expect(plugin.isActive).toBe(false);
  });

  it('activate 不重复调用', async () => {
    let count = 0;
    const plugin = createPlugin({
      ...def,
      async activate() { count++; },
    });
    const ctx = createMockContext();
    await plugin.activate(ctx);
    await plugin.activate(ctx);
    expect(count).toBe(1);
  });

  it('声明式命令注册', async () => {
    const plugin = createPlugin({
      ...def,
      commands: {
        hello: (args: { name: string }) => ({ greeting: `Hello ${args.name}` }),
      },
    });
    const ctx = createMockContext();
    await plugin.activate(ctx);
    expect(ctx.commands.has('hello')).toBe(true);
    const result = await ctx.commands.execute('hello', { name: 'World' });
    expect(result).toEqual({ greeting: 'Hello World' });
  });

  it('声明式命令 deactivate 时注销', async () => {
    const plugin = createPlugin({
      ...def,
      commands: {
        hello: () => ({ msg: 'hi' }),
      },
    });
    const ctx = createMockContext();
    await plugin.activate(ctx);
    expect(ctx.commands.has('hello')).toBe(true);
    await plugin.deactivate(ctx);
    expect(ctx.commands.has('hello')).toBe(false);
  });

  it('dispose 清理资源', async () => {
    let disposed = false;
    const plugin = createPlugin({
      ...def,
      async activate() {},
      async dispose() { disposed = true; },
    });
    const ctx = createMockContext();
    await plugin.activate(ctx);
    await plugin.dispose(ctx);
    expect(disposed).toBe(true);
    expect(plugin.isActive).toBe(false);
  });
});

describe('createPluginContext', () => {
  it('创建上下文', () => {
    const ctx = createMockContext('com.example.test');
    expect(ctx.pluginId).toBe('com.example.test');
  });

  it('命令注册/注销', () => {
    const ctx = createMockContext();
    const result = ctx.commands.register('test', async () => ({ ok: true }));
    expect(result.ok).toBe(true);
    expect(ctx.commands.has('test')).toBe(true);
    expect(ctx.commands.list()).toContain('test');
    const unregistered = ctx.commands.unregister('test');
    expect(unregistered).toBe(true);
    expect(ctx.commands.has('test')).toBe(false);
  });

  it('命令重复注册拒绝', () => {
    const ctx = createMockContext();
    ctx.commands.register('test', async () => ({ ok: true }));
    const result = ctx.commands.register('test', async () => ({ ok: true }));
    expect(result.ok).toBe(false);
    expect(result.error).toContain('已存在');
  });

  it('命令执行', async () => {
    const ctx = createMockContext();
    ctx.commands.register('add', async (args: { a: number; b: number }) => ({
      sum: args.a + args.b,
    }));
    const result = await ctx.commands.execute('add', { a: 1, b: 2 });
    expect(result).toEqual({ sum: 3 });
  });

  it('执行不存在的命令抛错', async () => {
    const ctx = createMockContext();
    await expect(ctx.commands.execute('nonexistent')).rejects.toThrow();
  });

  it('设置 Tab 注册', () => {
    const ctx = createMockContext();
    const result = ctx.settings.registerTab({ id: 'main', title: '设置' });
    expect(result.ok).toBe(true);
    const unregistered = ctx.settings.unregisterTab('main');
    expect(unregistered).toBe(true);
  });

  it('日志输出', () => {
    const ctx = createMockContext('com.example.test');
    // 不抛错即可
    expect(() => ctx.log.info('test')).not.toThrow();
    expect(() => ctx.log.warn('test')).not.toThrow();
    expect(() => ctx.log.error('test')).not.toThrow();
  });
});

describe('createPluginTestContext', () => {
  it('创建完整测试上下文', () => {
    const { plugin, ctx, backend, host } = createPluginTestContext({
      id: 'com.example.test',
      name: 'Test',
      version: '1.0.0',
    });
    expect(plugin.id).toBe('com.example.test');
    expect(ctx.pluginId).toBe('com.example.test');
    expect(backend).toBeDefined();
    expect(host).toBeDefined();
  });

  it('activate/deactivate 可用', async () => {
    const { plugin, activate, deactivate } = createPluginTestContext({
      id: 'com.example.test',
      name: 'Test',
      version: '1.0.0',
    });
    await activate();
    expect(plugin.isActive).toBe(true);
    await deactivate();
    expect(plugin.isActive).toBe(false);
  });
});
