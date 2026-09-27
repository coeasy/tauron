import { describe, expect, it } from 'vitest';
import { createPlugin } from './index.js';
import { createPluginTestContext, createMockContext } from './testing.js';
import type {
  CommandHandler,
  CommandRegisterResult,
  PluginContext,
  PluginDefinition,
} from './types.js';

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
      async activate() {
        count++;
      },
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
      async dispose() {
        disposed = true;
      },
    });
    const ctx = createMockContext();
    await plugin.activate(ctx);
    await plugin.dispose(ctx);
    expect(disposed).toBe(true);
    expect(plugin.isActive).toBe(false);
  });
});

// ── activate 失败回滚 ────────────────────────────────────────────────────
//
// 为什么单列一组：`createPlugin` 的 `activate` 是**多步副作用**流程（注册命令 →
// 注册设置 Tab → 注册贡献 → 调钩子 → 建订阅）。中途任一步抛错时，前面已落地的
// 副作用必须自己收干净。`deactivate` **兜不住**——`isActive` 只在 activate 跑完
// 才置 true，而 `deactivate` 开头就是 `if (!isActive) return`，所以「激活失败后
// 再调 deactivate」是空操作，注册过的命令会留下（跨主体调用仍会被受理并执行，
// 尽管插件自认为没激活）。
//
// 断言用**记录式上下文**而不是 `createMockContext`：后者的 `unregisterTab` 与
// 退订函数都是不记事的空实现，拿它写的回滚断言是**假绿**（撤销动作一次也没发生
// 也照样通过）。
function createTrackingContext(options?: { failSubscribeOn?: string }): {
  ctx: PluginContext;
  registered: string[];
  unregistered: string[];
  tabsRegistered: string[];
  tabsUnregistered: string[];
  subscriptions: string[];
  unsubscribed: string[];
} {
  const commands = new Map<string, CommandHandler>();
  const registered: string[] = [];
  const unregistered: string[] = [];
  const tabsRegistered: string[] = [];
  const tabsUnregistered: string[] = [];
  const subscriptions: string[] = [];
  const unsubscribed: string[] = [];

  const ctx: PluginContext = {
    pluginId: 'com.example.test',

    // `never`（而非省略返回类型）：仅抛错的 getter 会被推断成 `void`，
    // 与 `PluginContext['host']` 不兼容；`never` 对任何类型都可赋值。
    get host(): never {
      throw new Error('记录式上下文不支持 host 调用');
    },

    commands: {
      register<TArgs = unknown, TResult = unknown>(
        id: string,
        handler: CommandHandler<TArgs, TResult>,
      ): CommandRegisterResult {
        if (commands.has(id)) return { ok: false, error: `命令 "${id}" 已存在` };
        commands.set(id, handler as CommandHandler);
        registered.push(id);
        return { ok: true };
      },
      unregister(id: string): boolean {
        const existed = commands.delete(id);
        unregistered.push(id);
        return existed;
      },
      has(id: string): boolean {
        return commands.has(id);
      },
      list(): string[] {
        return [...commands.keys()];
      },
      async execute<TArgs = unknown, TResult = unknown>(
        id: string,
        args?: TArgs,
      ): Promise<TResult> {
        const handler = commands.get(id);
        if (!handler) throw new Error(`命令 "${id}" 不存在`);
        return (await handler(args as TArgs, ctx)) as TResult;
      },
    },

    settings: {
      registerTab(config): CommandRegisterResult {
        tabsRegistered.push(config.id);
        return { ok: true };
      },
      unregisterTab(id: string): boolean {
        tabsUnregistered.push(id);
        return true;
      },
    },

    events: {
      async publish(): Promise<void> {},
      subscribe(topic: string): () => void {
        if (options?.failSubscribeOn === topic) {
          throw new Error(`订阅 "${topic}" 失败`);
        }
        subscriptions.push(topic);
        return () => {
          unsubscribed.push(topic);
        };
      },
    },

    log: {
      info() {},
      warn() {},
      error() {},
    },
  };

  return {
    ctx,
    registered,
    unregistered,
    tabsRegistered,
    tabsUnregistered,
    subscriptions,
    unsubscribed,
  };
}

describe('createPlugin activate 失败回滚', () => {
  const def: PluginDefinition = {
    id: 'com.example.test',
    name: 'Test Plugin',
    version: '1.0.0',
  };

  it('activate 钩子抛错 → 已注册命令与设置 Tab 全部回滚，isActive 保持 false', async () => {
    const t = createTrackingContext();
    const plugin = createPlugin({
      ...def,
      commands: { hello: () => ({ msg: 'hi' }), bye: () => ({ msg: 'bye' }) },
      settings: [{ id: 'main', title: '设置' }],
      activate() {
        // 故意在「命令 / Tab 已落地」之后失败：真实场景里最容易漏收尾的时点
        throw new Error('激活钩子失败');
      },
    });

    await expect(plugin.activate(t.ctx)).rejects.toThrow('激活钩子失败');

    expect(plugin.isActive).toBe(false);
    expect(t.registered).toEqual(['hello', 'bye']);
    expect(t.unregistered).toEqual(['hello', 'bye']);
    expect(t.ctx.commands.has('hello')).toBe(false);
    expect(t.ctx.commands.has('bye')).toBe(false);
    expect(t.tabsRegistered).toEqual(['main']);
    expect(t.tabsUnregistered).toEqual(['main']);
  });

  it('事件订阅中途失败 → 已建立的订阅被释放，先前注册的命令 / Tab 一并回滚', async () => {
    const t = createTrackingContext({ failSubscribeOn: 'topic.b' });
    const plugin = createPlugin({
      ...def,
      commands: { hello: () => ({ msg: 'hi' }) },
      settings: [{ id: 'main', title: '设置' }],
      events: { subscribe: ['topic.a', 'topic.b'] },
    });

    await expect(plugin.activate(t.ctx)).rejects.toThrow('订阅 "topic.b" 失败');

    expect(plugin.isActive).toBe(false);
    expect(t.subscriptions).toEqual(['topic.a']);
    // 悬挂订阅必须被释放：否则宿主侧会一直往一个「已死」的插件推事件
    expect(t.unsubscribed).toEqual(['topic.a']);
    expect(t.unregistered).toEqual(['hello']);
    expect(t.tabsUnregistered).toEqual(['main']);
  });

  it('回滚只撤销「本次真的注册成功」的项——同名冲突不会误撤别人的命令', async () => {
    const t = createTrackingContext();
    // 「别人」先占住 hello：本次 register 会返回 ok:false，不该进回滚账本
    t.ctx.commands.register('hello', () => ({ owner: 'other' }));

    const plugin = createPlugin({
      ...def,
      commands: { hello: () => ({ owner: 'plugin' }), bye: () => ({ owner: 'plugin' }) },
      activate() {
        throw new Error('boom');
      },
    });

    await expect(plugin.activate(t.ctx)).rejects.toThrow('boom');

    expect(t.unregistered).toEqual(['bye']);
    expect(t.ctx.commands.has('hello')).toBe(true);
    await expect(t.ctx.commands.execute('hello')).resolves.toEqual({ owner: 'other' });
  });

  it('激活失败后再调 deactivate 是空操作（证明回滚不能依赖它）', async () => {
    const t = createTrackingContext();
    const plugin = createPlugin({
      ...def,
      commands: { hello: () => ({ msg: 'hi' }) },
      activate() {
        throw new Error('boom');
      },
    });

    await expect(plugin.activate(t.ctx)).rejects.toThrow('boom');
    const alreadyUndone = t.unregistered.length;

    // isActive 为 false → deactivate 开头即返回，不会再做任何撤销
    await expect(plugin.deactivate(t.ctx)).resolves.toBeUndefined();
    expect(t.unregistered.length).toBe(alreadyUndone);
    await expect(plugin.dispose(t.ctx)).resolves.toBeUndefined();
  });

  it('失败后可重新激活：回滚干净，重试能注册成功', async () => {
    const t = createTrackingContext();
    let attempts = 0;
    const plugin = createPlugin({
      ...def,
      commands: { hello: () => ({ msg: 'hi' }) },
      activate() {
        attempts++;
        if (attempts === 1) throw new Error('首次失败');
      },
    });

    await expect(plugin.activate(t.ctx)).rejects.toThrow('首次失败');
    // 关键：若没回滚，这条命令会残留，重试时 register 因「已存在」返回 ok:false
    expect(t.ctx.commands.has('hello')).toBe(false);

    await plugin.activate(t.ctx);
    expect(plugin.isActive).toBe(true);
    expect(t.ctx.commands.has('hello')).toBe(true);
    await expect(t.ctx.commands.execute('hello')).resolves.toEqual({ msg: 'hi' });
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
