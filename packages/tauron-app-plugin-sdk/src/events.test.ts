// @tauron/app-plugin-sdk — 事件订阅链路测试。
//
// 锁定的行为：声明式 events.subscribe 必须真正向宿主申请订阅，
// 宿主投递的事件必须送达插件（此前这条链路是断的）。

import { describe, expect, it } from 'vitest';
import { HostClient, MockBackend } from '@tauron/host';
import { createPluginContext } from './context.js';
import { createPlugin } from './createPlugin.js';
import type { PluginDefinition } from './types.js';

const PLUGIN_ID = 'com.example.test';

/** 让已排队的微任务全部落地。 */
const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

function makeHarness(): {
  backend: MockBackend;
  host: HostClient;
  ctx: ReturnType<typeof createPluginContext>;
} {
  const backend = new MockBackend({
    capabilities: ['host_events_subscribe', 'host_events_unsubscribe'],
    pluginId: PLUGIN_ID,
    cases: [
      { cmd: 'host_events_subscribe', result: { token: 'sub-1', selectors: [] } },
      { cmd: 'host_events_unsubscribe', result: undefined },
    ],
  });
  const host = new HostClient({ backend });
  const ctx = createPluginContext(PLUGIN_ID, host);
  return { backend, host, ctx };
}

const countOf = (backend: MockBackend, cmd: string): number =>
  backend.invocations.filter((i) => i.cmd === cmd).length;

describe('事件订阅链路', () => {
  it('subscribe 会真正向宿主申请订阅，dispatchEvent 送达订阅者', async () => {
    const { backend, ctx } = makeHarness();
    const seen: Array<{ topic: string; payload: unknown; pluginId: string }> = [];

    ctx.events.subscribe('com.example.topic', (payload, event) => {
      seen.push({ topic: event.topic, payload, pluginId: event.pluginId });
    });

    await tick();
    expect(countOf(backend, 'host_events_subscribe')).toBe(1);
    expect(backend.invocations[0]?.args).toEqual({ sub: [{ topic: 'com.example.topic' }] });

    ctx.dispatchEvent('com.example.topic', { v: 1 });
    expect(seen).toEqual([{ topic: 'com.example.topic', payload: { v: 1 }, pluginId: PLUGIN_ID }]);
  });

  it('同一 topic 的多个订阅者只向宿主订阅一次', async () => {
    const { backend, ctx } = makeHarness();
    const off1 = ctx.events.subscribe('dup', () => {});
    ctx.events.subscribe('dup', () => {});
    await tick();
    expect(countOf(backend, 'host_events_subscribe')).toBe(1);
    off1();
  });

  it('全部退订后释放宿主订阅', async () => {
    const { backend, ctx } = makeHarness();
    const off = ctx.events.subscribe('t1', () => {});
    await tick();
    off();
    await tick();
    expect(countOf(backend, 'host_events_unsubscribe')).toBe(1);
  });

  it('订阅建立途中全部退订时立即回收，不产生悬挂订阅', async () => {
    const { backend, ctx } = makeHarness();
    const off = ctx.events.subscribe('race', () => {});
    off(); // 宿主订阅尚未 resolve
    await tick();
    expect(countOf(backend, 'host_events_unsubscribe')).toBe(1);
  });

  it('单个订阅者抛错不影响其他订阅者', async () => {
    const { ctx } = makeHarness();
    const seen: unknown[] = [];
    ctx.events.subscribe('boom', () => {
      throw new Error('listener failed');
    });
    ctx.events.subscribe('boom', (payload) => seen.push(payload));

    expect(() => ctx.dispatchEvent('boom', 'ok')).not.toThrow();
    expect(seen).toEqual(['ok']);
  });

  it('dispatchEvent 在无订阅者时是安全的空操作', () => {
    const { ctx } = makeHarness();
    expect(() => ctx.dispatchEvent('nobody', {})).not.toThrow();
  });

  it('disposeEvents 释放全部宿主订阅', async () => {
    const { backend, ctx } = makeHarness();
    ctx.events.subscribe('a', () => {});
    ctx.events.subscribe('b', () => {});
    await tick();
    await ctx.disposeEvents();
    expect(countOf(backend, 'host_events_unsubscribe')).toBe(2);
  });
});

describe('createPlugin 声明式事件', () => {
  const def: PluginDefinition = {
    id: PLUGIN_ID,
    name: 'Test Plugin',
    version: '1.0.0',
  };

  it('events.subscribe 声明的事件投递到 onEvent', async () => {
    const { backend, ctx } = makeHarness();
    const received: Array<{ topic: string; payload: unknown }> = [];

    const plugin = createPlugin({
      ...def,
      events: { subscribe: ['com.example.topic'] },
      onEvent: (topic, payload) => {
        received.push({ topic, payload });
      },
    });

    await plugin.activate(ctx);
    await tick();
    expect(countOf(backend, 'host_events_subscribe')).toBe(1);

    ctx.dispatchEvent('com.example.topic', { n: 7 });
    expect(received).toEqual([{ topic: 'com.example.topic', payload: { n: 7 } }]);
  });

  it('未声明订阅的 topic 不会被投递', async () => {
    const { ctx } = makeHarness();
    const received: unknown[] = [];

    const plugin = createPlugin({
      ...def,
      events: { subscribe: ['declared'] },
      onEvent: (topic) => received.push(topic),
    });

    await plugin.activate(ctx);
    await tick();

    ctx.dispatchEvent('undeclared', {});
    expect(received).toEqual([]);

    ctx.dispatchEvent('declared', {});
    expect(received).toEqual(['declared']);
  });

  it('deactivate 后释放宿主订阅且不再投递', async () => {
    const { backend, ctx } = makeHarness();
    const received: unknown[] = [];

    const plugin = createPlugin({
      ...def,
      events: { subscribe: ['com.example.topic'] },
      onEvent: (topic) => received.push(topic),
    });

    await plugin.activate(ctx);
    await tick();
    await plugin.deactivate(ctx);
    await tick();

    expect(countOf(backend, 'host_events_unsubscribe')).toBe(1);

    ctx.dispatchEvent('com.example.topic', {});
    expect(received).toEqual([]);
  });

  it('取件泵：订阅建立后自动 drain 并把宿主帧分发给订阅者', async () => {
    // 断链回归：宿主总线是拉取模型——此前 subscribe 侧建立后无人取件，
    // 帧只进队列，本地订阅者永远收不到（「只订阅不取件 = 没订阅」）。
    const backend = new MockBackend({
      capabilities: ['host_events_subscribe', 'host_events_unsubscribe', 'host_events_drain'],
      pluginId: PLUGIN_ID,
      cases: [
        { cmd: 'host_events_subscribe', result: { token: 'sub-1', selectors: [] } },
        { cmd: 'host_events_unsubscribe', result: undefined },
        // 发布走可靠通道（request）；event 通道为空——按 args 分通道断言，
        // 证明泵两路都取且不会把同一批帧重复分发。
        {
          cmd: 'host_events_drain',
          args: { kind: 'request' },
          result: [{ topic: 'com.example.topic', seq: 1, payload: { v: 42 } }],
        },
        { cmd: 'host_events_drain', args: { kind: 'event' }, result: [] },
      ],
    });
    const host = new HostClient({ backend });
    const ctx = createPluginContext(PLUGIN_ID, host);
    const seen: unknown[] = [];
    ctx.events.subscribe('com.example.topic', (payload) => {
      seen.push(payload);
    });

    await tick(); // 订阅落地 → 泵起跑取第一拍
    await tick(); // drain promise 链落地 → 分发
    expect(seen).toEqual([{ v: 42 }]);
    expect(
      backend.invocations.filter((i) => i.cmd === 'host_events_drain').length,
    ).toBeGreaterThanOrEqual(2); // request + event 两路都取

    await ctx.disposeEvents(); // 收泵，不把定时器带进后续用例
  });

  it('声明式 contributes 会在激活时注册到宿主（best-effort，不阻断激活）', async () => {
    const backend = new MockBackend({
      capabilities: ['host_contributes_register', 'host_events_subscribe', 'host_events_drain'],
      pluginId: PLUGIN_ID,
      cases: [
        { cmd: 'host_contributes_register', result: undefined },
        { cmd: 'host_events_subscribe', result: { token: 'sub-1', selectors: [] } },
        { cmd: 'host_events_drain', result: [] },
      ],
    });
    const host = new HostClient({ backend });
    const ctx = createPluginContext(PLUGIN_ID, host);
    const plugin = createPlugin({
      ...def,
      contributes: {
        commands: [{ id: 'hello', title: 'Hello' }],
        menus: [{ id: 'm1', command: 'hello' }],
      },
    });

    await plugin.activate(ctx);
    await tick();

    const invs = backend.invocations.filter((i) => i.cmd === 'host_contributes_register');
    expect(invs.map((i) => i.args?.entry)).toEqual([
      { kind: 'command', id: 'hello', label: 'Hello' },
      { kind: 'menu', id: 'm1', label: 'hello' },
    ]);
    // self 档：线形不带 pluginId，署名由宿主从 label 解析。
    for (const inv of invs) {
      expect(inv.args).not.toHaveProperty('pluginId');
      expect(inv.args?.entry).not.toHaveProperty('pluginId');
    }
    expect(plugin.isActive).toBe(true);

    await ctx.disposeEvents();
  });

  it('settings.registerTab 会同步喂给宿主贡献表（本地 Map 只是去重账本）', async () => {
    // 断链回归：应用的设置中心读的是宿主贡献表（host_contributes_list）——
    // 不喂就是「注册了但永远不可见」。
    const backend = new MockBackend({
      capabilities: ['host_contributes_register'],
      pluginId: PLUGIN_ID,
      cases: [{ cmd: 'host_contributes_register', result: undefined }],
    });
    const host = new HostClient({ backend });
    const ctx = createPluginContext(PLUGIN_ID, host);

    expect(ctx.settings.registerTab({ id: 'main', title: '设置' })).toEqual({ ok: true });
    // 重复 id：本地拒绝，不上报宿主。
    expect(ctx.settings.registerTab({ id: 'main', title: '设置' }).ok).toBe(false);

    await tick();
    const invs = backend.invocations.filter((i) => i.cmd === 'host_contributes_register');
    expect(invs.length).toBe(1);
    expect(invs[0]?.args).toEqual({ entry: { kind: 'settings', id: 'main', label: '设置' } });

    await ctx.disposeEvents();
  });
});
