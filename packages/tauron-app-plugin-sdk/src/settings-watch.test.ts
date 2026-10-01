// @tauron/app-plugin-sdk — onSettingsChanged 接消息面（§33 R2-4 / W6）。
//
// 锁定的行为：
// 1. 声明 onSettingsChanged 的插件，激活时真的向宿主订阅设置变更镜像 topic
//    （此前这个钩子只在类型里存在，没有任何投递路径——写了也永远不会被调用）；
// 2. 投递只覆盖本插件命名空间的键，他人键/宿主键/畸形帧一律不触发回调；
// 3. 未获主窗批准（订阅失败）时静默降级，激活不得因此失败。

import { describe, expect, it } from 'vitest';
import { HostClient, MockBackend } from '@tauron/host';
import { createPluginContext } from './context.js';
import { createPlugin, HOST_SETTINGS_CHANGED_TOPIC } from './createPlugin.js';
import type { PluginDefinition } from './types.js';

const PLUGIN_ID = 'com.example.test';

const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

function makeHarness(): {
  backend: MockBackend;
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
  return { backend, ctx: createPluginContext(PLUGIN_ID, host) };
}

const subscribedTopics = (backend: MockBackend): unknown[] =>
  backend.invocations
    .filter((i) => i.cmd === 'host_events_subscribe')
    .map((i) => (i.args as { sub?: Array<{ topic: string }> })?.sub?.[0]?.topic);

const base: PluginDefinition = {
  id: PLUGIN_ID,
  name: 'Settings Watch Plugin',
  version: '1.0.0',
};

describe('onSettingsChanged 投递链', () => {
  it('声明钩子即订阅宿主设置变更镜像 topic', async () => {
    const { backend, ctx } = makeHarness();
    const plugin = createPlugin({ ...base, onSettingsChanged: () => {} });

    await plugin.activate(ctx);
    await tick();

    expect(subscribedTopics(backend)).toEqual([HOST_SETTINGS_CHANGED_TOPIC]);

    await plugin.deactivate(ctx);
    await tick();
    expect(backend.invocations.filter((i) => i.cmd === 'host_events_unsubscribe').length).toBe(1);
  });

  it('未声明钩子不订阅（不白拿一份跨进程可见性）', async () => {
    const { backend, ctx } = makeHarness();
    const plugin = createPlugin({ ...base });

    await plugin.activate(ctx);
    await tick();

    expect(subscribedTopics(backend)).toEqual([]);
    await plugin.deactivate(ctx);
  });

  it('本插件键投递给钩子；他人键、宿主键与畸形帧不触发', async () => {
    const { ctx } = makeHarness();
    const seen: Array<Record<string, unknown>> = [];
    const plugin = createPlugin({
      ...base,
      onSettingsChanged: (settings) => {
        seen.push(settings);
      },
    });

    await plugin.activate(ctx);
    await tick();

    const frame = (key: unknown, value: unknown) => ({ key, value, source: 'plugin', revision: 1 });
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, frame(`plugin:${PLUGIN_ID}.theme`, 'cobalt'));
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, frame('plugin:com.other.theme', 'x'));
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, frame(`plugin:${PLUGIN_ID}x.y`, 'x'));
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, frame('host.theme', 'x'));
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, { value: 'no key' });
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, null);

    expect(seen).toEqual([{ [`plugin:${PLUGIN_ID}.theme`]: 'cobalt' }]);

    await plugin.deactivate(ctx);
  });

  it('命名空间根键（等于 plugin:<id>）也算本插件', async () => {
    const { ctx } = makeHarness();
    const seen: unknown[] = [];
    const plugin = createPlugin({
      ...base,
      onSettingsChanged: (s) => {
        seen.push(s);
      },
    });

    await plugin.activate(ctx);
    await tick();
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, { key: `plugin:${PLUGIN_ID}`, value: 1 });

    expect(seen).toEqual([
      { [`plugin:${PLUGIN_ID}`]: 1 },
    ]);

    await plugin.deactivate(ctx);
  });

  it('钩子抛错（同步或 async rejection）不杀死订阅，也不冒泡给宿主', async () => {
    const { ctx } = makeHarness();
    const plugin = createPlugin({
      ...base,
      onSettingsChanged: () => {
        throw new Error('sync boom');
      },
    });
    await plugin.activate(ctx);
    await tick();

    expect(() =>
      ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, { key: `plugin:${PLUGIN_ID}.a`, value: 1 }),
    ).not.toThrow();

    const asyncPlugin = createPlugin({
      ...base,
      onSettingsChanged: async () => {
        throw new Error('async boom');
      },
    });
    await asyncPlugin.activate(ctx);
    await tick();
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, { key: `plugin:${PLUGIN_ID}.b`, value: 2 });
    await tick(); // rejection 在下一个微任务批次收口

    expect(asyncPlugin.isActive).toBe(true);
    await asyncPlugin.deactivate(ctx);
    await plugin.deactivate(ctx);
  });

  it('宿主拒绝订阅（未批准私有 topic）时降级为空操作，激活照常成功', async () => {
    // 断言落在**取件泵**上，不是 dispatchEvent：dispatchEvent 是测试/宿主主动
    // 注入的入口，本地有没有监听者都照投，证明不了「未批准就没有帧」。
    // 真机里未批准的后果是订阅拿不到 token → 泵不起跑 → `host_events_drain`
    // 一次都不会发生，钩子因此永不触发。
    const backend = new MockBackend({
      capabilities: ['host_events_subscribe', 'host_events_unsubscribe', 'host_events_drain'],
      pluginId: PLUGIN_ID,
      cases: [
        {
          cmd: 'host_events_subscribe',
          error: new Error('E_AUTH_DENIED: 私有 topic 未获主窗批准'),
        },
        { cmd: 'host_events_unsubscribe', result: undefined },
        { cmd: 'host_events_drain', result: [] },
      ],
    });
    const ctx = createPluginContext(PLUGIN_ID, new HostClient({ backend }));
    const seen: unknown[] = [];
    const plugin = createPlugin({
      ...base,
      onSettingsChanged: (s) => {
        seen.push(s);
      },
    });

    await expect(plugin.activate(ctx)).resolves.toBeUndefined();
    await tick();
    await tick();

    expect(subscribedTopics(backend)).toEqual([HOST_SETTINGS_CHANGED_TOPIC]); // 确实申请过
    expect(backend.invocations.filter((i) => i.cmd === 'host_events_drain')).toEqual([]); // 没起泵
    expect(seen).toEqual([]);
    expect(plugin.isActive).toBe(true);

    await plugin.deactivate(ctx);
  });
});
